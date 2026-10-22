use std::sync::Arc;

use serde::{Deserialize, Serialize};

const LYRIC_STREAM_TYPE: u32 = 4;

#[derive(Deserialize)]
pub struct Container<T> {
    #[serde(rename = "MediaContainer")]
    pub mc: T,
}

#[derive(Deserialize)]
pub struct Directories<T> {
    #[serde(rename = "Directory", default = "Vec::new")]
    pub items: Vec<T>,
}

#[derive(Debug, Deserialize)]
pub struct Section {
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub title: String,
    #[serde(rename = "type", default)]
    pub kind: String,
}

#[derive(Debug)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: u32,
}

#[derive(Debug)]
pub struct Artist {
    pub rating_key: String,
    pub title: String,
}

#[derive(Debug)]
pub struct Album {
    pub rating_key: String,
    pub title: String,
    pub parent_title: String,
    pub year: Option<u32>,
    pub leaf_count: Option<u32>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Track {
    pub rating_key: String,
    pub title: String,
    pub grandparent_title: String,
    pub parent_title: String,
    pub parent_rating_key: String,
    /// album art path, falling back to the track's own thumbnail
    pub thumb: String,
    pub index: Option<u32>,
    /// disc number within the album
    pub disc: Option<u32>,
    pub duration_ms: Option<u64>,
    pub part_key: String,
    pub container: Option<String>,
    pub audio_codec: Option<String>,
}

#[derive(Debug)]
pub struct Playlist {
    pub rating_key: String,
    pub title: String,
    pub leaf_count: Option<u32>,
    pub duration_ms: Option<u64>,
}

#[derive(Debug)]
pub struct AvailableLyrics {
    pub id: String,
    pub format: String,
}

#[derive(Debug)]
pub struct TrackMetadata {
    pub lyrics: Vec<AvailableLyrics>,
}

#[derive(Debug)]
pub struct TrackLyrics {
    pub timed: bool,
    pub lines: Vec<LyricLine>,
}

#[derive(Debug)]
pub struct LyricLine {
    pub offset: usize,
    pub text: String,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct RawItem {
    rating_key: String,
    title: String,
    parent_title: String,
    grandparent_title: String,
    parent_rating_key: String,
    thumb: String,
    parent_thumb: String,
    year: Option<u32>,
    leaf_count: Option<u32>,
    index: Option<u32>,
    parent_index: Option<u32>,
    duration: Option<u64>,
    #[serde(rename = "Media")]
    media: Vec<RawMedia>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct RawMedia {
    container: Option<String>,
    audio_codec: Option<String>,
    #[serde(rename = "Part")]
    parts: Vec<RawPart>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawPart {
    key: String,
    #[serde(rename = "Stream")]
    streams: Vec<RawStream>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct RawStream {
    id: u32,
    stream_type: u32,
    format: Option<String>,
    timed: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MetadataList {
    #[serde(rename = "Metadata", default)]
    pub items: Vec<RawItem>,
    #[serde(default)]
    pub total_size: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RawLyricsContainer {
    #[serde(rename = "Lyrics")]
    pub lyrics: Vec<RawLyrics>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RawLyrics {
    timed: bool,
    #[serde(rename = "Line")]
    lines: Vec<RawLine>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct RawLine {
    start_offset: Option<usize>,
    #[serde(rename = "Span")]
    span: Vec<Option<RawSpan>>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct RawSpan {
    text: String,
}

impl From<RawLyrics> for TrackLyrics {
    fn from(r: RawLyrics) -> Self {
        Self {
            timed: r.timed,
            lines: r
                .lines
                .iter()
                .filter_map(|l| {
                    let offset = if r.timed {
                        l.start_offset?
                    } else {
                        l.start_offset.unwrap_or(0)
                    };
                    let span = l.span.iter().flatten().next()?;
                    Some(LyricLine {
                        offset,
                        text: span.text.clone(),
                    })
                })
                .collect(),
        }
    }
}

impl From<RawItem> for Artist {
    fn from(r: RawItem) -> Self {
        Self {
            rating_key: r.rating_key,
            title: r.title,
        }
    }
}

impl From<RawItem> for Album {
    fn from(r: RawItem) -> Self {
        Self {
            rating_key: r.rating_key,
            title: r.title,
            parent_title: r.parent_title,
            year: r.year,
            leaf_count: r.leaf_count,
        }
    }
}

impl From<RawItem> for Track {
    fn from(r: RawItem) -> Self {
        let media = r.media.into_iter().next().unwrap_or_default();
        Self {
            rating_key: r.rating_key,
            title: r.title,
            grandparent_title: r.grandparent_title,
            parent_title: r.parent_title,
            parent_rating_key: r.parent_rating_key,
            thumb: if r.parent_thumb.is_empty() {
                r.thumb
            } else {
                r.parent_thumb
            },
            index: r.index,
            disc: r.parent_index,
            duration_ms: r.duration,
            part_key: media
                .parts
                .into_iter()
                .next()
                .map(|p| p.key)
                .unwrap_or_default(),
            container: media.container,
            audio_codec: media.audio_codec,
        }
    }
}

impl From<RawItem> for TrackMetadata {
    fn from(r: RawItem) -> Self {
        Self {
            lyrics: r
                .media
                .into_iter()
                .next()
                .unwrap_or_default()
                .parts
                .into_iter()
                .next()
                .iter()
                .flat_map(|p| {
                    p.streams
                        .iter()
                        .filter(|s| s.stream_type == LYRIC_STREAM_TYPE)
                        .map(|s| AvailableLyrics {
                            id: s.id.to_string(),
                            format: s.format.clone().unwrap_or_default(),
                        })
                })
                .collect(),
        }
    }
}

impl From<RawItem> for Arc<Track> {
    fn from(r: RawItem) -> Self {
        Arc::new(r.into())
    }
}

impl From<RawItem> for Playlist {
    fn from(r: RawItem) -> Self {
        Self {
            rating_key: r.rating_key,
            title: r.title,
            leaf_count: r.leaf_count,
            duration_ms: r.duration,
        }
    }
}
