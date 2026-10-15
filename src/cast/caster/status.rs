use std::time::Instant;

use cast_sender::namespace::media::{
    IdleReason, Media, MediaInformation, MediaStatus, PlayerState,
};

use super::media::request;
use super::{Caster, Current, Position, TAKEN_OVER};
use crate::backend::PlayerEvent;

impl Caster {
    pub(super) fn on_status(&mut self, st: MediaStatus, req: u32, now: Instant) {
        let Some(c) = self.current.as_mut() else {
            if self.abandoned_load == Some(req) {
                self.abandoned_load = None;
                self.send_media(Media::Stop(request(st.media_session_id)));
            }
            return;
        };
        if let Some(load) = c.pending_load {
            if load != req {
                return;
            }
            c.pending_load = None;
            c.media_session = Some(st.media_session_id);
            c.queue_item = st.current_item_id;
            let first = !std::mem::replace(&mut c.announced, true);
            let track = c.track.clone();
            if first {
                self.emit(PlayerEvent::TrackStarted(track));
            }
            self.resync_after_load(&st);
        } else if c.media_session != Some(st.media_session_id) {
            if c.media_session.is_some() && !matches!(st.player_state, PlayerState::Idle) {
                self.detach(TAKEN_OVER, now);
            }
            return;
        }
        if let Some(c) = self.current.as_mut()
            && c.queue_item.is_none()
            && self.queued.is_none()
        {
            c.queue_item = st.current_item_id;
        }
        self.learn_queued_item(&st);

        if let PlayerState::Idle = st.player_state {
            return self.on_idle(st.idle_reason, now);
        }
        if let Some(item) = st.current_item_id
            && self
                .current
                .as_ref()
                .is_some_and(|c| c.queue_item.is_some_and(|i| i != item))
            && !self.follow_device_advance(item, st.media.as_ref(), now)
        {
            return;
        }

        let at = (st.current_time * 1000.0) as u64;
        let settled = self.pending_toggle.is_none_or(|t| req >= t);
        self.set_buffering(matches!(st.player_state, PlayerState::Buffering));
        match st.player_state {
            PlayerState::Playing => {
                self.position = Position {
                    ms: at,
                    playing_since: Some(now),
                };
                if settled && self.paused {
                    self.paused = false;
                    self.emit(PlayerEvent::Resumed);
                }
                self.position_emitted = now;
                self.emit(PlayerEvent::Position(self.position_at(now)));
            }
            PlayerState::Paused => {
                self.position = Position::still(at);
                if settled && !self.paused {
                    self.paused = true;
                    self.emit(PlayerEvent::Paused);
                }
            }
            _ => self.position = Position::still(at),
        }
    }

    pub(super) fn resync_after_load(&mut self, st: &MediaStatus) {
        let Some(id) = self.media_session() else {
            return;
        };
        self.sync_stream_volume();
        let playing = matches!(
            st.player_state,
            PlayerState::Playing | PlayerState::Buffering
        );
        if self.paused && playing {
            self.toggle(Media::Pause(request(id)));
        } else if !self.paused && matches!(st.player_state, PlayerState::Paused) {
            self.toggle(Media::Play(request(id)));
        }
        if let Some(ms) = self.seek_on_load.take() {
            self.seek(id, ms);
        }
        self.sync_queue();
    }

    pub(super) fn learn_queued_item(&mut self, st: &MediaStatus) {
        let (Some(q), Some(items)) = (&mut self.queued, &st.items) else {
            return;
        };
        if q.queue_item.is_some() {
            return;
        }
        let current = self.current.as_ref().and_then(|c| c.queue_item);
        let mut others = items
            .iter()
            .rev()
            .filter(|i| i.item_id.is_some() && i.item_id != current);
        let by_url = others.clone().find(|i| {
            i.media
                .as_ref()
                .is_some_and(|m| m.content_id == q.content_url)
        });
        q.queue_item = by_url.or_else(|| others.next()).and_then(|i| i.item_id);
        if q.queue_item.is_some() {
            self.sync_queue();
        }
    }

    pub(super) fn on_idle(&mut self, reason: Option<IdleReason>, now: Instant) {
        match reason {
            Some(IdleReason::Finished)
                if self.queued.as_ref().is_some_and(|q| q.queue_item.is_some()) =>
            {
                self.awaiting_advance.get_or_insert(now);
            }
            Some(IdleReason::Finished) => self.advance_manually(),
            Some(IdleReason::Error) => {
                if let Some(c) = &self.current {
                    let msg = format!("{}: the cast device could not play it", c.track.title);
                    self.fail(Some(&c.track.rating_key), msg);
                }
                self.advance_manually();
            }
            Some(IdleReason::Cancelled | IdleReason::Interrupted) => {
                self.detach("playback was stopped from elsewhere", now)
            }
            None => {}
        }
    }

    pub(super) fn advance_manually(&mut self) {
        self.awaiting_advance = None;
        self.queued = None;
        self.position = Position::still(0);
        self.set_buffering(false);
        if self.current.take().is_none() {
            return;
        }
        self.emit(PlayerEvent::TrackEnded);
        if let Some(next) = self.wanted_next.take() {
            self.start(next);
        }
    }

    pub(super) fn follow_device_advance(
        &mut self,
        item: i32,
        media: Option<&MediaInformation>,
        now: Instant,
    ) -> bool {
        let ours = self.queued.as_ref().is_some_and(|q| {
            q.queue_item == Some(item)
                || (q.queue_item.is_none() && media.is_none_or(|m| m.content_id == q.content_url))
        });
        let (Some(q), Some(c)) = (self.queued.take().filter(|_| ours), self.current.take()) else {
            self.detach(TAKEN_OVER, now);
            return false;
        };
        self.awaiting_advance = None;
        self.position = Position::still(0);
        self.emit(PlayerEvent::TrackEnded);
        if self
            .wanted_next
            .as_ref()
            .is_some_and(|n| n.rating_key == q.track.rating_key)
        {
            self.wanted_next = None;
            self.queue_refused = false;
            self.current = Some(Current {
                track: q.track.clone(),
                media_session: c.media_session,
                queue_item: Some(item),
                pending_load: None,
                announced: true,
            });
            self.emit(PlayerEvent::TrackStarted(q.track));
            return true;
        }
        match self.wanted_next.take() {
            Some(next) => self.start(next),
            None => {
                if let Some(id) = c.media_session {
                    self.send_media(Media::Stop(request(id)));
                }
            }
        }
        false
    }
}
