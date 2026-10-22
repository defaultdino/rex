use std::sync::Arc;
use std::sync::mpsc::{Sender, SyncSender, TrySendError};
use std::time::Instant;

use image::RgbImage;
use souvlaki::{MediaControlEvent, SeekDirection};
use tracing::warn;

use super::now_playing::NowPlaying;
use super::queue::{Queue, SavedQueue};
use super::reporting::{Report, Reporter};
use super::state::Message;
use super::{AddMode, ApiRequest, TrackSource};
use crate::backend::{DeviceVolume, PlayerCmd, PlayerEvent};
use crate::plex::models::Track;

const ENQUEUE_BEFORE_END_MS: u64 = 10_000;
const RESTART_AFTER_MS: u64 = 3_000;
const SEEK_STEP_MS: i64 = 10_000;
const VOLUME_STEP: f32 = 0.05;

/// cover art for the thumb of the track it was fetched for
pub struct Cover {
    pub thumb: String,
    pub art: Option<RgbImage>,
}

/// the queue, what is playing, and the player thread that plays it
pub struct Playback {
    api: SyncSender<ApiRequest>,
    reports: Sender<Vec<Report>>,
    player: SyncSender<PlayerCmd>,
    pub queue: Queue,
    pub now: NowPlaying,
    pub cover: Option<Cover>,
    loading: Option<(u64, String)>,
    enqueued: Option<(u64, String)>,
    failures: usize,
    /// the restored track is shown paused at this position but not loaded in the player yet
    resume_ms: Option<u64>,
    /// seek here once the track being loaded starts
    seek_on_start: Option<u64>,
    pub(super) reporter: Reporter,
    /// now-playing changed in a way media widgets should hear about
    pub media_controls_dirty: bool,
    /// status line message, moved to the app's message after each event
    pub(super) notice: Option<Message>,
}

impl Playback {
    pub fn new(
        api: SyncSender<ApiRequest>,
        reports: Sender<Vec<Report>>,
        player: SyncSender<PlayerCmd>,
        volume: f32,
        saved: Option<SavedQueue>,
    ) -> Self {
        let mut playback = Self {
            api,
            reports,
            player,
            queue: Queue::default(),
            now: NowPlaying {
                volume,
                ..Default::default()
            },
            cover: None,
            loading: None,
            enqueued: None,
            failures: 0,
            resume_ms: None,
            seek_on_start: None,
            reporter: Reporter::default(),
            media_controls_dirty: false,
            notice: None,
        };

        if let Some(saved) = saved {
            let position_ms = saved.position_ms;
            playback.queue = Queue::restore(saved);
            if let Some(e) = playback.queue.current() {
                playback.now.track = Some(e.track.clone());
                playback.now.position_ms = position_ms;
                playback.now.paused = true;
                playback.resume_ms = Some(position_ms);
                playback.media_controls_dirty = true;
            }
        }

        playback
    }

    fn error(&mut self, text: String) {
        warn!("{text}");
        self.notice = Some(Message::error(text));
    }

    fn info(&mut self, text: String) {
        self.notice = Some(Message::info(text));
    }

    /// never blocks the UI: the player thread can be stuck in a stalled read for a while
    fn send(&mut self, cmd: PlayerCmd) {
        match self.player.try_send(cmd) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.error("player is busy (waiting for the network)".into())
            }
            Err(TrySendError::Disconnected(_)) => warn!("player thread is gone"),
        }
    }

    pub fn stop_player(&self) {
        let _ = self.player.send(PlayerCmd::Stop);
    }

    #[cfg(feature = "cast")]
    pub fn set_player(&mut self, player: SyncSender<PlayerCmd>) {
        self.player = player;
    }

    #[cfg(feature = "cast")]
    pub fn switch_player(&mut self, player: SyncSender<PlayerCmd>, casting: bool) {
        self.send(PlayerCmd::Stop);
        self.player = player;
        self.now.casting = casting;
        self.now.device_volume = None;
        if !casting {
            self.send(PlayerCmd::SetVolume(self.now.volume));
        }
        if self.now.track.is_none() || self.queue.current.is_none() {
            return;
        }
        let playing = !self.now.paused && self.resume_ms.is_none();
        self.resume_ms = Some(self.resume_ms.unwrap_or(self.now.position_ms));
        self.now.paused = true;
        self.now.buffering = false;
        self.loading = None;
        self.enqueued = None;
        self.seek_on_start = None;
        if playing {
            self.toggle_pause();
        }
    }

    pub(super) fn report(&self, reports: Vec<Report>) {
        if !reports.is_empty() && self.reports.send(reports).is_err() {
            warn!("playback reporter is gone");
        }
    }

    pub fn play_index(&mut self, i: usize) {
        let Some(e) = self.queue.entries.get(i) else {
            return;
        };
        let (id, track) = (e.id, e.track.clone());
        let stopped = self.reporter.stopped();
        self.report(stopped);
        self.media_controls_dirty = true;
        self.loading = Some((id, track.rating_key.clone()));
        self.queue.current = Some(i);
        self.enqueued = None;
        self.resume_ms = None;
        self.seek_on_start = None;
        self.now.track = Some(track.clone());
        self.now.position_ms = 0;
        self.now.paused = false;
        self.now.buffering = false;
        self.send(PlayerCmd::Load(track));
    }

    /// replaces the queue with `tracks` and plays from `start`
    pub fn play_tracks(&mut self, tracks: Vec<Arc<Track>>, start: usize) {
        self.queue.replace(tracks, Some(start));
        self.play_index(self.queue.current.unwrap_or(0));
    }

    pub fn stop(&mut self) {
        let stopped = self.reporter.stopped();
        self.report(stopped);
        self.media_controls_dirty = true;
        self.send(PlayerCmd::Stop);
        self.now.track = None;
        self.now.position_ms = 0;
        self.now.paused = false;
        self.now.buffering = false;
        self.loading = None;
        self.enqueued = None;
        self.resume_ms = None;
        self.seek_on_start = None;
    }

    pub fn toggle_pause(&mut self) {
        if let Some(ms) = self.resume_ms.take() {
            if let Some(i) = self.queue.current {
                self.play_index(i);
                // after play_index, which clears it
                self.seek_on_start = Some(ms);
            }
            return;
        }
        if self.now.track.is_none() {
            if let Some(i) = self.queue.current.or_else(|| self.queue.next_index(false)) {
                self.play_index(i);
            }
            return;
        }
        self.now.paused = !self.now.paused;
        let reports = self.reporter.set_paused(self.now.paused, Instant::now());
        self.report(reports);
        self.media_controls_dirty = true;
        self.send(if self.now.paused {
            PlayerCmd::Pause
        } else {
            PlayerCmd::Play
        });
    }

    pub fn skip(&mut self) {
        match self.queue.next_index(false) {
            Some(i) => self.play_index(i),
            None => self.stop(),
        }
    }

    pub fn previous(&mut self) {
        if self.now.track.is_some() && self.now.position_ms > RESTART_AFTER_MS {
            self.seek_to(0);
        } else if let Some(i) = self.queue.prev_index() {
            self.play_index(i);
        }
    }

    pub fn seek_by(&mut self, forward: bool) {
        let delta = if forward { SEEK_STEP_MS } else { -SEEK_STEP_MS };
        self.seek_to(self.now.position_ms.saturating_add_signed(delta));
    }

    /// seeks the playing track, staying a second clear of the end
    fn seek_to(&mut self, ms: u64) {
        let Some(t) = &self.now.track else { return };
        let target = match t.duration_ms {
            Some(d) => ms.min(d.saturating_sub(1_000)),
            None => ms,
        };
        self.now.position_ms = target;
        self.media_controls_dirty = true;
        if let Some(r) = self.resume_ms.as_mut() {
            *r = target;
            return;
        }
        self.send(PlayerCmd::Seek(target));
    }

    pub fn change_volume(&mut self, up: bool) {
        let step = if up { VOLUME_STEP } else { -VOLUME_STEP };
        let current = match self.now.casting {
            true => self.now.device_volume.map(|d| d.level),
            false => Some(self.now.volume),
        };
        let Some(v) = current else { return };
        self.set_volume(((v + step) * 20.0).round() / 20.0);
    }

    fn set_volume(&mut self, v: f32) {
        let v = v.clamp(0.0, 1.0);
        if self.now.casting {
            self.now.device_volume = Some(DeviceVolume {
                level: v,
                muted: false,
            });
        } else {
            self.now.volume = v;
        }
        self.send(PlayerCmd::SetVolume(v));
    }

    pub fn toggle_shuffle(&mut self) {
        self.queue.set_shuffle(!self.queue.shuffle);
        self.sync_next();
    }

    pub fn cycle_repeat(&mut self) {
        self.queue.repeat = self.queue.repeat.cycle();
        self.sync_next();
    }

    /// keeps the player's pre-opened next track in line with the queue
    fn sync_next(&mut self) {
        let due = self.now.track.as_ref().is_some_and(|t| {
            t.duration_ms
                .is_none_or(|d| d.saturating_sub(self.now.position_ms) <= ENQUEUE_BEFORE_END_MS)
        });
        let next = self
            .queue
            .next_index(true)
            .and_then(|i| self.queue.entries.get(i))
            .filter(|_| self.now.track.is_some() && self.resume_ms.is_none());
        match (next, &self.enqueued) {
            (Some(e), Some((id, _))) if e.id == *id => {}
            (Some(e), _) if due => {
                self.enqueued = Some((e.id, e.track.rating_key.clone()));
                let track = e.track.clone();
                self.send(PlayerCmd::EnqueueNext(track));
            }
            (_, Some(_)) => {
                self.enqueued = None;
                self.send(PlayerCmd::ClearNext);
            }
            _ => {}
        }
    }

    // XXX: this function is getting impossible to maintain,
    // I would like to iron out this playback stuff eventually
    pub fn on_player(&mut self, ev: PlayerEvent) {
        match ev {
            PlayerEvent::Position(ms) => {
                self.now.position_ms = ms;
                let reports = self.reporter.position(ms, Instant::now());
                self.report(reports);
                self.sync_next();
            }
            PlayerEvent::TrackStarted(t) => {
                let entry = if self
                    .enqueued
                    .as_ref()
                    .is_some_and(|(_, rk)| *rk == t.rating_key)
                {
                    self.enqueued.take()
                } else if self
                    .loading
                    .as_ref()
                    .is_some_and(|(_, rk)| *rk == t.rating_key)
                {
                    self.loading.take()
                } else {
                    None
                };
                if let Some(i) = entry.and_then(|(id, _)| self.queue.position(id)) {
                    self.queue.current = Some(i);
                }
                let reports = self.reporter.started(&t, Instant::now());
                self.report(reports);
                self.media_controls_dirty = true;
                self.now.track = Some(t);
                self.now.position_ms = 0;
                self.now.buffering = false;
                self.failures = 0;
                if let Some(ms) = self.seek_on_start.take() {
                    self.seek_to(ms);
                }
                self.sync_next();
            }
            PlayerEvent::TrackEnded => {
                let reports = self.reporter.ended();
                self.report(reports);
                self.media_controls_dirty = true;
                // a pre-opened successor is about to report TrackStarted
                if self.enqueued.is_some() {
                    return;
                }
                match self.queue.next_index(true) {
                    Some(i) => self.play_index(i),
                    None => {
                        self.now.track = None;
                        self.now.position_ms = 0;
                    }
                }
            }
            PlayerEvent::Error {
                rating_key,
                message,
            } => {
                self.error(message);
                let Some(rating_key) = rating_key else { return };
                if self
                    .loading
                    .as_ref()
                    .is_some_and(|(_, rk)| *rk == rating_key)
                {
                    self.loading = None;
                    self.failures += 1;
                    match self.queue.next_index(false) {
                        Some(i) if self.failures < self.queue.len() => self.play_index(i),
                        _ => self.stop(),
                    }
                } else if self
                    .enqueued
                    .as_ref()
                    .is_some_and(|(_, rk)| *rk == rating_key)
                {
                    self.enqueued = None;
                } else if self
                    .now
                    .track
                    .as_ref()
                    .is_some_and(|t| t.rating_key == rating_key)
                {
                    // the track is cut short; its coming TrackEnded must not count as a full play
                    let reports = self.reporter.stopped();
                    self.report(reports);
                }
            }
            PlayerEvent::BufferingChanged(b) => self.now.buffering = b,
            PlayerEvent::DeviceVolume(v) => self.now.device_volume = Some(v),
            PlayerEvent::Paused | PlayerEvent::Resumed => {
                let paused = matches!(ev, PlayerEvent::Paused);
                if self.now.track.is_some() && self.now.paused != paused {
                    self.now.paused = paused;
                    let reports = self.reporter.set_paused(paused, Instant::now());
                    self.report(reports);
                    self.media_controls_dirty = true;
                }
            }
        }
    }

    pub fn on_media_control(&mut self, ev: MediaControlEvent) {
        let playing = self.now.track.is_some() && !self.now.paused;
        match ev {
            MediaControlEvent::Play if !playing => self.toggle_pause(),
            MediaControlEvent::Pause if playing => self.toggle_pause(),
            MediaControlEvent::Toggle => self.toggle_pause(),
            MediaControlEvent::Next => self.skip(),
            MediaControlEvent::Previous => self.previous(),
            MediaControlEvent::Stop => self.stop(),
            MediaControlEvent::Seek(dir) => self.seek_by(dir == SeekDirection::Forward),
            MediaControlEvent::SeekBy(dir, by) => {
                let by = by.as_millis() as u64;
                let pos = self.now.position_ms;
                self.seek_to(match dir {
                    SeekDirection::Forward => pos.saturating_add(by),
                    SeekDirection::Backward => pos.saturating_sub(by),
                });
            }
            MediaControlEvent::SetPosition(p) => self.seek_to(p.0.as_millis() as u64),
            MediaControlEvent::SetVolume(v) => self.set_volume(v as f32),
            _ => {}
        }
    }

    pub fn add_tracks(&mut self, mode: AddMode, tracks: Vec<Arc<Track>>) {
        let n = tracks.len();
        match mode {
            AddMode::Append => self.queue.append(tracks),
            AddMode::Next => self.queue.play_next(tracks),
            AddMode::PlayFromStart => {
                self.queue.replace(tracks, Some(0));
                return self.play_index(0);
            }
            AddMode::PlayShuffled => {
                self.queue.shuffle = true;
                self.queue.replace(tracks, None);
                return self.play_index(0);
            }
            AddMode::PlayFrom(rk) => {
                let start = tracks.iter().position(|t| t.rating_key == rk).unwrap_or(0);
                return self.play_tracks(tracks, start);
            }
        }
        let what = if n == 1 {
            "track".to_owned()
        } else {
            format!("{n} tracks")
        };
        let verb = if matches!(mode, AddMode::Next) {
            "playing next:"
        } else {
            "added"
        };
        self.info(format!("{verb} {what}"));
        self.sync_next();
    }

    pub fn add_source(&mut self, source: TrackSource, mode: AddMode) {
        if self
            .api
            .try_send(ApiRequest::Tracks { source, mode })
            .is_err()
        {
            self.error("busy, try again".into());
        }
    }

    /// queues the whole album of `track` and starts at it
    pub fn play_from_album(&mut self, track: Arc<Track>) {
        let start = track.rating_key.clone();
        if track.parent_rating_key.is_empty() {
            return self.add_tracks(AddMode::PlayFrom(start), vec![track]);
        }
        self.add_source(
            TrackSource::Album(track.parent_rating_key.clone()),
            AddMode::PlayFrom(start),
        );
    }

    pub fn remove_at(&mut self, i: usize) {
        if self.queue.remove(i) && self.now.track.is_some() {
            if i < self.queue.len() {
                self.play_index(i);
            } else {
                self.stop();
            }
        }
        self.sync_next();
    }

    pub fn move_at(&mut self, i: usize, down: bool) -> usize {
        let j = self.queue.move_entry(i, down).unwrap_or(i);
        self.sync_next();
        j
    }

    pub fn clear_queue(&mut self) {
        self.queue.clear();
        self.stop();
    }

    pub fn sync_cover(&mut self) {
        let Some(t) = &self.now.track else { return };
        if t.thumb.is_empty() || self.cover.as_ref().is_some_and(|c| c.thumb == t.thumb) {
            return;
        }
        let thumb = t.thumb.clone();
        self.cover = Some(Cover {
            thumb: thumb.clone(),
            art: None,
        });
        if self.api.try_send(ApiRequest::Cover { thumb }).is_err() {
            warn!("cover request dropped, API queue full");
            self.cover = None;
        }
    }

    pub fn on_cover(&mut self, thumb: String, result: Result<RgbImage, String>) {
        let Some(c) = self.cover.as_mut().filter(|c| c.thumb == thumb) else {
            return;
        };
        match result {
            Ok(img) => {
                c.art = Some(img);
                self.media_controls_dirty = true;
            }
            Err(e) => warn!("cover art: {e}"),
        }
    }
}
