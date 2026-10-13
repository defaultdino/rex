use clap::Parser;
use commands::setup_logging;
use std::process::ExitCode;

mod app;
mod audio;
mod commands;
mod config;
mod media_controls;
mod plex;
mod ui;

fn main() -> anyhow::Result<ExitCode> {
    let options = self::commands::Options::parse();

    setup_logging(options.log_level)?;
    let mut cfg = config::Config::load()?;

    tracing::info!(version = env!("CARGO_PKG_VERSION"), "starting");
    options.run(&mut cfg)
}

pub fn fmt_ms(ms: u64) -> String {
    let secs = ms / 1000;
    match secs / 3600 {
        0 => format!("{}:{:02}", secs / 60, secs % 60),
        h => format!("{h}:{:02}:{:02}", secs / 60 % 60, secs % 60),
    }
}

/// resident set size of this process in kB, 0 if unavailable
pub fn rss_kb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("VmRSS:"))
                .and_then(|v| v.trim().trim_end_matches(" kB").parse().ok())
        })
        .unwrap_or(0)
}
