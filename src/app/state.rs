use std::sync::mpsc::SyncSender;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::time::{Duration, Instant};

use super::keys::Action;
use super::queue::Queue;
use super::reporting::Reporter;
use super::{AddMode, ApiRequest, PageData, TrackSource};
use crate::audio::player::PlayerCmd;
use crate::plex::api::SearchResults;
use crate::plex::models::{Album, Artist, Playlist, Track};

const PREFETCH_MARGIN: usize = 50;
const MESSAGE_TTL: Duration = Duration::from_secs(5);

pub const SECTIONS: [&str; 5] = ["Artists", "Albums", "Playlists", "Search", "Queue"];

#[derive(Debug, Clone)]
pub enum ListKind {
    Artists,
    Albums,
    Playlists,
    ArtistAlbums(String),
    AlbumTracks(String),
    PlaylistTracks(String),
    Search,
    Queue,
}

pub enum Items {
    Artists(Vec<Artist>),
    Albums(Vec<Album>),
    Tracks(Vec<Track>),
    Playlists(Vec<Playlist>),
    Search(Vec<SearchItem>),
}

pub enum SearchItem {
    Header(String),
    Artist(Artist),
    Album(Album),
    Track(Track),
}

impl Items {
    pub fn len(&self) -> usize {
        match self {
            Items::Artists(v) => v.len(),
            Items::Albums(v) => v.len(),
            Items::Tracks(v) => v.len(),
            Items::Playlists(v) => v.len(),
            Items::Search(v) => v.len(),
        }
    }
}

pub struct View {
    pub id: u64,
    pub kind: ListKind,
    pub title: String,
    pub items: Items,
    pub total: Option<u32>,
    pub loading: bool,
    pub selected: usize,
    pub offset: usize,
}

impl View {
    fn fetchable(&self) -> bool {
        !matches!(self.kind, ListKind::Search | ListKind::Queue)
    }

    fn complete(&self) -> bool {
        !self.fetchable() || self.total.is_some_and(|t| self.items.len() >= t as usize)
    }
}

#[derive(Default)]
pub struct NowPlaying {
    pub track: Option<Track>,
    pub position_ms: u64,
    pub paused: bool,
    pub buffering: bool,
    pub volume: f32,
}

pub struct Message {
    pub text: String,
    pub at: Instant,
    pub error: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    List,
}

pub struct AppState {
    pub(super) api: SyncSender<ApiRequest>,
    pub(super) player: SyncSender<PlayerCmd>,
    next_id: u64,
    pub queue: Queue,
    pub now: NowPlaying,
    /// queue entry id and rating key of the last `Load` not yet confirmed by `TrackStarted`
    pub(super) loading: Option<(u64, String)>,
    /// queue entry id and rating key handed to the player as the gapless successor
    pub(super) enqueued: Option<(u64, String)>,
    pub(super) failures: usize,
    pub(super) quit_armed: Option<Instant>,
    pub(super) reporter: Reporter,
    /// now-playing changed in a way media widgets should hear about
    pub mpris_dirty: bool,
    /// new server address found by rediscovery, saved to the config on exit
    pub server_url: Option<String>,
    pub focus: Focus,
    pub section: usize,
    pub stack: Vec<View>,
    pub help: bool,
    pub debug_line: bool,
    pub message: Option<Message>,
    pub list_height: usize,
    /// text being typed after `/`, `Some` while the search prompt is open
    pub search_input: Option<String>,
    pub quit: bool,
    pub art: Option<(String, Option<image::RgbImage>)>,
}

impl AppState {
    pub fn new(api: SyncSender<ApiRequest>, player: SyncSender<PlayerCmd>, volume: f32) -> Self {
        let mut s = Self {
            api,
            player,
            next_id: 0,
            queue: Queue::default(),
            now: NowPlaying {
                volume,
                ..Default::default()
            },
            loading: None,
            enqueued: None,
            failures: 0,
            quit_armed: None,
            reporter: Reporter::default(),
            mpris_dirty: false,
            server_url: None,
            focus: Focus::List,
            section: 0,
            stack: Vec::new(),
            help: false,
            debug_line: false,
            message: None,
            list_height: 10,
            search_input: None,
            quit: false,
            art: None,
        };
        s.set_section(0);
        s
    }

    pub fn view(&self) -> Option<&View> {
        self.stack.last()
    }

    fn view_mut(&mut self) -> Option<&mut View> {
        self.stack.last_mut()
    }

    pub fn error(&mut self, text: String) {
        tracing::warn!("{text}");
        self.message = Some(Message {
            text,
            at: Instant::now(),
            error: true,
        });
    }

    pub fn info(&mut self, text: String) {
        self.message = Some(Message {
            text,
            at: Instant::now(),
            error: false,
        });
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
    pub fn list_len(&self) -> usize {
        match self.view() {
            Some(v) if matches!(v.kind, ListKind::Queue) => self.queue.len(),
            Some(v) => v.items.len(),
            None => 0,
        }
    }

    fn new_view(&mut self, kind: ListKind, title: String) -> View {
        self.next_id += 1;
        let items = match kind {
            ListKind::Artists => Items::Artists(Vec::new()),
            ListKind::Albums | ListKind::ArtistAlbums(_) => Items::Albums(Vec::new()),
            ListKind::Playlists => Items::Playlists(Vec::new()),
            ListKind::AlbumTracks(_) | ListKind::PlaylistTracks(_) | ListKind::Queue => {
                Items::Tracks(Vec::new())
            }
            ListKind::Search => Items::Search(Vec::new()),
        };
        View {
            id: self.next_id,
            kind,
            title,
            items,
            total: None,
            loading: false,
            selected: 0,
            offset: 0,
        }
    }

    fn set_section(&mut self, i: usize) {
        self.section = i;
        let kind = match i {
            0 => ListKind::Artists,
            1 => ListKind::Albums,
            2 => ListKind::Playlists,
            3 => ListKind::Search,
            _ => ListKind::Queue,
        };
        let view = self.new_view(kind, SECTIONS[i].to_owned());
        self.stack = vec![view];
        self.ensure_loaded();
    }

    fn push(&mut self, kind: ListKind, title: String) {
        let view = self.new_view(kind, title);
        self.stack.push(view);
        self.ensure_loaded();
    }

    /// requests the next page when the selection nears the end of what is loaded
    fn ensure_loaded(&mut self) {
        let api = self.api.clone();
        let Some(v) = self.view_mut() else { return };
        if v.loading || v.complete() {
            return;
        }
        let len = v.items.len();
        if len > 0 && v.selected + PREFETCH_MARGIN < len {
            return;
        }
        let req = ApiRequest::Page {
            view: v.id,
            kind: v.kind.clone(),
            start: len as u32,
        };
        if api.try_send(req).is_ok() {
            v.loading = true;
        }
    }

    pub fn on_page(&mut self, view: u64, start: u32, result: Result<PageData, String>) {
        let Some(v) = self.stack.iter_mut().find(|v| v.id == view) else {
            return;
        };
        v.loading = false;
        let page = match result {
            Ok(p) => p,
            Err(e) => return self.error(e),
        };
        if start as usize != v.items.len() {
            return;
        }
        match (&mut v.items, page) {
            (Items::Artists(dst), PageData::Artists(p)) => {
                v.total = Some(p.total);
                dst.extend(p.items);
            }
            (Items::Albums(dst), PageData::Albums(p)) => {
                v.total = Some(p.total);
                dst.extend(p.items);
            }
            (Items::Tracks(dst), PageData::Tracks(p)) => {
                v.total = Some(p.total);
                dst.extend(p.items);
            }
            (Items::Playlists(dst), PageData::Playlists(p)) => {
                v.total = Some(p.total);
                dst.extend(p.items);
            }
            _ => return,
        }
        // an empty page means the server's totalSize overstated the list
        if v.items.len() == start as usize {
            v.total = Some(start);
        }
        self.ensure_loaded();
    }

    pub fn on_action(&mut self, action: Action) {
        if self.help {
            if matches!(action, Action::Help | Action::Quit | Action::Back) {
                self.help = false;
            }
            return;
        }
        let half = (self.list_height / 2).max(1) as isize;
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
                self.focus = match self.focus {
                    Focus::Sidebar => Focus::List,
                    Focus::List => Focus::Sidebar,
                }
            }
            Action::Jump(i) => {
                self.set_section(i);
                self.focus = Focus::List;
            }
            Action::PlayPause => self.toggle_pause(),
            Action::Next => self.skip(),
            Action::Previous => self.previous(),
            Action::SeekForward => self.seek_by(true),
            Action::SeekBack => self.seek_by(false),
            Action::VolumeUp => self.change_volume(true),
            Action::VolumeDown => self.change_volume(false),
            Action::Shuffle => self.toggle_shuffle(),
            Action::Repeat => self.cycle_repeat(),
            Action::Append => self.add_selected(AddMode::Append),
            Action::PlayNext => self.add_selected(AddMode::Next),
            Action::Remove => self.queue_edit(|s, i| s.remove_at(i)),
            Action::MoveDown => self.queue_edit(|s, i| s.move_at(i, true)),
            Action::MoveUp => self.queue_edit(|s, i| s.move_at(i, false)),
            Action::Clear => self.queue_edit(|s, _| s.clear_queue()),
            Action::Search => self.open_search(),
            Action::Help => self.help = true,
            Action::DebugLine => self.debug_line = !self.debug_line,
            Action::Quit => self.request_quit(),
        }
    }

    pub fn on_art(&mut self, thumb: String, result: Result<image::RgbImage, String>) {
        let Some((key, img)) = &mut self.art else {
            return;
        };
        if *key != thumb {
            return;
        }

        match result {
            Ok(i) => *img = Some(i),
            Err(e) => tracing::warn!("cover art: {e}"),
        }
    }

    fn move_by(&mut self, delta: isize) {
        match self.focus {
            Focus::Sidebar => {
                let i = self
                    .section
                    .saturating_add_signed(delta)
                    .min(SECTIONS.len() - 1);
                if i != self.section {
                    self.set_section(i);
                }
            }
            Focus::List => {
                let last = self.list_len().saturating_sub(1);
                let Some(v) = self.view_mut() else { return };
                v.selected = v.selected.saturating_add_signed(delta).min(last);
                if let Items::Search(items) = &v.items {
                    v.selected = skip_headers(items, v.selected, delta >= 0);
                }
                self.ensure_loaded();
            }
        }
    }

    fn open(&mut self) {
        if self.focus == Focus::Sidebar {
            self.focus = Focus::List;
            return;
        }
        let Some(v) = self.view() else { return };
        let i = v.selected;
        if matches!(v.kind, ListKind::Queue) {
            return self.play_index(i);
        }
        let child = match &v.items {
            Items::Artists(a) => a.get(i).map(|a| {
                (
                    ListKind::ArtistAlbums(a.rating_key.clone()),
                    a.title.clone(),
                )
            }),
            Items::Albums(a) => a.get(i).map(|a| {
                let title = if a.parent_title.is_empty() {
                    a.title.clone()
                } else {
                    format!("{} — {}", a.title, a.parent_title)
                };
                (ListKind::AlbumTracks(a.rating_key.clone()), title)
            }),
            Items::Playlists(p) => p.get(i).map(|p| {
                (
                    ListKind::PlaylistTracks(p.rating_key.clone()),
                    p.title.clone(),
                )
            }),
            Items::Search(items) => match items.get(i) {
                Some(SearchItem::Artist(a)) => Some((
                    ListKind::ArtistAlbums(a.rating_key.clone()),
                    a.title.clone(),
                )),
                Some(SearchItem::Album(a)) => Some((
                    ListKind::AlbumTracks(a.rating_key.clone()),
                    format!("{} — {}", a.title, a.parent_title),
                )),
                Some(SearchItem::Track(t)) => return self.play_from_album(t.clone()),
                _ => None,
            },
            Items::Tracks(t) if i < t.len() => {
                let tracks = t.clone();
                self.queue.replace(tracks, i);
                return self.play_index(self.queue.current.unwrap_or(0));
            }
            Items::Tracks(_) => None,
        };
        if let Some((kind, title)) = child {
            self.push(kind, title);
        }
    }

    /// `a` / `n`: queues the selected track directly, or fetches every track of an album, artist or playlist
    fn add_selected(&mut self, mode: AddMode) {
        let Some(v) = self.view() else { return };
        let i = v.selected;
        let source = match &v.items {
            _ if matches!(v.kind, ListKind::Queue) => return,
            Items::Tracks(t) => match t.get(i) {
                Some(t) => return self.add_tracks(mode, vec![t.clone()]),
                None => return,
            },
            Items::Albums(a) => a.get(i).map(|a| TrackSource::Album(a.rating_key.clone())),
            Items::Artists(a) => a.get(i).map(|a| TrackSource::Artist(a.rating_key.clone())),
            Items::Playlists(p) => p
                .get(i)
                .map(|p| TrackSource::Playlist(p.rating_key.clone())),
            Items::Search(items) => match items.get(i) {
                Some(SearchItem::Track(t)) => return self.add_tracks(mode, vec![t.clone()]),
                Some(SearchItem::Album(a)) => Some(TrackSource::Album(a.rating_key.clone())),
                Some(SearchItem::Artist(a)) => Some(TrackSource::Artist(a.rating_key.clone())),
                _ => None,
            },
        };
        if let Some(source) = source
            && self
                .api
                .try_send(ApiRequest::Tracks { source, mode })
                .is_err()
        {
            self.error("busy, try again".into());
        }
    }

    /// Enter on a search result track: queue its whole album and start at the track
    fn play_from_album(&mut self, track: Track) {
        let start = track.rating_key.clone();
        if track.parent_rating_key.is_empty() {
            return self.add_tracks(AddMode::PlayFrom(start), vec![track]);
        }
        let req = ApiRequest::Tracks {
            source: TrackSource::Album(track.parent_rating_key),
            mode: AddMode::PlayFrom(start),
        };
        if self.api.try_send(req).is_err() {
            self.error("busy, try again".into());
        }
    }

    pub fn add_tracks(&mut self, mode: AddMode, tracks: Vec<Track>) {
        let n = tracks.len();
        match mode {
            AddMode::Append => self.queue.append(tracks),
            AddMode::Next => self.queue.play_next(tracks),
            AddMode::PlayFrom(rk) => {
                let start = tracks.iter().position(|t| t.rating_key == rk).unwrap_or(0);
                self.queue.replace(tracks, start);
                return self.play_index(self.queue.current.unwrap_or(0));
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

    fn back(&mut self) {
        if self.focus == Focus::List && self.stack.len() > 1 {
            self.stack.pop();
        } else {
            self.focus = Focus::Sidebar;
        }
    }
}

/// moves `i` off a group header, preferring the direction of travel
fn skip_headers(items: &[SearchItem], i: usize, forward: bool) -> usize {
    let is_item =
        |j: &usize| matches!(items.get(*j), Some(it) if !matches!(it, SearchItem::Header(_)));
    let ahead = (i..items.len()).find(is_item);
    let behind = (0..=i.min(items.len().saturating_sub(1)))
        .rev()
        .find(is_item);
    let pick = if forward {
        ahead.or(behind)
    } else {
        behind.or(ahead)
    };
    pick.unwrap_or(i)
}

impl AppState {
    fn open_search(&mut self) {
        if !matches!(self.stack.first().map(|v| &v.kind), Some(ListKind::Search)) {
            self.set_section(3);
        }
        self.stack.truncate(1);
        self.section = 3;
        self.focus = Focus::List;
        self.search_input = Some(String::new());
    }

    /// keys typed while the search prompt is open
    pub fn on_search_key(&mut self, k: KeyEvent) {
        let Some(input) = self.search_input.as_mut() else {
            return;
        };
        match k.code {
            KeyCode::Esc => self.search_input = None,
            KeyCode::Enter => {
                let query = input.trim().to_owned();
                self.search_input = None;
                if query.is_empty() {
                    return;
                }
                if let Some(v) = self.stack.first_mut() {
                    v.title = format!("Search: {query}");
                    v.loading = true;
                }
                if self.api.try_send(ApiRequest::Search { query }).is_err() {
                    self.error("busy, try again".into());
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

    pub fn on_search(&mut self, query: String, result: Result<SearchResults, String>) {
        let Some(v) = self
            .stack
            .iter_mut()
            .find(|v| matches!(v.kind, ListKind::Search))
        else {
            return;
        };
        // a newer search was submitted while this one was in flight
        if v.title != format!("Search: {query}") {
            return;
        }
        v.loading = false;
        let r = match result {
            Ok(r) => r,
            Err(e) => return self.error(e),
        };
        let groups: [(&str, Vec<SearchItem>); 3] = [
            (
                "Artists",
                r.artists.into_iter().map(SearchItem::Artist).collect(),
            ),
            (
                "Albums",
                r.albums.into_iter().map(SearchItem::Album).collect(),
            ),
            (
                "Tracks",
                r.tracks.into_iter().map(SearchItem::Track).collect(),
            ),
        ];
        let mut items = Vec::new();
        for (name, group) in groups {
            if !group.is_empty() {
                items.push(SearchItem::Header(format!("{name} ({})", group.len())));
                items.extend(group);
            }
        }
        v.total = Some(items.len() as u32);
        v.selected = skip_headers(&items, 0, true);
        v.offset = 0;
        v.items = Items::Search(items);
    }
}
