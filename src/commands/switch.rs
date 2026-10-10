use crate::config::Config;
use crate::plex::auth;
use clap::Parser;
use std::fmt::Debug;
use std::process::ExitCode;

#[derive(Debug, Parser, Default)]
pub(super) struct Options {
    section: Option<usize>,
}

impl Options {
    pub fn run(self, cfg: &mut Config) -> anyhow::Result<ExitCode> {
        let client = auth::connect(cfg)?;

        match self.section {
            Some(section) => {
                // user passed a section by number, try to set it
                // to a section on their library
            }
            None => {
                // find sections for user, display in number format,
                // let them pick one, then set it
            }
        }

        Ok(ExitCode::SUCCESS)
    }
}
