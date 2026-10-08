pub mod keys;
mod playback;
pub mod queue;
mod reporting;
pub mod state;

use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use tracing::warn;

use crate::audio::player::{self, PlayerCmd, PlayerEvent};
use crate::config::{self, Config};
use crate::mpris::Mpris;
use crate::plex::api::SearchResults;
use crate::plex::models::{Album, Artist, Page, Playlist, Track};
use crate::plex::{PlexClient, auth};
use reporting::Report;
use souvlaki::MediaControlEvent;
use state::{AppState, ListKind};

const API_WORKERS: usize = 2;
const REPORT_TIMEOUT: Duration = Duration::from_secs(3);
const RECONNECT_EVERY: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub enum AddMode {
    Append,
    Next,
    /// replace the queue with the fetched tracks and start at this rating key
    PlayFrom(String),
    PlayFromStart,
}

pub enum TrackSource {
    Album(String),
    Artist(String),
    Playlist(String),
}

pub enum ApiRequest {
    Page {
        view: u64,
        kind: ListKind,
        start: u32,
    },
    Tracks {
        source: TrackSource,
        mode: AddMode,
    },
    Search {
        query: String,
    },
    Report(Vec<Report>),
}

pub enum PageData {
    Artists(Page<Artist>),
    Albums(Page<Album>),
    Tracks(Page<Track>),
    Playlists(Page<Playlist>),
}

pub enum AppEvent {
    Input(Event),
    Page {
        view: u64,
        start: u32,
        result: Result<PageData, String>,
    },
    Tracks {
        mode: AddMode,
        result: Result<Vec<Track>, String>,
    },
    Search {
        query: String,
        result: Result<SearchResults, String>,
    },
    Player(PlayerEvent),
    Mpris(MediaControlEvent),
    /// SIGTERM, SIGINT or SIGHUP
    Signal,
    /// the server answered again at this address after rediscovery
    Reconnected(String),
}

pub fn run(client: Arc<PlexClient>, cfg: &mut Config) -> Result<()> {
    let section = cfg
        .music_section
        .clone()
        .context("no music section selected")?;

    let (tx, rx) = sync_channel(64);
    let (api_tx, api_rx) = sync_channel(16);

    let cache = config::dirs()?.cache_dir().join("stream");
    let (player_tx, player_rx) = player::spawn(client.clone(), cache, cfg.volume)?;

    spawn_api_workers(client.clone(), section, cfg.clone(), api_rx, tx.clone())?;
    spawn_forwarder(player_rx, tx.clone())?;

    #[cfg(unix)]
    spawn_signals(tx.clone())?;

    let mpris = Mpris::start(tx.clone());
    spawn_input(tx)?;

    spawn_tui(rx, api_tx, player_tx, mpris, &client, cfg)
}

/// runs the ui on main thread
fn spawn_tui(
    rx: Receiver<AppEvent>,
    api_tx: SyncSender<ApiRequest>,
    player_tx: SyncSender<PlayerCmd>,
    mut mpris: Option<Mpris>,
    client: &PlexClient,
    cfg: &mut Config,
) -> Result<()> {
    thread::scope(|s| {
        let tui = thread::Builder::new()
            .name("tui".into())
            .spawn_scoped(s, move || {
                let mut state = AppState::new(api_tx, player_tx.clone(), cfg.volume);
                state.set_accent(cfg.accent_color.as_deref());

                // ratatui::init installs a panic hook that restores the terminal
                let mut terminal = ratatui::init();
                let result = event_loop(&mut terminal, &mut state, &rx, mpris.as_mut(), client);
                ratatui::restore();

                shutdown(&mut state, &player_tx, client, cfg)?;
                result
            })?;
        tui.join().unwrap_or_else(|e| std::panic::resume_unwind(e))
    })
}

/// stops playback, sends the final playback reports and saves what the session changed
fn shutdown(
    state: &mut AppState,
    player: &SyncSender<PlayerCmd>,
    client: &PlexClient,
    cfg: &mut Config,
) -> Result<()> {
    let _ = player.send(PlayerCmd::Stop);
    for r in state.reporter.stopped() {
        send_report(client, &r);
    }
    cfg.volume = state.now.volume;
    if let Some(url) = state.server_url.take() {
        cfg.server_url = Some(url);
    }
    cfg.save()
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    state: &mut AppState,
    rx: &Receiver<AppEvent>,
    mut mpris: Option<&mut Mpris>,
    client: &PlexClient,
) -> Result<()> {
    loop {
        terminal.draw(|f| crate::ui::draw(f, state))?;
        let first = match state.next_deadline() {
            Some(d) => match rx.recv_timeout(d) {
                Ok(ev) => Some(ev),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
            },
            None => Some(rx.recv().context("event channel closed")?),
        };
        for ev in first.into_iter().chain(rx.try_iter()) {
            handle(state, ev);
        }
        state.expire_message();
        if state.mpris_dirty {
            state.mpris_dirty = false;
            if let Some(m) = mpris.as_deref_mut() {
                m.update(&state.now, client);
            }
        }
        if state.quit {
            return Ok(());
        }
    }
}

fn handle(state: &mut AppState, ev: AppEvent) {
    match ev {
        AppEvent::Input(Event::Key(k)) if k.kind == KeyEventKind::Press => {
            if state.search_input.is_some() {
                state.on_search_key(k);
            } else if let Some(action) = keys::lookup(k) {
                state.on_action(action);
            }
        }
        AppEvent::Input(_) => {}
        AppEvent::Page {
            view,
            start,
            result,
        } => state.on_page(view, start, result),
        AppEvent::Tracks { mode, result } => match result {
            Ok(tracks) if tracks.is_empty() => state.info("nothing to add".into()),
            Ok(tracks) => state.add_tracks(mode, tracks),
            Err(e) => state.error(e),
        },
        AppEvent::Search { query, result } => state.on_search(query, result),
        AppEvent::Player(ev) => state.on_player(ev),
        AppEvent::Mpris(ev) => state.on_mpris(ev),
        AppEvent::Signal => state.quit = true,
        AppEvent::Reconnected(url) => {
            state.info(format!("reconnected to the server at {url}"));
            state.server_url = Some(url);
        }
    }
}

#[cfg(unix)]
fn spawn_signals(tx: SyncSender<AppEvent>) -> Result<()> {
    use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
    let mut signals = signal_hook::iterator::Signals::new([SIGTERM, SIGINT, SIGHUP])?;
    thread::Builder::new()
        .name("signals".into())
        .spawn(move || {
            for _ in signals.forever() {
                if tx.send(AppEvent::Signal).is_err() {
                    break;
                }
            }
        })?;
    Ok(())
}

fn send_report(c: &PlexClient, r: &Report) {
    let result = match r {
        Report::Timeline {
            rating_key,
            state,
            time_ms,
            duration_ms,
        } => {
            let key = format!("/library/metadata/{rating_key}");
            let time = time_ms.to_string();
            let duration = duration_ms.unwrap_or(0).to_string();
            let query = [
                ("ratingKey", rating_key.as_str()),
                ("key", &key),
                ("state", state.as_str()),
                ("time", &time),
                ("duration", &duration),
            ];
            c.send("/:/timeline", &query, REPORT_TIMEOUT)
        }
        Report::Scrobble { rating_key } => {
            let query = [
                ("key", rating_key.as_str()),
                ("identifier", "com.plexapp.plugins.library"),
            ];
            c.send("/:/scrobble", &query, REPORT_TIMEOUT)
        }
    };
    if let Err(e) = result {
        warn!("playback report: {e:#}");
    }
}

fn spawn_forwarder(rx: Receiver<PlayerEvent>, tx: SyncSender<AppEvent>) -> Result<()> {
    thread::Builder::new()
        .name("player-events".into())
        .spawn(move || {
            for ev in rx {
                if tx.send(AppEvent::Player(ev)).is_err() {
                    break;
                }
            }
        })?;
    Ok(())
}

fn spawn_input(tx: SyncSender<AppEvent>) -> Result<()> {
    thread::Builder::new().name("input".into()).spawn(move || {
        while let Ok(ev) = event::read() {
            if tx.send(AppEvent::Input(ev)).is_err() {
                break;
            }
        }
    })?;
    Ok(())
}

/// what an API worker needs to serve requests and to find the server again if it moves
struct ApiContext {
    client: Arc<PlexClient>,
    section: String,
    cfg: Config,
    tx: SyncSender<AppEvent>,
    /// last reconnect attempt and whether it worked, shared so workers don't stampede plex.tv
    last_reconnect: Mutex<Option<(Instant, bool)>>,
}

impl ApiContext {
    /// runs `f`; if the server was unreachable, rediscovers it (at most every 30 s) and retries once
    fn call<T>(&self, f: impl Fn() -> Result<T>) -> Result<T> {
        match f() {
            Err(e) if crate::plex::client::unreachable(&e) && self.reconnect() => f(),
            other => other,
        }
    }

    fn reconnect(&self) -> bool {
        let mut last = self
            .last_reconnect
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some((at, ok)) = *last
            && at.elapsed() < RECONNECT_EVERY
        {
            return ok;
        }
        let result = auth::reconnect(&self.client, &self.cfg);
        *last = Some((Instant::now(), result.is_ok()));
        match result {
            Ok(url) => {
                tracing::info!("reconnected to server at {url}");
                let _ = self.tx.send(AppEvent::Reconnected(url));
                true
            }
            Err(e) => {
                warn!("reconnecting: {e:#}");
                false
            }
        }
    }

    fn serve(&self, req: ApiRequest) -> Option<AppEvent> {
        let (c, section) = (&*self.client, self.section.as_str());
        let err = |e: anyhow::Error| format!("{e:#}");
        Some(match req {
            ApiRequest::Page { view, kind, start } => AppEvent::Page {
                view,
                start,
                result: self
                    .call(|| fetch_page(c, section, &kind, start))
                    .map_err(err),
            },
            ApiRequest::Search { query } => AppEvent::Search {
                result: self.call(|| c.search(section, &query)).map_err(err),
                query,
            },
            ApiRequest::Tracks { source, mode } => AppEvent::Tracks {
                mode,
                result: self.call(|| fetch_tracks(c, &source)).map_err(err),
            },
            ApiRequest::Report(reports) => {
                reports.iter().for_each(|r| send_report(c, r));
                return None;
            }
        })
    }
}

fn spawn_api_workers(
    client: Arc<PlexClient>,
    section: String,
    cfg: Config,
    rx: Receiver<ApiRequest>,
    tx: SyncSender<AppEvent>,
) -> Result<()> {
    let rx = Arc::new(Mutex::new(rx));
    let ctx = Arc::new(ApiContext {
        client,
        section,
        cfg,
        tx,
        last_reconnect: Mutex::new(None),
    });
    for i in 0..API_WORKERS {
        let (ctx, rx) = (ctx.clone(), rx.clone());
        thread::Builder::new()
            .name(format!("api-{i}"))
            .spawn(move || {
                loop {
                    let req = match rx.lock() {
                        Ok(rx) => rx.recv(),
                        Err(_) => break,
                    };
                    let Ok(req) = req else { break };
                    if let Some(ev) = ctx.serve(req)
                        && ctx.tx.send(ev).is_err()
                    {
                        break;
                    }
                }
            })?;
    }
    Ok(())
}

fn fetch_page(c: &PlexClient, section: &str, kind: &ListKind, start: u32) -> Result<PageData> {
    Ok(match kind {
        ListKind::Artists => PageData::Artists(c.artists(section, start)?),
        ListKind::Albums => PageData::Albums(c.albums(section, start)?),
        ListKind::Playlists => PageData::Playlists(c.playlists(start)?),
        ListKind::ArtistAlbums(rk) => PageData::Albums(c.artist_albums(section, rk, start)?),
        ListKind::AlbumTracks(rk) => PageData::Tracks(c.album_tracks(rk, start)?),
        ListKind::PlaylistTracks(rk) => PageData::Tracks(c.playlist_tracks(rk, start)?),
        ListKind::Search | ListKind::Queue => anyhow::bail!("{kind:?} is not paginated"),
    })
}

fn fetch_tracks(c: &PlexClient, source: &TrackSource) -> Result<Vec<Track>> {
    let mut out = Vec::new();
    loop {
        let start = out.len() as u32;
        let page = match source {
            TrackSource::Album(rk) => c.album_tracks(rk, start)?,
            TrackSource::Artist(rk) => c.artist_tracks(rk, start)?,
            TrackSource::Playlist(rk) => c.playlist_tracks(rk, start)?,
        };
        let done = page.items.is_empty();
        out.extend(page.items);
        if done || out.len() >= page.total as usize {
            return Ok(out);
        }
    }
}
