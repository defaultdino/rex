//! Plays media on a castv2 receiver through the Default Media Receiver. 
//! `Playback` drives either cast or local backend through the same channel.

use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender, channel, sync_channel};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use cast_sender::namespace::media::{
    GetStatusRequestData, IdleReason, LoadRequestData, Media, MediaInformation, MediaStatus,
    MusicTrackMediaMetadata, PlayerState, RequestData, SeekRequestData, StreamType,
};
use cast_sender::{App, AppId, Error as CastError, Image, Payload, Receiver};
use smol::{Timer, block_on, future};
use tracing::{debug, warn};

use crate::audio::player::{PlayerCmd, PlayerEvent};
use crate::plex::PlexClient;
use crate::plex::models::Track;

const POLL_EVERY: Duration = Duration::from_millis(500);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// connects to the cast device at `ip` (always port 8009) and plays on it
pub fn spawn(
    client: Arc<PlexClient>,
    ip: String,
) -> Result<(SyncSender<PlayerCmd>, mpsc::Receiver<PlayerEvent>)> {
    let (cmd_tx, cmd_rx) = sync_channel(32);
    let (ev_tx, ev_rx) = sync_channel(256);
    let (ready_tx, ready_rx) = channel();

    thread::Builder::new().name("cast".into()).spawn(move || {
        let session = match Session::open(&ip) {
            Ok(s) => s,
            Err(e) => {
                let _ = ready_tx.send(Err(e));
                return;
            }
        };
        let _ = ready_tx.send(Ok(()));
        Caster {
            client,
            ip,
            session: Some(session),
            events: ev_tx,
            current: None,
            next: None,
            paused: false,
            buffering: false,
            position_ms: 0,
        }
        .run(cmd_rx);
    })?;

    ready_rx.recv().context("cast thread died")??;
    Ok((cmd_tx, ev_rx))
}

struct Session {
    receiver: Receiver,
    app: App,
}

impl Session {
    fn open(ip: &str) -> Result<Self> {
        let open = async {
            let receiver = Receiver::new();
            receiver.connect(ip).await?;
            let app = receiver.launch_app(AppId::DefaultMediaReceiver).await?;
            Ok(Self { receiver, app })
        };
        let timeout = async {
            Timer::after(CONNECT_TIMEOUT).await;
            Err(anyhow!("timed out"))
        };
        block_on(future::or(open, timeout)).with_context(|| format!("casting to {ip}"))
    }

    fn media(&self, req: Media) -> Result<Vec<MediaStatus>> {
        let res = block_on(self.receiver.send_request(&self.app, req))?;
        match res.payload {
            Payload::Media(Media::MediaStatus(s)) => Ok(s.status),
            Payload::Media(Media::LoadFailed) => bail!("the cast device could not load the track"),
            Payload::Media(Media::InvalidRequest(e)) => bail!("invalid request: {:?}", e.reason),
            other => bail!("unexpected reply from the cast device: {other:?}"),
        }
    }
}

struct Current {
    track: Arc<Track>,
    media_session: Option<i32>,
}

struct Caster {
    client: Arc<PlexClient>,
    ip: String,
    session: Option<Session>,
    events: SyncSender<PlayerEvent>,
    current: Option<Current>,
    next: Option<Arc<Track>>,
    paused: bool,
    buffering: bool,
    position_ms: u64,
}

impl Caster {
    fn run(mut self, cmds: mpsc::Receiver<PlayerCmd>) {
        let mut polled = Instant::now();
        loop {
            match cmds.recv_timeout(POLL_EVERY.saturating_sub(polled.elapsed())) {
                Ok(cmd) => self.handle(cmd),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if polled.elapsed() >= POLL_EVERY {
                polled = Instant::now();
                self.poll();
            }
        }
        if let Some(s) = self.session.take() {
            block_on(async {
                let _ = s.receiver.stop_app(&s.app).await;
                s.receiver.disconnect().await;
            });
        }
    }

    fn emit(&self, ev: PlayerEvent) {
        if let PlayerEvent::Position(_) = ev {
            let _ = self.events.try_send(ev);
        } else if self.events.send(ev).is_err() {
            debug!("cast event receiver gone");
        }
    }

    fn error(&self, rating_key: &str, message: String) {
        self.emit(PlayerEvent::Error {
            rating_key: rating_key.to_owned(),
            message,
        });
    }

    fn set_buffering(&mut self, b: bool) {
        if b != self.buffering {
            self.buffering = b;
            self.emit(PlayerEvent::BufferingChanged(b));
        }
    }

    fn handle(&mut self, cmd: PlayerCmd) {
        debug!("cast cmd {cmd:?}");
        let res = match cmd {
            PlayerCmd::Load(track) => return self.start(track),
            PlayerCmd::EnqueueNext(track) => {
                self.next = Some(track);
                Ok(())
            }
            PlayerCmd::ClearNext => {
                self.next = None;
                Ok(())
            }
            PlayerCmd::Play => {
                self.paused = false;
                self.resume()
            }
            PlayerCmd::Pause => {
                self.paused = true;
                self.request(|id| Media::Pause(session(id)))
            }
            PlayerCmd::Seek(ms) => self.request(|id| {
                Media::Seek(SeekRequestData {
                    media_session_id: Some(id),
                    current_time: Some(ms as f64 / 1000.0),
                    ..Default::default()
                })
            }),
            PlayerCmd::Stop => {
                self.next = None;
                let res = self.request(|id| Media::Stop(session(id)));
                self.current = None;
                self.set_buffering(false);
                res
            }
            PlayerCmd::SetVolume(v) => match &self.session {
                Some(s) => block_on(s.receiver.set_volume(v.into(), false)).map_err(Into::into),
                None => Ok(()),
            },
        };
        if let Err(e) = res {
            self.fail(e);
        }
    }

    /// connects again if the device was lost
    fn session(&mut self) -> Result<&Session> {
        let s = match self.session.take() {
            Some(s) if block_on(s.receiver.is_connected()) => s,
            _ => Session::open(&self.ip)?,
        };
        Ok(self.session.insert(s))
    }

    /// 1. send a request about the current media
    /// 2. apply the status it answers with
    fn request(&mut self, req: impl FnOnce(i32) -> Media) -> Result<()> {
        let id = self.current.as_ref().and_then(|c| c.media_session);
        let (Some(s), Some(id)) = (&self.session, id) else {
            return Ok(());
        };
        let status = s.media(req(id))?;
        if let Some(st) = status.into_iter().find(|st| st.media_session_id == id) {
            self.on_status(st);
        }
        Ok(())
    }

    fn start(&mut self, track: Arc<Track>) {
        self.next = None;
        self.current = None;
        self.paused = false;
        self.position_ms = 0;
        self.set_buffering(false);
        match self.load(&track, 0) {
            Ok(id) => {
                self.current = Some(Current {
                    track: track.clone(),
                    media_session: Some(id),
                });
                self.emit(PlayerEvent::TrackStarted(track));
            }
            Err(e) => {
                if lost_connection(&e) {
                    self.session = None;
                }
                self.error(&track.rating_key, format!("{}: {e:#}", track.title));
            }
        }
    }

    fn load(&mut self, track: &Track, from_ms: u64) -> Result<i32> {
        if track.part_key.is_empty() {
            bail!("{} has no playable media", track.title);
        }
        let media = self.media_info(track);
        let status = self.session()?.media(Media::Load(LoadRequestData {
            media,
            autoplay: Some(true),
            current_time: Some(from_ms as f64 / 1000.0),
            ..Default::default()
        }))?;
        status
            .first()
            .map(|st| st.media_session_id)
            .context("the cast device sent no media status")
    }

    fn resume(&mut self) -> Result<()> {
        let Some(cur) = &self.current else {
            return Ok(());
        };
        if cur.media_session.is_some() {
            return self.request(|id| Media::Play(session(id)));
        }
        let track = cur.track.clone();
        let id = self.load(&track, self.position_ms)?;
        if let Some(c) = self.current.as_mut() {
            c.media_session = Some(id);
        }
        Ok(())
    }

    fn poll(&mut self) {
        let Some(id) = self.current.as_ref().and_then(|c| c.media_session) else {
            return;
        };
        let Some(s) = &self.session else { return };
        let req = Media::GetStatus(GetStatusRequestData {
            media_session_id: Some(id),
            options: None,
        });
        match s.media(req) {
            Ok(status) => match status.into_iter().find(|st| st.media_session_id == id) {
                Some(st) => self.on_status(st),
                None => self.finished(),
            },
            Err(e) => self.fail(e),
        }
    }

    fn on_status(&mut self, st: MediaStatus) {
        self.position_ms = (st.current_time * 1000.0) as u64;
        self.set_buffering(matches!(st.player_state, PlayerState::Buffering));
        match st.player_state {
            PlayerState::Playing => self.emit(PlayerEvent::Position(self.position_ms)),
            // paused on the device or by another sender
            PlayerState::Paused if !self.paused => {
                self.paused = true;
                self.emit(PlayerEvent::Paused);
            }
            PlayerState::Idle => match st.idle_reason {
                Some(IdleReason::Finished) => self.finished(),
                Some(IdleReason::Error) => {
                    if let Some(c) = &self.current {
                        let msg = format!("{}: the cast device could not play it", c.track.title);
                        self.error(&c.track.rating_key, msg);
                    }
                    self.finished();
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn finished(&mut self) {
        let Some(c) = self.current.take() else { return };
        self.set_buffering(false);
        self.emit(PlayerEvent::TrackEnded(c.track.rating_key.clone()));
        if let Some(next) = self.next.take() {
            self.start(next);
        }
    }

    fn fail(&mut self, e: anyhow::Error) {
        let rk = self.current.as_ref().map(|c| c.track.rating_key.clone());
        let rk = rk.unwrap_or_default();
        if !lost_connection(&e) {
            return self.error(&rk, format!("cast: {e:#}"));
        }
        warn!("cast device lost: {e:#}");
        self.session = None;
        self.set_buffering(false);
        if let Some(c) = self.current.as_mut() {
            c.media_session = None;
        }
        self.error(&rk, format!("cast device lost: {e:#}"));
        if !self.paused {
            self.paused = true;
            self.emit(PlayerEvent::Paused);
        }
    }

    fn media_info(&self, track: &Track) -> MediaInformation {
        let images = (!track.thumb.is_empty()).then(|| {
            vec![Image {
                url: self.client.url_with_token(&track.thumb),
                height: None,
                width: None,
            }]
        });
        let metadata = MusicTrackMediaMetadata {
            title: Some(track.title.clone()),
            artist: Some(track.grandparent_title.clone()),
            album_name: Some(track.parent_title.clone()),
            track_number: track.index.map(|n| n as i32),
            disc_number: track.disc.map(|n| n as i32),
            images,
            ..Default::default()
        };
        MediaInformation {
            // the device fetches this itself, so the server address must be reachable from it
            content_id: self.client.url_with_token(&track.part_key),
            content_type: content_type(track).into(),
            stream_type: StreamType::Buffered,
            duration: track.duration_ms.map(|d| d as f64 / 1000.0),
            metadata: Some(metadata.into()),
            ..Default::default()
        }
    }
}

fn session(id: i32) -> RequestData {
    RequestData {
        media_session_id: Some(id),
    }
}

fn lost_connection(e: &anyhow::Error) -> bool {
    matches!(
        e.downcast_ref::<CastError>(),
        Some(
            CastError::Io(_)
                | CastError::Tls(_)
                | CastError::NoConnection
                | CastError::ResponseTimeout
        )
    )
}

fn content_type(track: &Track) -> &'static str {
    match track.container.as_deref().unwrap_or_default() {
        "flac" => "audio/flac",
        "mp4" | "m4a" => "audio/mp4",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        "aac" => "audio/aac",
        _ => "audio/mpeg", // XXX: I don't think there are any other?
    }
}
