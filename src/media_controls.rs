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
use crate::plex::PlexClient;

pub struct MediaControls {
    controls: souvlaki::MediaControls,
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
            dbus_name: "rex",
            display_name: "rex",
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
        Some(Self { controls })
    }

    pub fn update(&mut self, now: &NowPlaying, client: &PlexClient) {
        let Some(t) = &now.track else {
            return self.apply(MediaMetadata::default(), MediaPlayback::Stopped);
        };
        let art = (!t.thumb.is_empty()).then(|| client.url_with_token(&t.thumb));
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

    fn apply(&mut self, metadata: MediaMetadata, playback: MediaPlayback) {
        if let Err(e) = self.controls.set_metadata(metadata) {
            warn!("media controls metadata: {e:?}");
        }
        if let Err(e) = self.controls.set_playback(playback) {
            warn!("media controls playback: {e:?}");
        }
    }
}
