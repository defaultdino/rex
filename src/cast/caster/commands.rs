use std::sync::Arc;
use std::time::Instant;

use cast_sender::Volume;
use cast_sender::namespace::media::{
    LoadRequestData, Media, QueueInsertRequestData, QueueItem, QueueRemoveRequestData,
    SeekRequestData, VolumeRequestData,
};
use tracing::debug;

use super::media::{request, secs};
use super::{Caster, Conn, Current, Position, QueuedNext};
use crate::backend::{PlayerCmd, perceptual_gain};
use crate::plex::models::Track;

const PRELOAD_SECS: f64 = 20.0;

impl Caster {
    pub fn handle(&mut self, cmd: PlayerCmd, now: Instant) {
        debug!("cast cmd {cmd:?}");
        match cmd {
            PlayerCmd::Load(track) => self.start(track),
            PlayerCmd::EnqueueNext(track) => {
                self.wanted_next = Some(track);
                self.sync_queue();
            }
            PlayerCmd::ClearNext => {
                self.wanted_next = None;
                self.sync_queue();
            }
            PlayerCmd::Play => {
                self.paused = false;
                match self.media_session() {
                    Some(id) => self.toggle(Media::Play(request(id))),
                    None => self.ensure_loaded(),
                }
            }
            PlayerCmd::Pause => {
                self.freeze(now);
                self.paused = true;
                if let Some(id) = self.media_session() {
                    self.toggle(Media::Pause(request(id)));
                }
            }
            PlayerCmd::Seek(ms) => {
                self.position = Position {
                    ms,
                    playing_since: self.position.playing_since.and(Some(now)),
                };
                match self.media_session() {
                    Some(id) => self.seek(id, ms),
                    None if self
                        .current
                        .as_ref()
                        .is_some_and(|c| c.pending_load.is_some()) =>
                    {
                        self.seek_on_load = Some(ms)
                    }
                    None => {}
                }
            }
            PlayerCmd::Stop => {
                if let Some(id) = self.media_session() {
                    self.send_media(Media::Stop(request(id)));
                }
                self.abandoned_load = self.current.as_ref().and_then(|c| c.pending_load);
                self.current = None;
                self.wanted_next = None;
                self.position = Position::still(0);
                self.clear_in_flight();
            }
            PlayerCmd::SetVolume(v) => {
                self.volume = v;
                self.sync_stream_volume();
            }
        }
    }

    pub(super) fn start(&mut self, track: Arc<Track>) {
        self.current = Some(Current {
            track,
            media_session: None,
            queue_item: None,
            pending_load: None,
            announced: false,
        });
        self.wanted_next = None;
        self.queue_refused = false;
        self.paused = false;
        self.position = Position::still(0);
        self.clear_in_flight();
        self.ensure_loaded();
    }

    pub(super) fn clear_in_flight(&mut self) {
        self.queued = None;
        self.awaiting_advance = None;
        self.pending_toggle = None;
        self.seek_on_load = None;
        self.set_buffering(false);
    }

    pub(super) fn ensure_loaded(&mut self) {
        let Some(c) = &self.current else { return };
        if c.media_session.is_some() || c.pending_load.is_some() {
            return;
        }
        match &self.conn {
            Conn::Down => self.conn = Conn::Connecting,
            Conn::Connecting | Conn::Up { app: None, .. } => {}
            Conn::Up { app: Some(_), .. } => self.load(),
        }
    }

    pub(super) fn load(&mut self) {
        let Some(c) = &self.current else { return };
        let media = match self.media_info(&c.track) {
            Ok(m) => m,
            Err(why) => return self.reject_current(why),
        };
        let req = self.send_media(Media::Load(LoadRequestData {
            media,
            autoplay: Some(!self.paused),
            current_time: Some(secs(self.position.ms)),
            ..Default::default()
        }));
        if let Some(c) = self.current.as_mut() {
            c.pending_load = req;
        }
    }

    pub(super) fn toggle(&mut self, m: Media) {
        self.pending_toggle = self.send_media(m).or(self.pending_toggle);
    }

    pub(super) fn seek(&mut self, id: i32, ms: u64) {
        self.send_media(Media::Seek(SeekRequestData {
            media_session_id: Some(id),
            current_time: Some(secs(ms)),
            ..Default::default()
        }));
    }

    pub(super) fn sync_stream_volume(&mut self) {
        let Some(id) = self.media_session() else {
            return;
        };
        self.send_media(Media::SetVolume(VolumeRequestData {
            media_session_id: Some(id),
            volume: Volume {
                level: Some(perceptual_gain(self.volume) as f64),
                muted: Some(false),
                control_type: None,
            },
        }));
    }

    pub(super) fn sync_queue(&mut self) {
        let Some(id) = self.media_session() else {
            return;
        };
        if let Some(q) = &self.queued
            && self
                .wanted_next
                .as_ref()
                .is_none_or(|n| n.rating_key != q.track.rating_key)
        {
            let Some(item) = q.queue_item else { return };
            self.send_media(Media::QueueRemove(QueueRemoveRequestData {
                item_ids: vec![item],
                media_session_id: Some(id),
                ..Default::default()
            }));
            self.queued = None;
        }
        if self.queued.is_some() || self.queue_refused || self.awaiting_advance.is_some() {
            return;
        }
        let Some(next) = self.wanted_next.clone() else {
            return;
        };
        // fail it now like the local player does, so `Playback` skips it when the current one ends
        let media = match self.media_info(&next) {
            Ok(m) => m,
            Err(why) => {
                self.wanted_next = None;
                return self.fail(Some(&next.rating_key), format!("{}: {why}", next.title));
            }
        };
        let content_url = media.content_id.clone();
        let item = QueueItem {
            media: Some(media),
            autoplay: Some(true),
            preload_time: Some(PRELOAD_SECS),
            ..Default::default()
        };
        let req = self.send_media(Media::QueueInsert(QueueInsertRequestData {
            items: vec![item],
            media_session_id: Some(id),
            ..Default::default()
        }));
        if let Some(insert_req) = req {
            self.queued = Some(QueuedNext {
                track: next,
                content_url,
                insert_req,
                queue_item: None,
            });
        }
    }
}
