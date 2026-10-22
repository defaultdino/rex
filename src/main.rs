use clap::Parser;
use commands::setup_logging;
use std::process::ExitCode;

mod app;
mod audio;
mod backend;
#[cfg(feature = "cast")]
mod cast;
mod commands;
mod config;
mod media_controls;
mod plex;
mod ui;

fn main() -> anyhow::Result<ExitCode> {
    let options = self::commands::Options::parse();
    setup_logging(options.log_level)?;

    let cfg = config::Config::load()?;
    let mut saved = config::SavedState::load()?;

    tracing::info!(version = env!("CARGO_PKG_VERSION"), "starting");
    options.run(cfg, &mut saved)
}
