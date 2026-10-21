mod commands;
mod connection;
mod media;
mod messages;
mod status;

use std::sync::Arc;
use std::sync::mpsc::SyncSender;
use std::time::Instant;

use cast_sender::Payload;
use cast_sender::namespace::media::Media;
use tracing::debug;

use crate::backend::{DeviceVolume, PlayerEvent};
use crate::plex::PlexClient;
use crate::plex::models::Track;

const PLATFORM: &str = "receiver-0";
const TAKEN_OVER: &str = "another device took over the cast device";

pub struct MediaApp {
    transport: String,
    session_id: String,
}

pub enum Conn {
    Down,
    Connecting,
    Up {
        app: Option<MediaApp>,
        last_heard: Instant,
        last_ping: Instant,
    },
}

struct Current {
    track: Arc<Track>,
    media_session: Option<i32>,
    queue_item: Option<i32>,
    pending_load: Option<u32>,
    announced: bool,
}

struct QueuedNext {
    track: Arc<Track>,
    content_url: String,
    insert_req: u32,
    queue_item: Option<i32>,
}

struct Position {
    ms: u64,
    playing_since: Option<Instant>,
}

impl Position {
    fn still(ms: u64) -> Self {
        Self {
            ms,
            playing_since: None,
        }
    }
}

pub struct Caster {
    plex: Arc<PlexClient>,
    events: SyncSender<PlayerEvent>,
    pub outbox: Vec<(String, Payload, Option<u32>)>,
    pub conn: Conn,
    last_req: u32,
    current: Option<Current>,
    wanted_next: Option<Arc<Track>>,
    queued: Option<QueuedNext>,
    queue_refused: bool,
    paused: bool,
    buffering: bool,
    device_volume: Option<DeviceVolume>,
    pending_volume_req: Option<u32>,
    position: Position,
    position_emitted: Instant,
    pending_toggle: Option<u32>,
    seek_on_load: Option<u64>,
    abandoned_load: Option<u32>,
    awaiting_advance: Option<Instant>,
}

impl Caster {
    pub fn new(plex: Arc<PlexClient>, events: SyncSender<PlayerEvent>) -> Self {
        Self {
            plex,
            events,
            outbox: Vec::new(),
            conn: Conn::Down,
            last_req: 0,
            current: None,
            wanted_next: None,
            queued: None,
            queue_refused: false,
            paused: false,
            buffering: false,
            device_volume: None,
            pending_volume_req: None,
            position: Position::still(0),
            position_emitted: Instant::now(),
            pending_toggle: None,
            seek_on_load: None,
            abandoned_load: None,
            awaiting_advance: None,
        }
    }

    fn emit(&self, ev: PlayerEvent) {
        if let PlayerEvent::Position(_) = ev {
            let _ = self.events.try_send(ev);
        } else if self.events.send(ev).is_err() {
            debug!("cast event receiver gone");
        }
    }

    fn fail(&self, rating_key: Option<&str>, message: String) {
        self.emit(PlayerEvent::Error {
            rating_key: rating_key.map(str::to_owned),
            message,
        });
    }

    fn set_buffering(&mut self, b: bool) {
        if b != self.buffering {
            self.buffering = b;
            self.emit(PlayerEvent::BufferingChanged(b));
        }
    }

    fn send_platform(&mut self, payload: impl Into<Payload>, with_id: bool) -> Option<u32> {
        let req = with_id.then(|| {
            self.last_req += 1;
            self.last_req
        });
        self.outbox.push((PLATFORM.into(), payload.into(), req));
        req
    }

    fn send_media(&mut self, m: Media) -> Option<u32> {
        let Conn::Up { app: Some(app), .. } = &self.conn else {
            return None;
        };
        let to = app.transport.clone();
        self.last_req += 1;
        self.outbox.push((to, m.into(), Some(self.last_req)));
        Some(self.last_req)
    }

    fn media_session(&self) -> Option<i32> {
        self.current.as_ref().and_then(|c| c.media_session)
    }

    fn position_at(&self, now: Instant) -> u64 {
        let Position { ms, playing_since } = self.position;
        let ms =
            ms + playing_since.map_or(0, |s| now.saturating_duration_since(s).as_millis() as u64);
        let end = self.current.as_ref().and_then(|c| c.track.duration_ms);
        end.map_or(ms, |d| ms.min(d))
    }

    fn freeze(&mut self, now: Instant) {
        self.position = Position::still(self.position_at(now));
    }
}
