use cast_sender::Image;
use cast_sender::namespace::media::{
    MediaInformation, MusicTrackMediaMetadata, RequestData, StreamType,
};

use super::Caster;
use crate::plex::models::Track;

impl Caster {
    pub(super) fn media_info(&self, track: &Track) -> Result<MediaInformation, String> {
        if track.part_key.is_empty() {
            return Err("no playable media".into());
        }
        let content_type = content_type(track).ok_or_else(|| {
            let format = track.audio_codec.as_deref().or(track.container.as_deref());
            format!(
                "cast devices can't play {}",
                format.unwrap_or("this format")
            )
        })?;
        let images = (!track.thumb.is_empty()).then(|| {
            vec![Image {
                url: self.plex.url_with_token(&track.thumb),
                height: None,
                width: None,
            }]
        });
        let metadata = MusicTrackMediaMetadata {
            title: Some(track.title.clone()),
            artist: Some(track.grandparent_title.clone()),
            album_name: Some(track.parent_title.clone()),
            track_number: track.index.map(|n| n as i32),
            disc_number: track.disc.map(|n| n as i32),
            images,
            ..Default::default()
        };
        Ok(MediaInformation {
            content_id: self.plex.url_with_token(&track.part_key),
            content_type: content_type.into(),
            stream_type: StreamType::Buffered,
            duration: track.duration_ms.map(|d| d as f64 / 1000.0),
            metadata: Some(metadata.into()),
            ..Default::default()
        })
    }
}

pub(super) fn request(id: i32) -> RequestData {
    RequestData {
        media_session_id: Some(id),
    }
}

pub(super) fn secs(ms: u64) -> f64 {
    ms as f64 / 1000.0
}

fn content_type(track: &Track) -> Option<&'static str> {
    let codec = track.audio_codec.as_deref().unwrap_or_default();
    Some(
        match (track.container.as_deref().unwrap_or_default(), codec) {
            ("mp3", "mp3" | "") => "audio/mpeg",
            ("flac", "flac" | "") => "audio/flac",
            ("mp4" | "m4a" | "m4b", "aac") => "audio/mp4",
            ("aac", "aac" | "") => "audio/aac",
            ("ogg", "vorbis" | "opus" | "flac") => "audio/ogg",
            ("webm", "vorbis" | "opus") => "audio/webm",
            ("wav", "pcm" | "") => "audio/wav",
            _ => return None,
        },
    )
}
