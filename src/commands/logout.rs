use std::process::ExitCode;

use clap::Parser;

use crate::config::SavedState;

#[derive(Parser, Debug)]
pub(super) struct Options {}

impl Options {
    pub fn run(self, saved: &mut SavedState) -> anyhow::Result<ExitCode> {
        saved.token = None;
        saved.server_token = None;
        saved.save()?;
        println!("Logged out.");
        Ok(ExitCode::SUCCESS)
    }
}
