use std::sync::Arc;

use anyhow::Result;

use super::PlexClient;
use super::models::{
    Album, Artist, Container, Directories, MetadataList, Page, Playlist, RawItem, Section, Track,
};

pub const PAGE_SIZE: u32 = 200;
const SEARCH_LIMIT: u32 = 50;

const TYPE_ARTIST: &str = "8";
const TYPE_ALBUM: &str = "9";
const TYPE_TRACK: &str = "10";

#[derive(Debug, Default)]
pub struct SearchResults {
    pub artists: Vec<Artist>,
    pub albums: Vec<Album>,
    pub tracks: Vec<Arc<Track>>,
}

impl PlexClient {
    fn page<T: From<RawItem>>(
        &self,
        path: &str,
        query: &[(&str, &str)],
        start: u32,
        size: u32,
    ) -> Result<Page<T>> {
        let c: Container<MetadataList> = self.get_range(path, query, start, size)?;
        let items: Vec<T> = c.mc.items.into_iter().map(T::from).collect();
        let total = c.mc.total_size.unwrap_or(start + items.len() as u32);
        Ok(Page { items, total })
    }

    #[cfg(feature = "cast")]
    pub fn transient_token(&self) -> Result<String> {
        let query = [("type", "delegation"), ("scope", "all")];
        let c: Container<super::models::TransientToken> = self.get("/security/token", &query)?;
        Ok(c.mc.token)
    }

    pub fn sections(&self) -> Result<Vec<Section>> {
        let c: Container<Directories<Section>> = self.get("/library/sections", &[])?;
        Ok(c.mc.items)
    }

    pub fn music_sections(&self) -> Result<Vec<Section>> {
        let mut s = self.sections()?;
        s.retain(|s| s.kind == "artist");
        Ok(s)
    }

    pub fn artists(&self, section: &str, start: u32) -> Result<Page<Artist>> {
        let path = format!("/library/sections/{section}/all");
        self.page(&path, &[("type", TYPE_ARTIST)], start, PAGE_SIZE)
    }

    pub fn albums(&self, section: &str, start: u32) -> Result<Page<Album>> {
        let path = format!("/library/sections/{section}/all");
        self.page(&path, &[("type", TYPE_ALBUM)], start, PAGE_SIZE)
    }

    /// every release by `artist`, newest first; the artist's `/children` leaves out releases
    /// Plex files as EPs, live albums or compilations
    pub fn artist_albums(&self, section: &str, artist: &str, start: u32) -> Result<Page<Album>> {
        let path = format!("/library/sections/{section}/all");
        let query = [
            ("type", TYPE_ALBUM),
            ("artist.id", artist),
            ("sort", "year:desc"),
        ];
        self.page(&path, &query, start, PAGE_SIZE)
    }

    pub fn album_tracks(&self, album: &str, start: u32) -> Result<Page<Arc<Track>>> {
        let path = format!("/library/metadata/{album}/children");
        self.page(&path, &[], start, PAGE_SIZE)
    }

    pub fn artist_tracks(&self, artist: &str, start: u32) -> Result<Page<Arc<Track>>> {
        let path = format!("/library/metadata/{artist}/allLeaves");
        self.page(&path, &[], start, PAGE_SIZE)
    }

    pub fn playlists(&self, start: u32) -> Result<Page<Playlist>> {
        self.page("/playlists", &[("playlistType", "audio")], start, PAGE_SIZE)
    }

    pub fn playlist_tracks(&self, playlist: &str, start: u32) -> Result<Page<Arc<Track>>> {
        let path = format!("/playlists/{playlist}/items");
        self.page(&path, &[], start, PAGE_SIZE)
    }

    pub fn search(&self, section: &str, query: &str) -> Result<SearchResults> {
        let path = format!("/library/sections/{section}/search");
        let q = |kind| [("type", kind), ("query", query)];
        std::thread::scope(|s| {
            let artists = s.spawn(|| self.page(&path, &q(TYPE_ARTIST), 0, SEARCH_LIMIT));
            let albums = s.spawn(|| self.page(&path, &q(TYPE_ALBUM), 0, SEARCH_LIMIT));
            let tracks = self.page(&path, &q(TYPE_TRACK), 0, SEARCH_LIMIT)?.items;
            Ok(SearchResults {
                artists: artists.join().unwrap()?.items,
                albums: albums.join().unwrap()?.items,
                tracks,
            })
        })
    }
}
