use std::net::IpAddr;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender};
use std::thread;
use std::time::Duration;

use super::keys::Action;
use super::state::{AppState, Modal};
use super::{AppEvent, forward};
use crate::backend::PlayerCmd;
use crate::cast::Device;
use crate::plex::PlexClient;

const DISCOVER_WAIT: Duration = Duration::from_millis(1500);
const STOP_WAIT: Duration = Duration::from_secs(2);

pub enum CastEvent {
    Found(Device),
    Searched(Result<(), String>),
    Connected {
        id: u64,
        name: String,
        result: Result<(SyncSender<PlayerCmd>, Receiver<()>), String>,
    },
}

#[derive(Debug)]
pub enum Search {
    Running,
    Done,
    Failed(String),
}

/// row 0 is this computer, row `i + 1` is `devices[i]`
#[derive(Debug)]
pub struct Picker {
    pub devices: Vec<Device>,
    pub search: Search,
    pub selected: usize,
}

pub struct CastState {
    events: SyncSender<AppEvent>,
    plex: Arc<PlexClient>,
    local: SyncSender<PlayerCmd>,
    /// disconnects once the cast thread has ended
    session: Option<Receiver<()>>,
    request: u64,
}

impl CastState {
    pub fn new(
        events: SyncSender<AppEvent>,
        plex: Arc<PlexClient>,
        local: SyncSender<PlayerCmd>,
    ) -> Self {
        Self {
            events,
            plex,
            local,
            session: None,
            request: 0,
        }
    }
}

impl AppState {
    pub(super) fn open_cast_picker(&mut self) {
        if !self.static_cast_devices.is_empty() {
            // static device definitions, no mDNS
            self.modal = Some(Modal::Cast(Picker {
                devices: self.static_cast_devices.clone(),
                search: Search::Done,
                selected: 0,
            }))
        } else {
            let events = self.cast.events.clone();
            let spawned = thread::Builder::new()
                .name("cast-discover".into())
                .spawn(move || {
                    let send = |ev| events.send(AppEvent::Cast(ev)).is_ok();
                    let result = crate::cast::discover(DISCOVER_WAIT, |d| {
                        send(CastEvent::Found(d));
                    });
                    send(CastEvent::Searched(
                        result.map_err(|e| format!("finding cast devices: {e}")),
                    ));
                });
            if let Err(e) = spawned {
                return self.error(format!("can't look for cast devices: {e}"));
            }

            self.modal = Some(Modal::Cast(Picker {
                devices: Vec::new(),
                search: Search::Running,
                selected: 0,
            }));
        }
    }

    pub(super) fn on_cast_key(&mut self, action: Action) {
        let Some(Modal::Cast(p)) = &mut self.modal else {
            return;
        };
        match action {
            Action::Down => p.selected = (p.selected + 1).min(p.devices.len()),
            Action::Up => p.selected = p.selected.saturating_sub(1),
            Action::Open => {
                let device = p.selected.checked_sub(1).and_then(|i| p.devices.get(i));
                let target = device.map(|d| (d.name.clone(), d.ip));
                self.modal = None;
                match target {
                    Some((name, ip)) => self.cast_to(name, ip),
                    None => self.play_locally(),
                }
            }
            _ => self.modal = None,
        }
    }

    pub fn on_cast_event(&mut self, ev: CastEvent) {
        let picker = match &mut self.modal {
            Some(Modal::Cast(p)) => Some(p),
            _ => None,
        };
        match ev {
            CastEvent::Found(d) => {
                if let Some(p) = picker
                    && !p.devices.iter().any(|x| x.ip == d.ip)
                {
                    let at = p.devices.partition_point(|x| x.name < d.name);
                    p.devices.insert(at, d);
                    if p.selected > at {
                        p.selected += 1;
                    }
                }
            }
            CastEvent::Searched(result) => {
                if let Some(p) = picker {
                    p.search = result.map_or_else(Search::Failed, |()| Search::Done);
                }
            }
            CastEvent::Connected { id, name, result } => self.on_connected(id, name, result),
        }
    }

    fn cast_to(&mut self, name: String, ip: IpAddr) {
        self.cast.request += 1;
        let id = self.cast.request;
        let (events, plex) = (self.cast.events.clone(), self.cast.plex.clone());
        let label = name.clone();
        let spawned = thread::Builder::new()
            .name("cast-connect".into())
            .spawn(move || {
                let (result, rx) = match crate::cast::spawn(plex, ip) {
                    Ok(((player, rx), done)) => (Ok((player, done)), Some(rx)),
                    Err(e) => (Err(format!("{e:#}")), None),
                };
                // the app drops a session's events until it has switched to it,
                // so they must not overtake Connected
                let _ = events.send(AppEvent::Cast(CastEvent::Connected { id, name, result }));
                if let Some(rx) = rx {
                    forward(rx, events, id);
                }
            });
        match spawned {
            Ok(_) => self.info(format!("connecting to {label}…")),
            Err(e) => self.error(format!("can't cast to {label}: {e}")),
        }
    }

    fn on_connected(
        &mut self,
        id: u64,
        name: String,
        result: Result<(SyncSender<PlayerCmd>, Receiver<()>), String>,
    ) {
        if id != self.cast.request {
            return;
        }
        match result {
            Ok((player, done)) => {
                self.playback.switch_player(player, true);
                self.cast.session = Some(done);
                self.backend = id;
                self.info(format!("casting to {name}"));
            }
            Err(e) => self.error(format!("can't cast to {name}: {e}")),
        }
    }

    fn play_locally(&mut self) {
        self.cast.request += 1;
        if self.cast.session.take().is_none() {
            return;
        }
        self.playback.switch_player(self.cast.local.clone(), false);
        self.backend = 0;
        self.info("playing on this computer".into());
    }

    pub(super) fn close_cast(&mut self) -> Option<Receiver<()>> {
        let done = self.cast.session.take()?;
        self.playback.set_player(self.cast.local.clone());
        Some(done)
    }
}

pub(super) fn wait_closed(done: Option<Receiver<()>>) {
    if let Some(done) = done {
        let _ = done.recv_timeout(STOP_WAIT);
    }
}
