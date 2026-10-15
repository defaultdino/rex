pub mod browser;
pub mod keys;
pub mod now_playing;
mod playback;
pub mod queue;
mod reporting;
pub mod state;

use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use image::RgbImage;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use tracing::warn;

use crate::audio::player;
use crate::backend::{PlayerCmd, PlayerEvent};
use crate::config::{self, SavedState};
use crate::media_controls::{MediaControls, run_main_loop};
use crate::plex::api::SearchResults;
use crate::plex::models::{Album, Artist, Page, Playlist, Track};
use crate::plex::{PlexClient, auth};
use browser::ListKind;
use reporting::Report;
use souvlaki::MediaControlEvent;
use state::AppState;

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
        view: u64,
        query: String,
    },
    Report(Vec<Report>),
    Cover {
        thumb: String,
    },
}

pub enum PageData {
    Artists(Page<Artist>),
    Albums(Page<Album>),
    Tracks(Page<Arc<Track>>),
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
        result: Result<Vec<Arc<Track>>, String>,
    },
    Search {
        view: u64,
        result: Result<SearchResults, String>,
    },
    Cover {
        thumb: String,
        result: Result<RgbImage, String>,
    },
    Player(PlayerEvent),
    MediaControl(MediaControlEvent),
    /// SIGTERM, SIGINT or SIGHUP
    Signal,
    /// the server answered again at this address after rediscovery
    Reconnected(String),
}

pub fn run(client: Arc<PlexClient>, saved: &mut SavedState, accent: Option<&str>) -> Result<()> {
    let section = saved
        .music_section
        .clone()
        .context("no music section selected")?;

    let (tx, rx) = sync_channel(64);
    let (api_tx, api_rx) = sync_channel(16);

    let cache = config::dirs()?.cache_dir().join("stream");
    let (player_tx, player_rx) = player::spawn(client.clone(), cache, saved.volume)?;

    spawn_api_workers(client.clone(), section, saved, api_rx, tx.clone())?;
    spawn_forwarder(player_rx, tx.clone())?;

    #[cfg(unix)]
    spawn_signals(tx.clone())?;

    let media_controls = MediaControls::start(tx.clone());
    spawn_input(tx)?;

    let mut app = AppState::new(api_tx, player_tx.clone(), saved.volume, saved.queue.take());
    app.set_accent(accent);
    spawn_tui(rx, app, player_tx, media_controls, &client, saved)
}

/// runs the ui on a scoped thread while the main thread runs the platform run loop
fn spawn_tui(
    rx: Receiver<AppEvent>,
    mut state: AppState,
    player_tx: SyncSender<PlayerCmd>,
    mut media_controls: Option<MediaControls>,
    client: &PlexClient,
    saved: &mut SavedState,
) -> Result<()> {
    thread::scope(|s| {
        let tui = thread::Builder::new()
            .name("tui".into())
            .spawn_scoped(s, move || {
                // ratatui::init installs a panic hook that restores the terminal
                let mut terminal = ratatui::init();
                let result = event_loop(
                    &mut terminal,
                    &mut state,
                    &rx,
                    media_controls.as_mut(),
                    client,
                );
                ratatui::restore();

                shutdown(&mut state, &player_tx, client, saved)?;
                result
            })?;
        run_main_loop(&tui);
        tui.join().unwrap_or_else(|e| std::panic::resume_unwind(e))
    })
}

/// stops playback, sends the final playback reports and saves what the session changed
fn shutdown(
    state: &mut AppState,
    player: &SyncSender<PlayerCmd>,
    client: &PlexClient,
    saved: &mut SavedState,
) -> Result<()> {
    let _ = player.send(PlayerCmd::Stop);
    for r in state.playback.reporter.stopped() {
        send_report(client, &r);
    }

    let disk = SavedState::read()?;
    let same_library = disk
        .as_ref()
        .is_none_or(|d| d.server_id == saved.server_id && d.music_section == saved.music_section);

    let mut out = disk.unwrap_or_else(|| std::mem::take(saved));
    let playback = &state.playback;
    out.volume = playback.now.volume;

    if same_library {
        out.queue = playback.queue.save(playback.now.position_ms);
        if let Some(url) = state.server_url.take() {
            out.server_url = Some(url);
        }
    }
    out.save()
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    state: &mut AppState,
    rx: &Receiver<AppEvent>,
    mut media_controls: Option<&mut MediaControls>,
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
        let playback = &mut state.playback;
        playback.sync_cover();
        if playback.media_controls_dirty {
            playback.media_controls_dirty = false;
            if let Some(m) = media_controls.as_deref_mut() {
                m.update(&playback.now, client);
            }
        }
        if state.quit {
            return Ok(());
        }
    }
}

fn handle(state: &mut AppState, ev: AppEvent) {
    match ev {
        AppEvent::Input(Event::Key(k)) if k.kind == KeyEventKind::Press => state.on_key(k),
        AppEvent::Input(_) => {}
        AppEvent::Page {
            view,
            start,
            result,
        } => {
            if let Err(e) = state.browser.on_page(view, start, result) {
                state.error(e);
            }
        }
        AppEvent::Tracks { mode, result } => match result {
            Ok(tracks) if tracks.is_empty() => state.info("nothing to add".into()),
            Ok(tracks) => state.playback.add_tracks(mode, tracks),
            Err(e) => state.error(e),
        },
        AppEvent::Search { view, result } => {
            if let Err(e) = state.browser.on_search(view, result) {
                state.error(e);
            }
        }
        AppEvent::Cover { thumb, result } => state.playback.on_cover(thumb, result),
        AppEvent::Player(ev) => state.playback.on_player(ev),
        AppEvent::MediaControl(MediaControlEvent::Quit) | AppEvent::Signal => state.quit = true,
        AppEvent::MediaControl(ev) => state.playback.on_media_control(ev),
        AppEvent::Reconnected(url) => {
            state.info(format!("reconnected to the server at {url}"));
            state.server_url = Some(url);
        }
    }
    if let Some(m) = state.playback.notice.take() {
        state.message = Some(m);
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
    /// what `auth::reconnect` needs to look the server up on plex.tv
    token: Option<String>,
    server_id: Option<String>,
    client_id: String,
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

        let result = auth::reconnect(
            &self.client,
            self.token.as_deref(),
            self.server_id.as_deref(),
            &self.client_id,
        );

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
            ApiRequest::Search { view, query } => AppEvent::Search {
                view,
                result: self.call(|| c.search(section, &query)).map_err(err),
            },
            ApiRequest::Tracks { source, mode } => AppEvent::Tracks {
                mode,
                result: self.call(|| fetch_tracks(c, &source)).map_err(err),
            },
            ApiRequest::Report(reports) => {
                reports.iter().for_each(|r| send_report(c, r));
                return None;
            }
            ApiRequest::Cover { thumb } => AppEvent::Cover {
                result: self.call(|| fetch_cover(c, &thumb)).map_err(err),
                thumb,
            },
        })
    }
}

fn spawn_api_workers(
    client: Arc<PlexClient>,
    section: String,
    saved: &SavedState,
    rx: Receiver<ApiRequest>,
    tx: SyncSender<AppEvent>,
) -> Result<()> {
    let rx = Arc::new(Mutex::new(rx));
    let ctx = Arc::new(ApiContext {
        client,
        section,
        token: saved.token.clone(),
        server_id: saved.server_id.clone(),
        client_id: saved.client_identifier.clone(),
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

/// longest side of the stored cover; the player pixelates it down to the cells it has
const COVER_SIZE: u32 = 128;

fn fetch_cover(c: &PlexClient, thumb: &str) -> Result<RgbImage> {
    let size = COVER_SIZE.to_string();
    let query = [("width", size.as_str()), ("height", &size), ("url", thumb)];
    let bytes = c.get_bytes("/photo/:/transcode", &query)?;
    let img = image::load_from_memory(&bytes).context("decoding cover art")?;
    Ok(img.thumbnail(COVER_SIZE, COVER_SIZE).to_rgb8())
}

fn fetch_tracks(c: &PlexClient, source: &TrackSource) -> Result<Vec<Arc<Track>>> {
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
