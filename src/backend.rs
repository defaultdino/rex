//! The contract between `Playback` and whatever plays the audio: the local player or a cast
//! device. A backend runs on its own thread, takes `PlayerCmd`s and answers with `PlayerEvent`s

use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender};

use crate::plex::models::Track;

pub type Channels = (SyncSender<PlayerCmd>, Receiver<PlayerEvent>);

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
}

pub fn perceptual_gain(volume: f32) -> f32 {
    let v = volume.clamp(0.0, 1.0);
    v * v * v
}
