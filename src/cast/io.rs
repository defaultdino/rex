use std::future::Future;
use std::net::IpAddr;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use cast_sender::{Client, Error as CastError, Response};
use smol::Timer;
use smol::channel::Receiver;
use smol::future::{self, BoxedLocal, FutureExt};
use tracing::debug;

use super::caster::{Caster, Conn};
use crate::backend::PlayerCmd;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const SEND_TIMEOUT: Duration = Duration::from_secs(5);

pub async fn connect(ip: IpAddr) -> Result<Client> {
    let ip = ip.to_string();
    let connect = async { Ok(Client::connect(&ip).await?) };
    within(CONNECT_TIMEOUT, anyhow!("timed out"), connect).await
}

async fn within<T, E>(
    limit: Duration,
    timed_out: E,
    f: impl Future<Output = Result<T, E>>,
) -> Result<T, E> {
    let timer = async {
        Timer::after(limit).await;
        Err(timed_out)
    };
    f.or(timer).await
}

struct Link {
    client: Client,
    /// kept across loop turns: dropping a read halfway would desync the stream
    reading: BoxedLocal<Result<Response, CastError>>,
}

impl Link {
    fn new(client: Client) -> Self {
        let reading = read(&client);
        Self { client, reading }
    }

    async fn next(&mut self) -> Result<Response, CastError> {
        let res = (&mut self.reading).await;
        self.reading = read(&self.client);
        res
    }
}

fn read(client: &Client) -> BoxedLocal<Result<Response, CastError>> {
    let client = client.clone();
    async move { client.receive().await }.boxed_local()
}

#[allow(clippy::large_enum_variant)]
enum Wake {
    Cmd(Option<PlayerCmd>),
    Read(Result<Response, CastError>),
    Connected(Result<Client>),
    Timer,
}

pub async fn run(mut caster: Caster, ip: IpAddr, first: Client, cmds: Receiver<PlayerCmd>) {
    let mut link = Some(Link::new(first));
    let mut connecting: Option<BoxedLocal<Result<Client>>> = None;
    caster.connected(Instant::now());
    loop {
        flush(&mut caster, &mut link).await;
        match caster.conn {
            Conn::Down => {
                link = None;
                connecting = None;
            }
            Conn::Connecting if connecting.is_none() => {
                link = None;
                connecting = Some(connect(ip).boxed_local());
            }
            _ => {}
        }

        let wake_at = caster.wake_at();
        let cmd = async { Wake::Cmd(cmds.recv().await.ok()) };
        let io = async {
            match (&mut link, &mut connecting) {
                (Some(l), _) => Wake::Read(l.next().await),
                (None, Some(c)) => Wake::Connected(c.await),
                (None, None) => future::pending().await,
            }
        };
        let timer = async {
            match wake_at {
                Some(at) => Timer::at(at).await,
                None => future::pending().await,
            };
            Wake::Timer
        };
        let wake = cmd.or(io).or(timer).await;

        let now = Instant::now();
        match wake {
            Wake::Cmd(None) => break,
            Wake::Cmd(Some(cmd)) => caster.handle(cmd, now),
            Wake::Read(Ok(msg)) => caster.receive(msg, now),
            Wake::Read(Err(e @ (CastError::Deserialize(_) | CastError::Decode(_)))) => {
                debug!("cast: skipping a message: {e}")
            }
            Wake::Read(Err(e)) => caster.lost(&format!("cast device lost: {e}"), now),
            Wake::Connected(Ok(client)) => {
                connecting = None;
                link = Some(Link::new(client));
                caster.connected(now);
            }
            Wake::Connected(Err(e)) => {
                connecting = None;
                caster.lost(&format!("can't reach the cast device: {e:#}"), now);
            }
            Wake::Timer => caster.tick(now),
        }
    }
    caster.shutdown();
    flush(&mut caster, &mut link).await;
}

async fn flush(caster: &mut Caster, link: &mut Option<Link>) {
    let out = std::mem::take(&mut caster.outbox);
    let Some(l) = link else { return };
    for (to, payload, request_id) in out {
        let send = l.client.send(to, payload, request_id);
        if let Err(e) = within(SEND_TIMEOUT, CastError::ResponseTimeout, send).await {
            *link = None;
            return caster.lost(&format!("cast device lost: {e}"), Instant::now());
        }
    }
}
