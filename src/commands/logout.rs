use std::process::ExitCode;

use crate::config::SavedState;

pub(super) fn run(saved: &mut SavedState) -> anyhow::Result<ExitCode> {
    saved.token = None;
    saved.server_token = None;
    saved.save()?;
    println!("Logged out.");
    Ok(ExitCode::SUCCESS)
}
