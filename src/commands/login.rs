use std::process::ExitCode;

use clap::Parser;

use crate::config::SavedState;
use crate::plex::auth;

#[derive(Parser, Debug)]
pub(super) struct Options {}

impl Options {
    pub fn run(self, saved: &mut SavedState) -> anyhow::Result<ExitCode> {
        // reset not only auth settings but
        // also user state
        saved.token = Some(auth::login(saved)?);
        saved.server_url = None;
        saved.server_id = None;
        saved.server_token = None;
        saved.music_section = None;
        saved.save()?;
        let client = auth::connect(saved)?;
        println!("Signed in. Using {}.", client.base());
        Ok(ExitCode::SUCCESS)
    }
}
