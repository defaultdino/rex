use std::process::ExitCode;

use clap::Parser;

use crate::config::SavedState;
use crate::plex::auth;

#[derive(Parser, Debug)]
pub(super) struct Options {
    section: Option<usize>,
}

impl Options {
    pub fn run(self, saved: &mut SavedState) -> anyhow::Result<ExitCode> {
        let section_name = auth::switch_section(saved, self.section)?;
        println!("Using {section_name}");
        Ok(ExitCode::SUCCESS)
    }
}
