use std::time::Instant;

use cast_sender::namespace::connection::Connection;
use cast_sender::namespace::heartbeat::Heartbeat;
use cast_sender::namespace::media::{ErrorReason, Media};
use cast_sender::namespace::receiver::{Receiver as Platform, Status};
use cast_sender::{AppId, Payload, Response, Volume};
use tracing::{debug, warn};

use super::{Caster, Conn, MediaApp, PLATFORM, Position, TAKEN_OVER};
use crate::backend::{DeviceVolume, PlayerEvent};

impl Caster {
    pub fn receive(&mut self, msg: Response, now: Instant) {
        if let Conn::Up { last_heard, .. } = &mut self.conn {
            *last_heard = now;
        }
        let req = msg.request_id.unwrap_or(0);
        match msg.payload {
            Payload::Heartbeat(Heartbeat::Ping) => {
                self.outbox
                    .push((msg.source_id, Heartbeat::Pong.into(), None))
            }
            Payload::Connection(Connection::Close) => {
                if msg.source_id == PLATFORM {
                    self.lost("the cast device closed the connection", now);
                } else if self.is_app(&msg.source_id) {
                    self.receiver_gone(now);
                }
            }
            Payload::Receiver(Platform::ReceiverStatus(r)) => self.on_receiver(r.status, req, now),
            Payload::Receiver(Platform::LaunchError(e)) => {
                let msg = format!("the cast device could not start playing: {}", e.reason);
                self.disconnect();
                self.detach(&msg, now);
            }
            Payload::Media(Media::MediaStatus(s)) => {
                for st in s.status {
                    self.on_status(st, req, now);
                }
            }
            Payload::Media(Media::LoadFailed) => self.refused(req, None, now),
            Payload::Media(Media::LoadCancelled) => {
                if self
                    .current
                    .as_ref()
                    .is_some_and(|c| c.pending_load == Some(req))
                {
                    self.detach(TAKEN_OVER, now);
                }
            }
            Payload::Media(Media::InvalidRequest(e)) => self.refused(req, Some(e.reason), now),
            Payload::Media(Media::InvalidPlayerState) => debug!("cast: invalid player state"),
            _ => {}
        }
        if self.pending_toggle.is_some_and(|t| req >= t) {
            self.pending_toggle = None;
        }
    }

    pub(super) fn is_app(&self, transport: &str) -> bool {
        matches!(&self.conn, Conn::Up { app: Some(a), .. } if a.transport == transport)
    }

    pub(super) fn on_receiver(&mut self, status: Status, req: u32, now: Instant) {
        self.on_device_volume(&status.volume, req);
        let Conn::Up { app, .. } = &mut self.conn else {
            return;
        };
        let ours = status
            .applications
            .unwrap_or_default()
            .into_iter()
            .find(|a| a.app_id == AppId::DefaultMediaReceiver);
        match (app.as_ref(), ours) {
            (None, Some(a)) => {
                let transport = a.transport_id.clone();
                *app = Some(MediaApp {
                    transport: a.transport_id,
                    session_id: a.session_id,
                });
                self.outbox
                    .push((transport, Connection::Connect.into(), None));
                self.ensure_loaded();
            }
            (Some(old), new) if new.as_ref().is_none_or(|n| n.session_id != old.session_id) => {
                self.receiver_gone(now)
            }
            _ => {}
        }
    }

    fn on_device_volume(&mut self, v: &Volume, req: u32) {
        if self.pending_volume_req.is_some_and(|r| req < r) {
            return;
        }
        self.pending_volume_req = None;
        let Some(level) = v.level else { return };
        let d = DeviceVolume {
            level: level as f32,
            muted: v.muted == Some(true),
        };
        self.device_volume = Some(d);
        self.emit(PlayerEvent::DeviceVolume(d));
    }

    pub(super) fn receiver_gone(&mut self, now: Instant) {
        self.disconnect();
        self.detach("casting was stopped from elsewhere", now);
    }

    pub(super) fn refused(&mut self, req: u32, reason: Option<ErrorReason>, now: Instant) {
        let why = match &reason {
            Some(r) => format!("the cast device refused it ({r:?})"),
            None => "the cast device could not load it".into(),
        };
        if self
            .current
            .as_ref()
            .is_some_and(|c| c.pending_load == Some(req))
        {
            return self.reject_current(why);
        }
        if self.queued.as_ref().is_some_and(|q| q.insert_req == req) {
            debug!("cast: queueing the next track failed: {why}");
            self.queued = None;
            self.queue_refused = true;
            return;
        }
        if matches!(reason, Some(ErrorReason::InvalidMediaSessionId))
            && self.media_session().is_some()
        {
            return self.detach("the cast session ended", now);
        }
        warn!("cast: {why}");
    }

    pub(super) fn reject_current(&mut self, why: String) {
        let Some(c) = self.current.take() else { return };
        self.position = Position::still(0);
        self.fail(
            Some(&c.track.rating_key),
            format!("{}: {why}", c.track.title),
        );
    }
}
