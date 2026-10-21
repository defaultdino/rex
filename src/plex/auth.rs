use std::io::Write;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use tracing::{info, warn};

use super::PlexClient;
use super::models::Container;
use crate::config::SavedState;

const PLEX_TV: &str = "https://plex.tv";
const API_TIMEOUT: Duration = Duration::from_secs(15);
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pin {
    id: u64,
    code: String,
    #[serde(default)]
    auth_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Resource {
    #[serde(default)]
    name: String,
    #[serde(default)]
    provides: String,
    #[serde(default)]
    client_identifier: String,
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    connections: Vec<Connection>,
}

#[derive(Deserialize)]
struct Connection {
    uri: String,
    #[serde(default)]
    local: bool,
    #[serde(default)]
    relay: bool,
}

impl Connection {
    fn rank(&self) -> u8 {
        match (self.relay, self.local) {
            (false, true) => 0,
            (false, false) => 1,
            (true, _) => 2,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Identity {
    #[serde(default)]
    machine_identifier: String,
}

// Returns the plex account token
pub fn login(saved: &SavedState) -> Result<String> {
    let tv = PlexClient::new(PLEX_TV, "", &saved.client_identifier, API_TIMEOUT);
    let pin: Pin = tv.post("/api/v2/pins", &[("strong", "true")])?;
    println!(
        "Open this URL in a browser to sign in:\n\n  https://app.plex.tv/auth#?clientID={}&code={}&context%5Bdevice%5D%5Bproduct%5D=rexamp\n\nWaiting…",
        saved.client_identifier, pin.code
    );
    let deadline = Instant::now() + Duration::from_secs(300);
    let path = format!("/api/v2/pins/{}", pin.id);
    while Instant::now() < deadline {
        thread::sleep(Duration::from_secs(2));
        match tv.get::<Pin>(&path, &[("code", &pin.code)]) {
            Ok(p) => {
                if let Some(token) = p.auth_token.filter(|t| !t.is_empty()) {
                    return Ok(token);
                }
            }
            Err(e) => warn!("polling pin: {e:#}"),
        }
    }
    bail!("timed out waiting for sign-in")
}

pub fn connect(saved: &mut SavedState) -> Result<PlexClient> {
    let client = connect_server(saved)?;
    if saved.music_section.is_none() {
        pick_section(&client, saved, None)?;
    }
    Ok(client)
}

pub fn switch_section(saved: &mut SavedState, pick: Option<usize>) -> Result<String> {
    let account = saved
        .token
        .clone()
        .context("not logged in; run `rexamp login`")?;
    discover(saved, &account, None, pick).map(|(_, title)| title)
}

fn connect_server(saved: &mut SavedState) -> Result<PlexClient> {
    if let Some(url) = saved.server_url.clone() {
        let token = saved
            .server_token
            .clone()
            .or_else(|| saved.token.clone())
            .context("server_url is set but no token; run `rexamp login`")?;

        if probe(&url, saved.server_id.as_deref()) {
            return Ok(PlexClient::new(
                &url,
                &token,
                &saved.client_identifier,
                API_TIMEOUT,
            ));
        }

        warn!("the saved server {url} is unreachable, rediscovering");
    }

    let account = saved
        .token
        .clone()
        .context("not logged in; run `rexamp login`")?;

    let only = saved.server_id.clone();
    discover(saved, &account, only.as_deref(), None).map(|(client, _)| client)
}

fn probe(uri: &str, expected_id: Option<&str>) -> bool {
    let c = PlexClient::new(uri, "", "rexamp-probe", PROBE_TIMEOUT);
    match c.get_once::<Container<Identity>>("/identity", &[]) {
        Ok(id) => expected_id.is_none_or(|e| e == id.mc.machine_identifier),
        Err(e) => {
            info!("probe failed: {e:#}");
            false
        }
    }
}

/// servers on the account, optionally narrowed to one machine identifier
fn servers(account: &str, client_id: &str, only: Option<&str>) -> Result<Vec<Resource>> {
    let tv = PlexClient::new(PLEX_TV, account, client_id, API_TIMEOUT);
    let resources: Vec<Resource> = tv.get(
        "/api/v2/resources",
        &[("includeHttps", "1"), ("includeRelay", "1")],
    )?;
    let servers: Vec<Resource> = resources
        .into_iter()
        .filter(|r| r.provides.split(',').any(|p| p == "server"))
        .filter(|r| only.is_none_or(|id| id == r.client_identifier))
        .collect();
    if servers.is_empty() {
        bail!("no Plex Media Server found on this account");
    }
    Ok(servers)
}

/// probes every connection of every server in parallel; best reachable connection per server
fn reachable(servers: &[Resource]) -> Vec<Option<&Connection>> {
    thread::scope(|s| {
        let handles: Vec<_> = servers
            .iter()
            .map(|srv| {
                srv.connections
                    .iter()
                    .map(|c| {
                        (
                            c,
                            s.spawn(move || probe(&c.uri, Some(&srv.client_identifier))),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        handles
            .into_iter()
            .map(|conns| {
                conns
                    .into_iter()
                    .filter_map(|(c, h)| matches!(h.join(), Ok(true)).then_some(c))
                    .min_by_key(|c| c.rank())
            })
            .collect()
    })
}

/// finds a working address for the saved server after it stopped answering, and switches `client` to it
pub fn reconnect(
    client: &PlexClient,
    account: Option<&str>,
    server_id: Option<&str>,
    client_id: &str,
) -> Result<String> {
    let account = account.context("not logged in")?;
    let id = server_id.context("no saved server to look up")?;
    let servers = servers(account, client_id, Some(id))?;
    let uri = reachable(&servers)
        .into_iter()
        .flatten()
        .next()
        .map(|c| c.uri.clone())
        .context("server unreachable on every known address")?;
    client.set_base(&uri);
    Ok(uri)
}

fn discover(
    saved: &mut SavedState,
    account: &str,
    only: Option<&str>,
    pick: Option<usize>,
) -> Result<(PlexClient, String)> {
    println!("Loading servers...");

    let servers = servers(account, &saved.client_identifier, only)?;
    let reachable = reachable(&servers);

    let mut choices = Vec::new();
    for (srv, conn) in servers.iter().zip(reachable) {
        let Some(conn) = conn else {
            warn!("server {} unreachable", srv.name);
            continue;
        };
        let token = srv.access_token.as_deref().unwrap_or(account);
        let client = PlexClient::new(&conn.uri, token, &saved.client_identifier, API_TIMEOUT);
        let sections = match client.music_sections() {
            Ok(s) => s,
            Err(e) => {
                warn!("listing sections on {}: {e:#}", srv.name);
                continue;
            }
        };
        for sec in sections {
            choices.push((srv, client.base(), token.to_owned(), sec));
        }
    }
    if choices.is_empty() {
        bail!("no reachable server with a music library");
    }
    let labels: Vec<String> = choices
        .iter()
        .map(|(srv, _, _, sec)| format!("{} — {}", srv.name, sec.title))
        .collect();
    let i = match pick {
        Some(n) if (1..=labels.len()).contains(&n) => n - 1,
        Some(n) => bail!("no music library {n}; there are {}", labels.len()),
        None => choose("Music library", &labels)?,
    };
    let (srv, url, token, sec) = choices.swap_remove(i);

    info!("using server {} at {url}", srv.name);
    // since a saved queues tracks only exist on
    // the library they came from we need to reset the queue
    if saved.server_id.as_ref() != Some(&srv.client_identifier)
        || saved.music_section.as_ref() != Some(&sec.key)
    {
        saved.queue = None;
    }
    saved.server_id = Some(srv.client_identifier.clone());
    saved.server_url = Some(url.clone());
    saved.server_token = Some(token.clone());
    saved.music_section = Some(sec.key);
    saved.save()?;
    let client = PlexClient::new(&url, &token, &saved.client_identifier, API_TIMEOUT);
    Ok((client, format!("{} — {}", srv.name, sec.title)))
}

/// saves the chosen music library and returns its title
fn pick_section(
    client: &PlexClient,
    saved: &mut SavedState,
    pick: Option<usize>,
) -> Result<String> {
    let mut sections = client.music_sections()?;
    if sections.is_empty() {
        bail!("server has no music library");
    }
    let i = match pick {
        Some(n) if (1..=sections.len()).contains(&n) => n - 1,
        Some(n) => bail!("no music library {n}; this server has {}", sections.len()),
        None => {
            let labels: Vec<String> = sections.iter().map(|s| s.title.clone()).collect();
            choose("Music library", &labels)?
        }
    };
    let sec = sections.swap_remove(i);
    if saved.music_section.as_ref() != Some(&sec.key) {
        saved.queue = None;
    }
    saved.music_section = Some(sec.key);
    saved.save()?;
    Ok(sec.title)
}

fn choose(what: &str, labels: &[String]) -> Result<usize> {
    if labels.len() == 1 {
        return Ok(0);
    }
    loop {
        println!("{what}:");
        for (i, l) in labels.iter().enumerate() {
            println!("  {}) {l}", i + 1);
        }
        print!("Choose [1-{}]: ", labels.len());
        std::io::stdout().flush()?;
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line)? == 0 {
            bail!("no choice made");
        }
        match line.trim().parse::<usize>() {
            Ok(n) if (1..=labels.len()).contains(&n) => return Ok(n - 1),
            _ => println!("Invalid choice."),
        }
    }
}
