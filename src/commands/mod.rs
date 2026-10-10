use std::process::ExitCode;

use clap::Parser;
use tracing::Level;
use tracing_appender::rolling::{RollingFileAppender, Rotation};

use crate::config::{self, Config};

mod debug;
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
    /// Query the Plex server without the TUI
    Debug(self::debug::Options),
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
    pub fn run(self, cfg: &mut Config) -> anyhow::Result<ExitCode> {
        use Subcommand as S;
        match self.subcommand {
            Some(S::Login(c)) => c.run(cfg),
            Some(S::Logout(c)) => c.run(cfg),
            Some(S::Debug(c)) => c.run(cfg),
            Some(S::Switch(c)) => c.run(cfg),
            None => self::tui::Options::default().run(cfg),
        }
    }
}

// sets up a file logger in the app directory
// as rex.log with daily log rotations
pub fn setup_logging(level: Level) -> anyhow::Result<()> {
    let dir = config::state_dir()?;
    std::fs::create_dir_all(&dir)?;
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("rex")
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
