use std::sync::mpsc::SyncSender;

use super::{ApiRequest, PageData};
use crate::plex::api::SearchResults;
use crate::plex::models::{Album, Artist, Playlist, Track};

const PREFETCH_MARGIN: usize = 50;

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

/// the library side: sections, the stack of opened lists and their paging
pub struct Browser {
    api: SyncSender<ApiRequest>,
    next_id: u64,
    pub section: usize,
    pub stack: Vec<View>,
    pub list_height: usize,
}

impl Browser {
    pub fn new(api: SyncSender<ApiRequest>) -> Self {
        let mut b = Self {
            api,
            next_id: 0,
            section: 0,
            stack: Vec::new(),
            list_height: 10,
        };
        b.set_section(0);
        b
    }

    pub fn view(&self) -> Option<&View> {
        self.stack.last()
    }

    pub fn view_mut(&mut self) -> Option<&mut View> {
        self.stack.last_mut()
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

    pub fn set_section(&mut self, i: usize) {
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

    pub fn push(&mut self, kind: ListKind, title: String) {
        let view = self.new_view(kind, title);
        self.stack.push(view);
        self.ensure_loaded();
    }

    pub fn move_section(&mut self, delta: isize) {
        let i = self
            .section
            .saturating_add_signed(delta)
            .min(SECTIONS.len() - 1);
        if i != self.section {
            self.set_section(i);
        }
    }

    /// moves the selection within a list of `len` rows
    pub fn move_selection(&mut self, delta: isize, len: usize) {
        let Some(v) = self.view_mut() else { return };
        v.selected = v
            .selected
            .saturating_add_signed(delta)
            .min(len.saturating_sub(1));
        if let Items::Search(items) = &v.items {
            v.selected = skip_headers(items, v.selected, delta >= 0);
        }
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

    pub fn on_page(
        &mut self,
        view: u64,
        start: u32,
        result: Result<PageData, String>,
    ) -> Result<(), String> {
        let Some(v) = self.stack.iter_mut().find(|v| v.id == view) else {
            return Ok(());
        };
        v.loading = false;
        let page = result?;
        if start as usize != v.items.len() {
            return Ok(());
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
            _ => return Ok(()),
        }
        // an empty page means the server's totalSize overstated the list
        if v.items.len() == start as usize {
            v.total = Some(start);
        }
        self.ensure_loaded();
        Ok(())
    }

    /// switches to the search section, keeping earlier results
    pub fn show_search(&mut self) {
        if !matches!(self.stack.first().map(|v| &v.kind), Some(ListKind::Search)) {
            self.set_section(3);
        }
        self.stack.truncate(1);
        self.section = 3;
    }

    pub fn search(&mut self, query: String) -> Result<(), String> {
        if let Some(v) = self.stack.first_mut() {
            v.title = format!("Search: {query}");
            v.loading = true;
        }
        self.api
            .try_send(ApiRequest::Search { query })
            .map_err(|_| "busy, try again".to_owned())
    }

    pub fn on_search(
        &mut self,
        query: String,
        result: Result<SearchResults, String>,
    ) -> Result<(), String> {
        let Some(v) = self
            .stack
            .iter_mut()
            .find(|v| matches!(v.kind, ListKind::Search))
        else {
            return Ok(());
        };
        // a newer search was submitted while this one was in flight
        if v.title != format!("Search: {query}") {
            return Ok(());
        }
        v.loading = false;
        let r = result?;
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
        Ok(())
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
