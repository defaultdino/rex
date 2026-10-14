use std::fmt::Debug;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;

use crate::audio::player::{self, PlayerCmd, PlayerEvent};
use crate::config::{self, SavedState};
use crate::plex::auth;
use crate::plex::models::Page;

#[derive(Parser, Debug)]
pub(super) struct Options {
    #[command(subcommand)]
    subcommand: Subcommand,
}

#[derive(Parser, Debug)]
enum Subcommand {
    /// List all library sections on the server
    Sections,

    /// List every artist in the music library
    Artists,

    /// List an artist's albums, or every album when no artist is given
    Albums {
        /// Artist rating key
        artist: Option<String>,
    },

    /// List an album's tracks
    Tracks {
        /// Album rating key
        album: String,
    },

    /// List audio playlists, or a playlist's tracks when one is given
    Playlists {
        /// Playlist rating key
        playlist: Option<String>,
    },

    /// Search artists, albums and tracks
    Search {
        #[arg(required = true)]
        query: Vec<String>,
    },

    /// Play tracks gaplessly without the TUI
    ///
    /// Reads commands from stdin: p (pause/resume), s <secs> (seek), . and , (±10s), + and - (volume), n (next), q (quit)
    Play {
        /// Track rating keys, played in order
        #[arg(required = true)]
        tracks: Vec<String>,
    },
}

impl Options {
    pub fn run(self, saved: &mut SavedState) -> Result<ExitCode> {
        use Subcommand as SC;
        let client = auth::connect(saved)?;
        let section = saved
            .music_section
            .clone()
            .context("no music section selected")?;
        match self.subcommand {
            SC::Sections => client.sections()?.iter().for_each(|s| println!("{s:?}")),
            SC::Artists => print_all(|start| client.artists(&section, start))?,
            SC::Albums { artist: None } => print_all(|start| client.albums(&section, start))?,
            SC::Albums { artist: Some(rk) } => {
                print_all(|start| client.artist_albums(&section, &rk, start))?
            }
            SC::Tracks { album } => print_all(|start| client.album_tracks(&album, start))?,
            SC::Playlists { playlist: None } => print_all(|start| client.playlists(start))?,
            SC::Playlists { playlist: Some(rk) } => {
                print_all(|start| client.playlist_tracks(&rk, start))?
            }
            SC::Search { query } => println!("{:#?}", client.search(&section, &query.join(" "))?),
            SC::Play { tracks } => play(client, saved.volume, &tracks)?,
        }
        Ok(ExitCode::SUCCESS)
    }
}

fn print_all<T: Debug>(mut fetch: impl FnMut(u32) -> Result<Page<T>>) -> Result<()> {
    let mut seen = 0;
    loop {
        let page = fetch(seen)?;
        if page.items.is_empty() {
            break;
        }
        seen += page.items.len() as u32;
        page.items.iter().for_each(|i| println!("{i:?}"));
        eprintln!("-- {seen} of {}", page.total);
        if seen >= page.total {
            break;
        }
    }
    Ok(())
}

fn play(client: crate::plex::PlexClient, volume: f32, keys: &[String]) -> Result<()> {
    let client = Arc::new(client);
    let tracks = keys
        .iter()
        .map(|k| client.track(k))
        .collect::<Result<Vec<_>>>()?;
    let cache = config::dirs()?.cache_dir().join("stream");
    let (cmd, events) = player::spawn(client, cache, volume)?;

    let (line_tx, lines) = mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lines().map_while(Result::ok) {
            if line_tx.send(line).is_err() {
                break;
            }
        }
    });

    let mut current = 0;
    let (mut pos, mut paused, mut vol) = (0u64, false, volume);
    cmd.send(PlayerCmd::Load(tracks[0].clone()))?;
    loop {
        match events.recv_timeout(Duration::from_millis(50)) {
            Ok(PlayerEvent::Position(ms)) => {
                pos = ms;
                eprint!("\r{}  rss {} kB   ", crate::fmt_ms(ms), crate::rss_kb());
            }
            Ok(PlayerEvent::TrackStarted(t)) => {
                current = tracks
                    .iter()
                    .position(|x| x.rating_key == t.rating_key)
                    .unwrap_or(current);
                eprintln!(
                    "\nstarted: {} — {} [{:?}]",
                    t.grandparent_title, t.title, t.audio_codec
                );
                if let Some(next) = tracks.get(current + 1) {
                    cmd.send(PlayerCmd::EnqueueNext(next.clone()))?;
                }
            }
            Ok(PlayerEvent::TrackEnded(rk)) => {
                eprintln!("\nended: {rk}");
                if tracks.last().is_some_and(|t| t.rating_key == rk) {
                    break;
                }
            }
            Ok(PlayerEvent::Error { message, .. }) => eprintln!("\nerror: {message}"),
            Ok(PlayerEvent::BufferingChanged(b)) => eprintln!("\nbuffering: {b}"),
            Ok(PlayerEvent::Paused) => {
                paused = true;
                eprintln!("\npaused by the player");
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        while let Ok(line) = lines.try_recv() {
            let mut words = line.split_whitespace();
            let c = match words.next().unwrap_or("p") {
                "p" => {
                    paused = !paused;
                    if paused {
                        PlayerCmd::Pause
                    } else {
                        PlayerCmd::Play
                    }
                }
                "s" => match words.next().and_then(|s| s.parse::<u64>().ok()) {
                    Some(secs) => PlayerCmd::Seek(secs * 1000),
                    None => continue,
                },
                "." => PlayerCmd::Seek(pos + 10_000),
                "," => PlayerCmd::Seek(pos.saturating_sub(10_000)),
                "+" | "-" => {
                    vol = (vol + if line.starts_with('+') { 0.05 } else { -0.05 }).clamp(0.0, 1.0);
                    eprintln!("volume {:.0}%", vol * 100.0);
                    PlayerCmd::SetVolume(vol)
                }
                "n" => match tracks.get(current + 1) {
                    Some(t) => PlayerCmd::Load(t.clone()),
                    None => continue,
                },
                "q" => {
                    cmd.send(PlayerCmd::Stop)?;
                    return Ok(());
                }
                _ => continue,
            };
            cmd.send(c)?;
        }
    }
    eprintln!("rss {} kB", crate::rss_kb());
    Ok(())
}
