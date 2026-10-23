//! Plays on a Google Cast device through its Default Media Receiver, behind the same
//! `PlayerCmd`/`PlayerEvent` contract as the local player

mod caster;
mod discover;
mod io;

pub use discover::{Device, discover};

use std::net::IpAddr;
use std::sync::Arc;
use std::sync::mpsc::{SyncSender, sync_channel};
use std::thread;

use anyhow::{Context, Result, bail};
use smol::block_on;
use smol::channel;

use crate::backend::{Events, PlayerCmd};
use crate::plex::PlexClient;
use caster::Caster;

/// connects to the device and returns its command sender and the session, which runs on
/// the calling thread until the device disconnects or the sender is dropped
pub fn connect(
    plex: Arc<PlexClient>,
    ip: IpAddr,
    events: Events,
) -> Result<(SyncSender<PlayerCmd>, impl FnOnce())> {
    let server = plex.base();
    if loopback(&server) {
        bail!("rexamp reaches plex at {server}, which the device can't");
    }
    let token = plex
        .transient_token()
        .context("getting a cast token from plex")?;
    let first = block_on(io::connect(ip)).with_context(|| format!("casting to {ip}"))?;
    let (cmd_tx, cmd_rx) = sync_channel(32);
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

    let session = move || block_on(io::run(Caster::new(plex, token, events), ip, first, fwd_rx));
    Ok((cmd_tx, session))
}

fn loopback(url: &str) -> bool {
    let Some(host) = url
        .parse::<ureq::http::Uri>()
        .ok()
        .and_then(|u| u.host().map(str::to_owned))
    else {
        return false;
    };
    // plex.direct names spell the ip in their first label: 127-0-0-1.<id>.plex.direct
    let label = host.split('.').next().unwrap_or_default().replace('-', ".");
    host == "localhost"
        || [host.trim_matches(['[', ']']), &label]
            .iter()
            .any(|h| h.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback()))
}
