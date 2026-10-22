use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::plex::models::Track;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

impl Repeat {
    pub fn cycle(self) -> Self {
        match self {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        }
    }
}

pub struct Entry {
    pub id: u64,
    pub track: Arc<Track>,
}

/// the queue as saved between runs
#[derive(Serialize, Deserialize)]
pub struct SavedQueue {
    /// in play order
    tracks: Vec<Arc<Track>>,
    /// indices into `tracks` in insertion order, so shuffle can still be undone
    unshuffled: Vec<usize>,
    current: Option<usize>,
    pub position_ms: u64,
    shuffle: bool,
    repeat: Repeat,
}

/// tracks in play order; `unshuffled` remembers insertion order so shuffle can be undone
pub struct Queue {
    pub entries: Vec<Entry>,
    pub current: Option<usize>,
    pub shuffle: bool,
    pub repeat: Repeat,
    unshuffled: Vec<u64>,
    next_id: u64,
    rng: u64,
}

impl Default for Queue {
    fn default() -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0x9E37_79B9, |d| d.as_nanos() as u64);
        Self {
            entries: Vec::new(),
            current: None,
            shuffle: false,
            repeat: Repeat::Off,
            unshuffled: Vec::new(),
            next_id: 0,
            rng: seed | 1,
        }
    }
}

impl Queue {
    pub fn save(&self, position_ms: u64) -> Option<SavedQueue> {
        if self.entries.is_empty() {
            return None;
        }
        let index = positions(self.entries.iter().map(|e| e.id));
        let unshuffled = self
            .unshuffled
            .iter()
            .filter_map(|id| index.get(id).copied())
            .collect();
        Some(SavedQueue {
            tracks: self.entries.iter().map(|e| e.track.clone()).collect(),
            unshuffled,
            current: self.current,
            position_ms,
            shuffle: self.shuffle,
            repeat: self.repeat,
        })
    }

    pub fn restore(saved: SavedQueue) -> Self {
        let mut q = Self::default();
        q.entries = q.make(saved.tracks);
        q.unshuffled = saved
            .unshuffled
            .iter()
            .filter_map(|&i| q.entries.get(i).map(|e| e.id))
            .collect();
        q.current = saved.current.filter(|&c| c < q.entries.len());
        q.shuffle = saved.shuffle;
        q.repeat = saved.repeat;
        q
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn current(&self) -> Option<&Entry> {
        self.current.and_then(|i| self.entries.get(i))
    }

    pub fn position(&self, id: u64) -> Option<usize> {
        self.entries.iter().position(|e| e.id == id)
    }

    pub fn is_current(&self, id: u64) -> bool {
        self.current.is_some_and(|i| self.entries[i].id == id)
    }

    fn make(&mut self, tracks: Vec<Arc<Track>>) -> Vec<Entry> {
        tracks
            .into_iter()
            .map(|track| {
                self.next_id += 1;
                Entry {
                    id: self.next_id,
                    track,
                }
            })
            .collect()
    }

    pub fn replace(&mut self, tracks: Vec<Arc<Track>>, start: Option<usize>) {
        self.entries = self.make(tracks);
        self.unshuffled = self.entries.iter().map(|e| e.id).collect();
        self.current = start.filter(|&s| s < self.entries.len());
        if self.shuffle {
            self.shuffle_around_current();
        }
    }

    pub fn append(&mut self, tracks: Vec<Arc<Track>>) {
        let new = self.make(tracks);
        self.unshuffled.extend(new.iter().map(|e| e.id));
        self.entries.extend(new);
    }

    pub fn play_next(&mut self, tracks: Vec<Arc<Track>>) {
        let new = self.make(tracks);
        let at = self.current.map_or(0, |c| c + 1);
        let uat = self
            .current()
            .and_then(|e| self.unshuffled.iter().position(|&id| id == e.id))
            .map_or(0, |p| p + 1);
        let ids: Vec<u64> = new.iter().map(|e| e.id).collect();
        self.unshuffled.splice(uat..uat, ids);
        self.entries.splice(at..at, new);
    }

    /// removes entry `i`; returns true if it was the current one
    pub fn remove(&mut self, i: usize) -> bool {
        if i >= self.entries.len() {
            return false;
        }
        let e = self.entries.remove(i);
        self.unshuffled.retain(|&id| id != e.id);
        match self.current {
            Some(c) if c == i => {
                self.current = None;
                true
            }
            Some(c) if c > i => {
                self.current = Some(c - 1);
                false
            }
            _ => false,
        }
    }

    /// swaps entry `i` with its neighbour; returns the entry's new index
    pub fn move_entry(&mut self, i: usize, down: bool) -> Option<usize> {
        let j = if down {
            i.checked_add(1)?
        } else {
            i.checked_sub(1)?
        };
        if j >= self.entries.len() {
            return None;
        }
        self.entries.swap(i, j);

        if !self.shuffle {
            self.unshuffled.swap(i, j);
        }

        self.current = self.current.map(|c| match c {
            c if c == i => j,
            c if c == j => i,
            c => c,
        });
        Some(j)
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.unshuffled.clear();
        self.current = None;
    }

    pub fn set_shuffle(&mut self, on: bool) {
        if on == self.shuffle {
            return;
        }
        self.shuffle = on;
        if on {
            self.shuffle_around_current();
        } else {
            let cur = self.current().map(|e| e.id);
            let order = positions(self.unshuffled.iter().copied());
            self.entries.sort_by_key(|e| order.get(&e.id).copied());
            self.current = cur.and_then(|id| self.position(id));
        }
    }

    fn shuffle_around_current(&mut self) {
        let start = self.current.map_or(0, |c| c + 1);
        for i in (start + 1..self.entries.len()).rev() {
            let j = start + (self.rand() % (i - start + 1) as u64) as usize;
            self.entries.swap(i, j);
        }
    }

    fn rand(&mut self) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng
    }

    /// what plays after the current entry; `auto` is true when the current track ended by itself
    pub fn next_index(&self, auto: bool) -> Option<usize> {
        let len = self.entries.len();
        let Some(c) = self.current else {
            return (len > 0).then_some(0);
        };
        if auto && self.repeat == Repeat::One {
            return Some(c);
        }
        match c + 1 {
            n if n < len => Some(n),
            _ if self.repeat != Repeat::Off && len > 0 => Some(0),
            _ => None,
        }
    }

    pub fn prev_index(&self) -> Option<usize> {
        match self.current? {
            0 if self.repeat != Repeat::Off => self.entries.len().checked_sub(1),
            0 => Some(0),
            c => Some(c - 1),
        }
    }
}

/// maps each id to its index in `ids`
fn positions(ids: impl IntoIterator<Item = u64>) -> HashMap<u64, usize> {
    ids.into_iter().enumerate().map(|(i, id)| (id, i)).collect()
}
