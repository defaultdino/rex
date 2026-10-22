use std::fs;
use std::path::PathBuf;
use std::sync::mpsc::SyncSender;
use std::thread::ScopedJoinHandle;
use std::time::Duration;

use anyhow::Result;
#[cfg(target_os = "macos")]
use core_foundation::runloop::{CFRunLoop, kCFRunLoopDefaultMode};
use souvlaki::{MediaControlEvent, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig};
use tracing::warn;

use crate::app::AppEvent;
use crate::app::now_playing::NowPlaying;
use crate::app::playback::Cover;

pub struct MediaControls {
    controls: souvlaki::MediaControls,
    art_dir: PathBuf,
    art: Option<(String, PathBuf)>,
    saved: u64,
}

/// blocks the main thread on the run loop macOS uses for media key and Control Center commands
#[cfg(target_os = "macos")]
pub fn run_main_loop(tui: &ScopedJoinHandle<'_, Result<()>>) {
    while !tui.is_finished() {
        CFRunLoop::run_in_mode(
            unsafe { kCFRunLoopDefaultMode },
            Duration::from_secs(1),
            false,
        );
    }
}

#[cfg(not(target_os = "macos"))]
pub fn run_main_loop(_tui: &ScopedJoinHandle<'_, Result<()>>) {}

impl MediaControls {
    /// registers on the session bus; returns `None` (after logging once) when D-Bus isn't usable
    pub fn start(tx: SyncSender<AppEvent>) -> Option<Self> {
        let config = PlatformConfig {
            dbus_name: "rexamp",
            display_name: "rexamp",
            hwnd: None,
        };
        let mut controls = match souvlaki::MediaControls::new(config) {
            Ok(c) => c,
            Err(e) => {
                warn!("media controls unavailable: {e:?}");
                return None;
            }
        };
        let attached = controls.attach(move |ev: MediaControlEvent| {
            let _ = tx.send(AppEvent::MediaControl(ev));
        });
        if let Err(e) = attached {
            warn!("media controls unavailable: {e:?}");
            return None;
        }
        let art_dir = crate::config::dirs().ok()?.cache_dir().to_path_buf();
        Some(Self {
            controls,
            art_dir,
            art: None,
            saved: 0,
        })
    }

    pub fn update(&mut self, now: &NowPlaying, cover: Option<&Cover>) {
        let Some(t) = &now.track else {
            return self.apply(MediaMetadata::default(), MediaPlayback::Stopped);
        };
        let art = cover
            .filter(|c| c.thumb == t.thumb)
            .and_then(|c| self.art_url(c));
        let metadata = MediaMetadata {
            title: Some(&t.title),
            artist: Some(&t.grandparent_title),
            album: Some(&t.parent_title),
            duration: t.duration_ms.map(Duration::from_millis),
            cover_url: art.as_deref(),
        };
        let progress = Some(MediaPosition(Duration::from_millis(now.position_ms)));
        let playback = if now.paused {
            MediaPlayback::Paused { progress }
        } else {
            MediaPlayback::Playing { progress }
        };
        self.apply(metadata, playback);
    }

    fn art_url(&mut self, c: &Cover) -> Option<String> {
        let img = c.art.as_ref()?;
        if self.art.as_ref().is_none_or(|(thumb, _)| *thumb != c.thumb) {
            self.saved += 1;
            let path = self.art_dir.join(format!("cover-{}.png", self.saved));
            if let Err(e) = fs::create_dir_all(&self.art_dir)
                .map_err(Into::into)
                .and_then(|()| img.save(&path))
            {
                warn!("saving cover for media controls: {e}");
                return None;
            }
            if let Some((_, old)) = self.art.replace((c.thumb.clone(), path)) {
                let _ = fs::remove_file(old);
            }
        }
        self.art
            .as_ref()
            .map(|(_, p)| format!("file://{}", p.display()))
    }

    fn apply(&mut self, metadata: MediaMetadata, playback: MediaPlayback) {
        if let Err(e) = self.controls.set_metadata(metadata) {
            warn!("media controls metadata: {e:?}");
        }
        if let Err(e) = self.controls.set_playback(playback) {
            warn!("media controls playback: {e:?}");
        }
    }
}

impl Drop for MediaControls {
    fn drop(&mut self) {
        if let Some((_, path)) = &self.art {
            let _ = fs::remove_file(path);
        }
    }
}
