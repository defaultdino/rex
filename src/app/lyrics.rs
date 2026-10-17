use crate::{
    app::state::AppState,
    plex::models::{AvailableLyrics, TrackLyrics},
};

pub enum Remote<T> {
    Idle,
    Loading,
    Loaded(T),
}

pub struct Lyrics {
    pub track_key: Option<String>,
    pub lyric_id: Option<String>,
    pub available: Remote<Vec<AvailableLyrics>>,
    pub lyrics: Remote<TrackLyrics>,
}

impl Default for Lyrics {
    fn default() -> Self {
        Self {
            track_key: None,
            lyric_id: None,
            available: Remote::Idle,
            lyrics: Remote::Idle,
        }
    }
}

impl Lyrics {
    pub fn clear(&mut self) -> Lyrics {
        Lyrics::default()
    }

    pub fn set_loading_available(&mut self, track_key: &str) {
        self.track_key = Some(track_key.into());
        self.available = Remote::Loading;
    }

    pub fn set_loading_lyrics(&mut self, lyric_id: &str) {
        self.lyric_id = Some(lyric_id.into());
        self.lyrics = Remote::Loading;
    }

    pub fn set_available(&mut self, track_key: &str, available: Vec<AvailableLyrics>) {
        if self.track_key != Some(track_key.into()) {
            return;
        }

        self.available = Remote::Loaded(available);
    }

    pub fn set_lyrics(&mut self, lyric_id: &str, lyrics: TrackLyrics) {
        if self.lyric_id != Some(lyric_id.into()) {
            return;
        }

        self.lyrics = Remote::Loaded(lyrics);
    }

    pub fn is_available_for(&self, track_key: &str) -> bool {
        self.track_key == Some(track_key.into()) && matches!(self.available, Remote::Loaded(_))
    }

    pub fn is_loaded_for(&self, lyrics_id: &str) -> bool {
        self.lyric_id == Some(lyrics_id.into()) && matches!(self.lyrics, Remote::Loaded(_))
    }
}
