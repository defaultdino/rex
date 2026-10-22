use std::time::{Duration, Instant};

use crate::plex::models::Track;

const TIMELINE_EVERY: Duration = Duration::from_secs(10);
const SCROBBLE_AT: f64 = 0.9;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayState {
    Playing,
    Paused,
    Stopped,
}

impl PlayState {
    pub fn as_str(self) -> &'static str {
        match self {
            PlayState::Playing => "playing",
            PlayState::Paused => "paused",
            PlayState::Stopped => "stopped",
        }
    }
}

#[derive(Debug)]
pub enum Report {
    Timeline {
        rating_key: String,
        state: PlayState,
        time_ms: u64,
        duration_ms: Option<u64>,
    },
    Scrobble {
        rating_key: String,
    },
}

struct Play {
    rating_key: String,
    duration_ms: Option<u64>,
    state: PlayState,
    position_ms: u64,
    last_timeline: Instant,
    scrobbled: bool,
}

impl Play {
    fn timeline(&self, state: PlayState, time_ms: u64) -> Report {
        Report::Timeline {
            rating_key: self.rating_key.clone(),
            state,
            time_ms,
            duration_ms: self.duration_ms,
        }
    }
}

/// decides which timeline and scrobble requests a play produces; one `Play` per track start,
/// so seeking within a track never counts as a new play
#[derive(Default)]
pub struct Reporter {
    play: Option<Play>,
}

impl Reporter {
    pub fn started(&mut self, track: &Track, now: Instant) -> Vec<Report> {
        let mut out = self.stopped();
        let play = Play {
            rating_key: track.rating_key.clone(),
            duration_ms: track.duration_ms,
            state: PlayState::Playing,
            position_ms: 0,
            last_timeline: now,
            scrobbled: false,
        };
        out.push(play.timeline(PlayState::Playing, 0));
        self.play = Some(play);
        out
    }

    pub fn position(&mut self, ms: u64, now: Instant) -> Vec<Report> {
        let Some(p) = self.play.as_mut() else {
            return Vec::new();
        };
        p.position_ms = ms;
        let mut out = Vec::new();
        let past_threshold = p
            .duration_ms
            .is_some_and(|d| d > 0 && ms as f64 >= d as f64 * SCROBBLE_AT);
        if past_threshold && !p.scrobbled {
            p.scrobbled = true;
            out.push(Report::Scrobble {
                rating_key: p.rating_key.clone(),
            });
        }
        if p.state == PlayState::Playing && now.duration_since(p.last_timeline) >= TIMELINE_EVERY {
            p.last_timeline = now;
            out.push(p.timeline(PlayState::Playing, ms));
        }
        out
    }

    pub fn set_paused(&mut self, paused: bool, now: Instant) -> Vec<Report> {
        let Some(p) = self.play.as_mut() else {
            return Vec::new();
        };
        let state = if paused {
            PlayState::Paused
        } else {
            PlayState::Playing
        };
        if p.state == state {
            return Vec::new();
        }
        p.state = state;
        p.last_timeline = now;
        vec![p.timeline(state, p.position_ms)]
    }

    /// the track played to its end by itself
    pub fn ended(&mut self) -> Vec<Report> {
        let Some(p) = self.play.take() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if !p.scrobbled {
            out.push(Report::Scrobble {
                rating_key: p.rating_key.clone(),
            });
        }
        let end = p.duration_ms.unwrap_or(p.position_ms);
        out.push(p.timeline(PlayState::Stopped, end));
        out
    }

    /// playback stopped, quit, or moved to another track before this one finished
    pub fn stopped(&mut self) -> Vec<Report> {
        match self.play.take() {
            Some(p) => vec![p.timeline(PlayState::Stopped, p.position_ms)],
            None => Vec::new(),
        }
    }
}
