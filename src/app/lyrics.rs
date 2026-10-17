use crate::plex::models::TrackLyrics;

pub struct Lyrics {
    pub track_key: Option<String>,
    pub lyric_id: Option<String>,
    pub available: Option<Vec<String>>,
    pub lyrics: Option<TrackLyrics>,
}

impl Default for Lyrics {
    fn default() -> Self {
        Self {
            track_key: None,
            lyric_id: None,
            available: None,
            lyrics: None,
        }
    }
}

impl Lyrics {
    pub fn clear(&mut self) -> Lyrics {
        Lyrics::default()
    }

    pub fn get_available(&self, track_key: &str) -> Option<&Vec<String>> {
        if self.track_key.as_deref() == Some(track_key) {
            self.available.as_ref()
        } else {
            None
        }
    }

    pub fn set_track(&mut self, track_key: &str) {
        self.track_key = Some(track_key.to_owned());
    }

    pub fn set_lyric_id(&mut self, lyric_id: &str) {
        self.lyric_id = Some(lyric_id.to_owned());
    }

    pub fn set_available_for(&mut self, track_key: &str, lyrics: Vec<String>) {
        self.track_key = Some(track_key.to_owned());
        self.available = Some(lyrics);
    }

    pub fn set_loaded_for(&mut self, lyric_id: &str, lyrics: TrackLyrics) {
        self.lyric_id = Some(lyric_id.to_owned());
        self.lyrics = Some(lyrics);
    }

    pub fn is_available_for(&self, track_key: &str) -> bool {
        self.track_key.as_deref() == Some(track_key) && self.available.is_some()
    }

    pub fn is_loaded_for(&self, track_key: &str) -> bool {
        self.track_key.as_deref() == Some(track_key) && self.lyrics.is_some()
    }
}
