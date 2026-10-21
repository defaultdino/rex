use std::process::ExitCode;

use clap::Parser;
use tracing::Level;
use tracing_appender::rolling::{RollingFileAppender, Rotation};

use crate::config::{self, Config, SavedState};

mod login;
mod logout;
mod switch;
mod tui;

#[derive(Parser, Debug)]
enum Subcommand {
    /// Sign in to Plex and choose a server and music library
    Login(self::login::Options),
    /// Remove saved tokens
    Logout(self::logout::Options),
    /// Switch media libraries
    Switch(self::switch::Options),
}

#[derive(Parser, Debug)]
#[command(version)]
pub struct Options {
    #[arg(long, global = true, default_value = "info")]
    pub log_level: Level,
    #[command(subcommand)]
    subcommand: Option<Subcommand>,
}

impl Options {
    pub fn run(self, cfg: Config, saved: &mut SavedState) -> anyhow::Result<ExitCode> {
        use Subcommand as S;
        match self.subcommand {
            Some(S::Login(c)) => c.run(saved),
            Some(S::Logout(c)) => c.run(saved),
            Some(S::Switch(c)) => c.run(saved),
            None => self::tui::Options::default().run(
                saved,
                cfg.accent_color.as_deref(),
                #[cfg(feature = "cast")]
                cfg.static_cast_devices.unwrap_or_default(),
            ),
        }
    }
}

// sets up a file logger in the app directory
// as rexamp.log with daily log rotations
pub fn setup_logging(level: Level) -> anyhow::Result<()> {
    let dir = config::state_dir()?;
    std::fs::create_dir_all(&dir)?;
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("rexamp")
        .filename_suffix("log")
        .max_log_files(3)
        .build(dir)?;
    tracing_subscriber::fmt()
        .with_writer(appender)
        .with_ansi(false)
        .with_max_level(level)
        .init();
    Ok(())
}
