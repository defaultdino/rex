use std::process::ExitCode;

use clap::Parser;

use crate::config::Config;
use crate::plex::auth;

#[derive(Parser, Debug)]
pub(super) struct Options {
    section: Option<usize>,
}

impl Options {
    pub fn run(self, cfg: &mut Config) -> anyhow::Result<ExitCode> {
        let section_name = auth::switch_section(cfg, self.section)?;
        println!("Using {section_name}");
        Ok(ExitCode::SUCCESS)
    }
}
