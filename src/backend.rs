//! The contract between `Playback` and whatever plays the audio: the local player or a cast
//! device. A backend runs on its own thread, takes `PlayerCmd`s and answers with `PlayerEvent`s

use std::sync::Arc;
use std::sync::mpsc::{SendError, SyncSender, TrySendError};

use crate::app::AppEvent;
use crate::plex::models::Track;

/// sends a backends events into the apps channel, tagged with the backends id so the app
/// can ignore any backends it has switched away from
#[derive(Clone)]
pub struct Events {
    tx: SyncSender<AppEvent>,
    id: u64,
}

impl Events {
    pub fn new(tx: SyncSender<AppEvent>, id: u64) -> Self {
        Self { tx, id }
    }

    pub fn send(&self, ev: PlayerEvent) -> Result<(), SendError<AppEvent>> {
        self.tx.send(AppEvent::Player(self.id, ev))
    }

    pub fn try_send(&self, ev: PlayerEvent) -> Result<(), TrySendError<AppEvent>> {
        self.tx.try_send(AppEvent::Player(self.id, ev))
    }
}

#[derive(Debug)]
pub enum PlayerCmd {
    Load(Arc<Track>),
    EnqueueNext(Arc<Track>),
    Play,
    Pause,
    Seek(u64),
    Stop,
    SetVolume(f32),
    ClearNext,
}

#[derive(Debug)]
pub enum PlayerEvent {
    Position(u64),
    TrackStarted(Arc<Track>),
    TrackEnded,
    Error {
        rating_key: Option<String>,
        message: String,
    },
    Paused,
    #[cfg_attr(not(feature = "cast"), allow(dead_code))]
    Resumed,
    BufferingChanged(bool),
    #[cfg_attr(not(feature = "cast"), allow(dead_code))]
    DeviceVolume(DeviceVolume),
}

#[cfg_attr(not(feature = "cast"), allow(dead_code))]
#[derive(Debug, Clone, Copy)]
pub struct DeviceVolume {
    pub level: f32,
    pub muted: bool,
}
