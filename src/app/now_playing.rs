use std::sync::Arc;

use crate::backend::DeviceVolume;
use crate::plex::models::Track;

#[derive(Default)]
pub struct NowPlaying {
    pub track: Option<Arc<Track>>,
    pub position_ms: u64,
    pub paused: bool,
    pub buffering: bool,
    pub volume: f32,
    pub casting: bool,
    pub device_volume: Option<DeviceVolume>,
}

impl NowPlaying {
    pub fn artist_playing_now(&self, artist_title: &str) -> bool {
        self.track
            .as_ref()
            .is_some_and(|t| t.grandparent_title == *artist_title)
    }

    pub fn album_playing_now(&self, album_key: &str) -> bool {
        self.track
            .as_ref()
            .is_some_and(|t| t.parent_rating_key == *album_key)
    }

    pub fn track_playing_now(&self, track_key: &str) -> bool {
        self.track
            .as_ref()
            .is_some_and(|t| t.rating_key == *track_key)
    }
}
