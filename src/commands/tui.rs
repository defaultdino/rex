use std::process::ExitCode;
use std::sync::Arc;

use anyhow::Result;
use clap::Parser;

#[cfg(feature = "cast")]
use crate::cast::Device;
use crate::config::SavedState;
use crate::plex::auth;

#[derive(Parser, Debug, Default)]
pub(super) struct Options {}

impl Options {
    pub fn run(
        self,
        saved: &mut SavedState,
        accent: Option<&str>,
        #[cfg(feature = "cast")] static_cast_devices: Vec<Device>,
    ) -> Result<ExitCode> {
        if saved.token.is_none() && saved.server_url.is_none() {
            saved.token = Some(auth::login(saved)?);
            saved.save()?;
        }
        println!("Connecting...");
        let client = auth::connect(saved)?;
        crate::app::run(
            Arc::new(client),
            saved,
            accent,
            #[cfg(feature = "cast")]
            static_cast_devices,
        )?;
        Ok(ExitCode::SUCCESS)
    }
}
