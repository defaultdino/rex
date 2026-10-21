use std::time::{Duration, Instant};

use cast_sender::AppId;
use cast_sender::namespace::connection::Connection;
use cast_sender::namespace::heartbeat::Heartbeat;
use cast_sender::namespace::receiver::Receiver as Platform;
use tracing::warn;

use super::{Caster, Conn};
use crate::backend::PlayerEvent;

const PING_EVERY: Duration = Duration::from_secs(5);
const DEAD_AFTER_SILENCE: Duration = Duration::from_secs(15);
const POSITION_EVERY: Duration = Duration::from_millis(250);
const ADVANCE_TIMEOUT: Duration = Duration::from_secs(5);

impl Caster {
    pub fn connected(&mut self, now: Instant) {
        self.conn = Conn::Up {
            app: None,
            last_heard: now,
            last_ping: now,
        };
        self.pending_volume_req = None;
        self.send_platform(Connection::Connect, false);
        self.send_platform(Platform::launch_request(AppId::DefaultMediaReceiver), true);
    }

    pub(super) fn disconnect(&mut self) {
        if let Conn::Up { .. } = self.conn {
            self.send_platform(Connection::Close, false);
        }
        self.conn = Conn::Down;
    }

    pub fn lost(&mut self, message: &str, now: Instant) {
        warn!("{message}");
        self.conn = Conn::Down;
        self.detach(message, now);
    }

    pub(super) fn detach(&mut self, message: &str, now: Instant) {
        self.freeze(now);
        self.clear_in_flight();
        let Some(c) = self.current.as_mut() else {
            return;
        };
        c.media_session = None;
        c.queue_item = None;
        c.pending_load = None;
        self.fail(None, message.into());
        if !self.paused {
            self.paused = true;
            self.emit(PlayerEvent::Paused);
        }
    }

    pub fn shutdown(&mut self) {
        if let Conn::Up { app: Some(app), .. } = &self.conn {
            let session = app.session_id.clone();
            self.send_platform(Platform::stop_request(session), true);
        }
        self.disconnect();
    }

    pub fn wake_at(&self) -> Option<Instant> {
        let ping = match &self.conn {
            Conn::Up { last_ping, .. } => Some(*last_ping + PING_EVERY),
            _ => None,
        };
        let position = self
            .position
            .playing_since
            .map(|_| self.position_emitted + POSITION_EVERY);
        let ended = self.awaiting_advance.map(|t| t + ADVANCE_TIMEOUT);
        [ping, position, ended].into_iter().flatten().min()
    }

    pub fn tick(&mut self, now: Instant) {
        if let Conn::Up {
            last_heard,
            last_ping,
            ..
        } = &mut self.conn
            && now >= *last_ping + PING_EVERY
        {
            if now.duration_since(*last_heard) >= DEAD_AFTER_SILENCE {
                return self.lost("the cast device stopped answering", now);
            }
            *last_ping = now;
            self.send_platform(Heartbeat::Ping, false);
        }
        if self
            .awaiting_advance
            .is_some_and(|t| now >= t + ADVANCE_TIMEOUT)
        {
            warn!("cast device did not move on to the queued track");
            self.advance_manually();
        }
        if self.position.playing_since.is_some() && now >= self.position_emitted + POSITION_EVERY {
            self.position_emitted = now;
            self.emit(PlayerEvent::Position(self.position_at(now)));
        }
    }
}
