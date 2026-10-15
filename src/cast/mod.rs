//! Plays on a Google Cast device through its Default Media Receiver, behind the same
//! `PlayerCmd`/`PlayerEvent` contract as the local player

mod caster;
mod io;

use std::net::IpAddr;
use std::sync::Arc;
use std::sync::mpsc::sync_channel;
use std::thread::{self, JoinHandle};

use anyhow::{Context, Result};
use smol::block_on;
use smol::channel;

use crate::backend::Channels;
use crate::plex::PlexClient;
use caster::Caster;

/// Drop the sender and join the handle before exiting, or the device keeps playing
// shortcut: nothing calls this until the tui can cast, drop the allow in that pr
#[allow(dead_code)]
pub fn spawn(plex: Arc<PlexClient>, ip: &str, volume: f32) -> Result<(Channels, JoinHandle<()>)> {
    let ip: IpAddr = ip
        .parse()
        .with_context(|| format!("{ip} is not an IP address"))?;
    let first = block_on(io::connect(ip)).with_context(|| format!("casting to {ip}"))?;
    let (cmd_tx, cmd_rx) = sync_channel(32);
    let (ev_tx, ev_rx) = sync_channel(256);
    let (fwd_tx, fwd_rx) = channel::bounded(32);

    thread::Builder::new()
        .name("cast-cmds".into())
        .stack_size(64 * 1024)
        .spawn(move || {
            while let Ok(cmd) = cmd_rx.recv() {
                if fwd_tx.send_blocking(cmd).is_err() {
                    break;
                }
            }
        })?;
    let cast = thread::Builder::new()
        .name("cast".into())
        .spawn(move || block_on(io::run(Caster::new(plex, ev_tx, volume), ip, first, fwd_rx)))?;
    Ok(((cmd_tx, ev_rx), cast))
}
