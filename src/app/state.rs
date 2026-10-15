use std::sync::mpsc::SyncSender;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Color;

use super::browser::{Browser, ListKind, Selected, View};
use super::keys::{
    self, Action, GLOBAL_PLAYER_KEYS, HELP_KEYS, LIST_KEYS, MAXIMIZED_PLAYER_KEYS, NAV_KEYS,
    QUEUE_KEYS, SEARCH_INPUT_KEYS,
};
use super::playback::Playback;
use super::queue::SavedQueue;
use super::{AddMode, ApiRequest, TrackSource};
use crate::backend::PlayerCmd;

const MESSAGE_TTL: Duration = Duration::from_secs(5);
const QUIT_CONFIRM: Duration = Duration::from_secs(3);
#[cfg(feature = "cast")]
const CAST_STOP_WAIT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    List,
}

/// overlay that takes all input until closed
#[derive(Debug, PartialEq, Eq)]
pub enum Modal {
    Search(String),
    Help,
    #[cfg(feature = "cast")]
    Cast {
        devices: Option<Result<Vec<crate::cast::Device>, String>>,
        selected: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Browse(Focus),
    Player,
}

pub struct Message {
    pub text: String,
    pub at: Instant,
    pub error: bool,
}

impl Message {
    pub fn error(text: String) -> Self {
        Self {
            text,
            at: Instant::now(),
            error: true,
        }
    }

    pub fn info(text: String) -> Self {
        Self {
            text,
            at: Instant::now(),
            error: false,
        }
    }
}

pub struct AppState {
    pub screen: Screen,
    pub modal: Option<Modal>,
    pub browser: Browser,
    pub playback: Playback,
    quit_armed: Option<Instant>,
    /// new server address found by rediscovery, saved to the state on exit
    pub server_url: Option<String>,
    pub debug_line: bool,
    /// colour for focus, selection and highlights, from `accent_color` in the config
    pub accent: Color,
    pub message: Option<Message>,
    pub quit: bool,
    pub backend: u64,
    #[cfg(feature = "cast")]
    api: SyncSender<ApiRequest>,
    #[cfg(feature = "cast")]
    local_player: SyncSender<PlayerCmd>,
    #[cfg(feature = "cast")]
    cast: Option<std::thread::JoinHandle<()>>,
    #[cfg(feature = "cast")]
    cast_request: u64,
}

impl AppState {
    pub fn new(
        api: SyncSender<ApiRequest>,
        player: SyncSender<PlayerCmd>,
        volume: f32,
        saved: Option<SavedQueue>,
    ) -> Self {
        Self {
            screen: Screen::Browse(Focus::List),
            modal: None,
            browser: Browser::new(api.clone()),
            #[cfg(feature = "cast")]
            api: api.clone(),
            #[cfg(feature = "cast")]
            local_player: player.clone(),
            #[cfg(feature = "cast")]
            cast: None,
            #[cfg(feature = "cast")]
            cast_request: 0,
            backend: 0,
            playback: Playback::new(api, player, volume, saved),
            quit_armed: None,
            server_url: None,
            debug_line: false,
            accent: Color::Cyan,
            message: None,
            quit: false,
        }
    }

    /// applies `accent_color` from the config, keeping the default and saying so when it won't parse
    pub fn set_accent(&mut self, name: Option<&str>) {
        let Some(name) = name else { return };
        match name.parse() {
            Ok(c) => self.accent = c,
            Err(_) => self.error(format!("unknown accent_color {name:?}, using cyan")),
        }
    }

    pub fn error(&mut self, text: String) {
        tracing::warn!("{text}");
        self.message = Some(Message::error(text));
    }

    pub fn info(&mut self, text: String) {
        self.message = Some(Message::info(text));
    }

    /// how long until the transient message should disappear
    pub fn next_deadline(&self) -> Option<Duration> {
        self.message
            .as_ref()
            .map(|m| MESSAGE_TTL.saturating_sub(m.at.elapsed()))
    }

    pub fn expire_message(&mut self) {
        if self.next_deadline().is_some_and(|d| d.is_zero()) {
            self.message = None;
        }
    }

    /// number of rows in the current list (the queue view renders the queue itself)
    fn list_len(&self) -> usize {
        match self.browser.view() {
            Some(v) if matches!(v.kind, ListKind::Queue) => self.playback.queue.len(),
            Some(v) => v.items.len(),
            None => 0,
        }
    }

    pub fn get_supported_actions(&self) -> Vec<Action> {
        let groups: &[&[Action]] = match (&self.modal, self.screen) {
            (Some(Modal::Search(_)), _) => &[SEARCH_INPUT_KEYS],
            (Some(Modal::Help), _) => &[HELP_KEYS],
            #[cfg(feature = "cast")]
            (Some(Modal::Cast { .. }), _) => &[keys::CAST_KEYS],
            (None, Screen::Player) => &[MAXIMIZED_PLAYER_KEYS, GLOBAL_PLAYER_KEYS],
            (None, Screen::Browse(Focus::Sidebar)) => &[NAV_KEYS, GLOBAL_PLAYER_KEYS],
            (None, Screen::Browse(Focus::List)) => match self.browser.view().map(|v| &v.kind) {
                Some(ListKind::Queue) => &[QUEUE_KEYS, NAV_KEYS, GLOBAL_PLAYER_KEYS],
                Some(_) => &[LIST_KEYS, NAV_KEYS, GLOBAL_PLAYER_KEYS],
                None => &[],
            },
        };
        groups.concat()
    }

    pub fn on_key(&mut self, k: KeyEvent) {
        if matches!(self.modal, Some(Modal::Search(_))) {
            self.on_search_key(k);
        } else if let Some(action) = keys::lookup(k) {
            self.on_action(action);
        }
    }

    fn on_action(&mut self, action: Action) {
        if !self.get_supported_actions().contains(&action) {
            return;
        }
        // every key the help modal accepts closes it
        if self.modal == Some(Modal::Help) {
            self.modal = None;
            return;
        }
        #[cfg(feature = "cast")]
        if let Some(Modal::Cast { devices, selected }) = &mut self.modal {
            let found = devices.as_ref().and_then(|d| d.as_ref().ok());
            let len = found.map_or(0, Vec::len);
            match action {
                Action::Down => *selected = (*selected + 1).min(len),
                Action::Up => *selected = selected.saturating_sub(1),
                Action::Open => {
                    let device = selected.checked_sub(1).and_then(|i| found?.get(i));
                    let target = device.map(|d| (d.name.clone(), d.ip));
                    let local = *selected == 0;
                    self.modal = None;
                    match target {
                        Some((name, ip)) => self.cast_to(name, ip),
                        None if local => self.play_locally(),
                        None => {}
                    }
                }
                _ => self.modal = None,
            }
            return;
        }

        let half = (self.browser.list_height / 2).max(1) as isize;
        match action {
            Action::Down => self.move_by(1),
            Action::Up => self.move_by(-1),
            Action::Top => self.move_by(isize::MIN / 2),
            Action::Bottom => self.move_by(isize::MAX / 2),
            Action::HalfDown => self.move_by(half),
            Action::HalfUp => self.move_by(-half),
            Action::Open => self.open(),
            Action::Back => self.back(),
            Action::ToggleFocus => {
                if let Screen::Browse(f) = &mut self.screen {
                    *f = match f {
                        Focus::Sidebar => Focus::List,
                        Focus::List => Focus::Sidebar,
                    };
                }
            }
            Action::Jump(section) => {
                self.browser.set_section(section);
                self.screen = Screen::Browse(Focus::List);
            }
            Action::PlayPause => self.playback.toggle_pause(),
            Action::Next => self.playback.skip(),
            Action::Previous => self.playback.previous(),
            Action::SeekForward => self.playback.seek_by(true),
            Action::SeekBack => self.playback.seek_by(false),
            Action::VolumeUp => self.playback.change_volume(true),
            Action::VolumeDown => self.playback.change_volume(false),
            Action::Shuffle => self.playback.toggle_shuffle(),
            Action::Repeat => self.playback.cycle_repeat(),
            Action::Append => self.add_selected(AddMode::Append),
            Action::PlayNext => self.add_selected(AddMode::Next),
            Action::Remove => self.queue_edit(|p, i| {
                p.remove_at(i);
                i
            }),
            Action::MoveDown => self.queue_edit(|p, i| p.move_at(i, true)),
            Action::MoveUp => self.queue_edit(|p, i| p.move_at(i, false)),
            Action::Clear => self.queue_edit(|p, _| {
                p.clear_queue();
                0
            }),
            Action::Search => {
                self.browser.show_search();
                self.screen = Screen::Browse(Focus::List);
                self.modal = Some(Modal::Search(String::new()));
            }
            Action::Help => self.modal = Some(Modal::Help),
            Action::DebugLine => self.debug_line = !self.debug_line,
            Action::Quit => self.request_quit(),
            Action::Play => self.add_selected(AddMode::PlayFromStart),
            Action::TogglePlayer => {
                self.screen = match self.screen {
                    Screen::Browse(_) => Screen::Player,
                    Screen::Player => Screen::Browse(Focus::List),
                }
            }
            // routed through on_search_key while the search modal is open, never via lookup
            Action::SearchSubmit | Action::SearchCancel => {}
            #[cfg(feature = "cast")]
            Action::Cast => {
                if self.api.try_send(ApiRequest::Discover).is_err() {
                    return self.error("can't look for cast devices, API queue full".into());
                }
                self.modal = Some(Modal::Cast {
                    devices: None,
                    selected: 0,
                });
            }
        }
    }

    #[cfg(feature = "cast")]
    pub fn on_devices(&mut self, result: Result<Vec<crate::cast::Device>, String>) {
        if let Some(Modal::Cast { devices, selected }) = &mut self.modal {
            *devices = Some(result);
            *selected = 0;
        }
    }

    #[cfg(feature = "cast")]
    fn cast_to(&mut self, name: String, ip: std::net::IpAddr) {
        self.cast_request += 1;
        let req = ApiRequest::Cast {
            id: self.cast_request,
            volume: self.playback.now.volume,
            name: name.clone(),
            ip,
        };
        if self.api.try_send(req).is_err() {
            return self.error("can't start casting, API queue full".into());
        }
        self.info(format!("connecting to {name}…"));
    }

    #[cfg(feature = "cast")]
    pub fn on_cast(
        &mut self,
        id: u64,
        name: String,
        result: Result<(SyncSender<PlayerCmd>, std::thread::JoinHandle<()>), String>,
    ) {
        if id != self.cast_request {
            return;
        }
        match result {
            Ok((player, cast)) => {
                drop(self.playback.switch_player(player));
                self.cast = Some(cast);
                self.backend = id;
                self.info(format!("casting to {name}"));
            }
            Err(e) => self.error(format!("can't cast to {name}: {e}")),
        }
    }

    #[cfg(feature = "cast")]
    fn play_locally(&mut self) {
        self.cast_request += 1;
        if self.cast.take().is_none() {
            return;
        }
        drop(self.playback.switch_player(self.local_player.clone()));
        self.backend = 0;
        self.info("playing on this computer".into());
    }

    #[cfg(feature = "cast")]
    pub fn release_cast(&mut self) {
        let Some(cast) = self.cast.take() else { return };
        drop(self.playback.replace_player(self.local_player.clone()));
        let deadline = Instant::now() + CAST_STOP_WAIT;
        while !cast.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn move_by(&mut self, delta: isize) {
        match self.screen {
            Screen::Browse(Focus::Sidebar) => self.browser.move_section(delta),
            Screen::Browse(Focus::List) => {
                let len = self.list_len();
                self.browser.move_selection(delta, len);
            }
            Screen::Player => {}
        }
    }

    fn open(&mut self) {
        if self.screen == Screen::Browse(Focus::Sidebar) {
            self.screen = Screen::Browse(Focus::List);
            return;
        }
        let Some(v) = self.browser.view() else { return };
        if matches!(v.kind, ListKind::Queue) {
            return self.playback.play_index(v.selected);
        }
        let (kind, title) = match v.selected() {
            Some(Selected::Artist(a)) => (
                ListKind::ArtistAlbums(a.rating_key.clone()),
                a.title.clone(),
            ),
            Some(Selected::Album(a)) => {
                let title = if a.parent_title.is_empty() {
                    a.title.clone()
                } else {
                    format!("{} — {}", a.title, a.parent_title)
                };
                (ListKind::AlbumTracks(a.rating_key.clone()), title)
            }
            Some(Selected::Playlist(p)) => (
                ListKind::PlaylistTracks(p.rating_key.clone()),
                p.title.clone(),
            ),
            Some(Selected::Track(_, Some(list))) => {
                return self.playback.play_tracks(list.to_vec(), v.selected);
            }
            Some(Selected::Track(t, None)) => return self.playback.play_from_album(t.clone()),
            None => return,
        };
        self.browser.push(kind, title);
    }

    /// `a` / `n`: queues the selected track directly, or fetches every track of an album, artist or playlist
    fn add_selected(&mut self, mode: AddMode) {
        let source = match self.browser.view().and_then(View::selected) {
            Some(Selected::Track(t, _)) => return self.playback.add_tracks(mode, vec![t.clone()]),
            Some(Selected::Album(a)) => TrackSource::Album(a.rating_key.clone()),
            Some(Selected::Artist(a)) => TrackSource::Artist(a.rating_key.clone()),
            Some(Selected::Playlist(p)) => TrackSource::Playlist(p.rating_key.clone()),
            None => return,
        };
        self.playback.add_source(source, mode);
    }

    /// applies `f` to the selected queue entry; `f` returns the index to select afterwards
    fn queue_edit(&mut self, f: impl FnOnce(&mut Playback, usize) -> usize) {
        let Some(v) = self.browser.view_mut() else {
            return;
        };
        let i = f(&mut self.playback, v.selected);
        v.selected = i.min(self.playback.queue.len().saturating_sub(1));
    }

    fn back(&mut self) {
        if self.screen == Screen::Browse(Focus::List) && self.browser.stack.len() > 1 {
            self.browser.stack.pop();
        } else {
            self.screen = Screen::Browse(Focus::Sidebar);
        }
    }

    /// quits right away unless something is playing, then asks for a second `q`
    fn request_quit(&mut self) {
        let now = &self.playback.now;
        let playing = now.track.is_some() && !now.paused;
        if !playing || self.quit_armed.is_some_and(|t| t.elapsed() < QUIT_CONFIRM) {
            self.quit = true;
        } else {
            self.quit_armed = Some(Instant::now());
            self.info("press q again to quit".into());
        }
    }

    /// keys typed while the search modal is open
    fn on_search_key(&mut self, k: KeyEvent) {
        let Some(Modal::Search(input)) = &mut self.modal else {
            return;
        };
        match k.code {
            KeyCode::Esc => self.modal = None,
            KeyCode::Enter => {
                let query = input.trim().to_owned();
                self.modal = None;
                if !query.is_empty()
                    && let Err(e) = self.browser.search(query)
                {
                    self.error(e);
                }
            }
            KeyCode::Backspace => {
                input.pop();
            }
            KeyCode::Char('u') if k.modifiers.contains(KeyModifiers::CONTROL) => input.clear(),
            KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => input.push(c),
            _ => {}
        }
    }
}
