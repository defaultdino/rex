use std::sync::RwLock;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde::de::DeserializeOwned;
use ureq::{Agent, RequestBuilder};

pub struct Stream {
    /// offset of the first body byte; 0 when the server ignored the range
    pub start: u64,
    /// total size of the resource, if the server said
    pub len: Option<u64>,
    pub body: ureq::BodyReader<'static>,
}

pub struct PlexClient {
    agent: Agent,
    /// kept separate from `agent` so media downloads aren't cut off by its global timeout,
    /// and long-lived so its connection pool skips a TCP/TLS handshake per track
    stream_agent: Agent,
    /// swapped in place when the server is rediscovered at another address
    base: RwLock<String>,
    token: String,
    client_id: String,
    device: String,
}

impl PlexClient {
    pub fn new(base: &str, token: &str, client_id: &str, timeout: Duration) -> Self {
        let agent = Agent::config_builder()
            .timeout_global(Some(timeout))
            .build()
            .into();
        // no per-read timeout here; the audio source restarts stalled downloads itself
        let stream_agent = Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_recv_response(Some(Duration::from_secs(15)))
            .build()
            .into();
        let device = std::fs::read_to_string("/proc/sys/kernel/hostname")
            .map(|h| h.trim().to_owned())
            .unwrap_or_else(|_| "rex".into());
        Self {
            agent,
            stream_agent,
            base: RwLock::new(base.trim_end_matches('/').to_owned()),
            token: token.to_owned(),
            client_id: client_id.to_owned(),
            device,
        }
    }

    pub fn base(&self) -> String {
        self.base
            .read()
            .map_or_else(|e| e.into_inner().clone(), |b| b.clone())
    }

    pub fn set_base(&self, url: &str) {
        let url = url.trim_end_matches('/').to_owned();
        match self.base.write() {
            Ok(mut b) => *b = url,
            Err(e) => *e.into_inner() = url,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base())
    }

    fn headers<B>(&self, mut r: RequestBuilder<B>, query: &[(&str, &str)]) -> RequestBuilder<B> {
        r = r
            .header("Accept", "application/json")
            .header("X-Plex-Product", "rex")
            .header("X-Plex-Version", env!("CARGO_PKG_VERSION"))
            .header("X-Plex-Client-Identifier", &self.client_id)
            .header("X-Plex-Platform", "Linux")
            .header("X-Plex-Device-Name", &self.device);
        if !self.token.is_empty() {
            r = r.header("X-Plex-Token", &self.token);
        }
        for (k, v) in query {
            r = r.query(*k, *v);
        }
        r
    }

    /// GET returning JSON, retried once after a second on network errors and 5xx responses
    pub fn get<T: DeserializeOwned>(&self, path: &str, query: &[(&str, &str)]) -> Result<T> {
        self.get_json(path, query, None, true)
    }

    /// like `get` but without the retry, for probes that must fail fast
    pub fn get_once<T: DeserializeOwned>(&self, path: &str, query: &[(&str, &str)]) -> Result<T> {
        self.get_json(path, query, None, false)
    }

    pub fn get_range<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
        start: u32,
        size: u32,
    ) -> Result<T> {
        self.get_json(path, query, Some((start, size)), true)
    }

    fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
        range: Option<(u32, u32)>,
        retry: bool,
    ) -> Result<T> {
        let attempt = || {
            let mut req = self.headers(self.agent.get(self.url(path)), query);
            if let Some((start, size)) = range {
                req = req
                    .header("X-Plex-Container-Start", start.to_string())
                    .header("X-Plex-Container-Size", size.to_string());
            }
            req.call().and_then(|mut r| r.body_mut().read_json::<T>())
        };
        let result = match attempt() {
            Err(e) if retry && retryable(&e) => {
                thread::sleep(Duration::from_secs(1));
                attempt()
            }
            other => other,
        };
        result.map_err(|e| describe(e, &format!("GET {}", self.url(path))))
    }

    /// streaming GET for media downloads starting at byte `from`
    pub fn stream(&self, path: &str, from: u64) -> Result<Stream> {
        let mut req = self.headers(self.stream_agent.get(self.url(path)), &[]);
        if from > 0 {
            req = req.header("Range", format!("bytes={from}-"));
        }
        let resp = req
            .call()
            .map_err(|e| describe(e, &format!("GET {} from {from}", self.url(path))))?;
        let partial = resp.status().as_u16() == 206;
        // "bytes 100-199/1000" -> 1000
        let total = resp
            .headers()
            .get("content-range")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.rsplit('/').next())
            .and_then(|t| t.parse().ok());
        let body = resp.into_body();
        let start = if partial { from } else { 0 };
        Ok(Stream {
            start,
            len: total.or_else(|| body.content_length().map(|n| start + n)),
            body: body.into_reader(),
        })
    }

    /// fire-and-forget GET for endpoints whose response body we don't need
    pub fn send(&self, path: &str, query: &[(&str, &str)], timeout: Duration) -> Result<()> {
        self.headers(self.agent.get(self.url(path)), query)
            .config()
            .timeout_global(Some(timeout))
            .build()
            .call()
            .and_then(|r| r.into_body().read_to_vec())
            .map_err(|e| describe(e, &format!("GET {}", self.url(path))))?;
        Ok(())
    }

    /// absolute URL for `path` with the token in the query, for consumers that can't send headers
    pub fn url_with_token(&self, path: &str) -> String {
        let sep = if path.contains('?') { '&' } else { '?' };
        format!("{}{path}{sep}X-Plex-Token={}", self.base(), self.token)
    }

    pub fn post<T: DeserializeOwned>(&self, path: &str, query: &[(&str, &str)]) -> Result<T> {
        let req = self.headers(self.agent.post(self.url(path)), query);
        req.send_empty()
            .and_then(|mut r| r.body_mut().read_json())
            .with_context(|| format!("POST {}", self.url(path)))
    }
}

fn retryable(e: &ureq::Error) -> bool {
    match e {
        ureq::Error::StatusCode(code) => *code >= 500,
        other => is_network(other),
    }
}

fn is_network(e: &ureq::Error) -> bool {
    matches!(
        e,
        ureq::Error::Io(_)
            | ureq::Error::Timeout(_)
            | ureq::Error::HostNotFound
            | ureq::Error::ConnectionFailed
            | ureq::Error::Tls(_)
            | ureq::Error::Protocol(_)
    )
}

fn describe(e: ureq::Error, what: &str) -> anyhow::Error {
    match e {
        ureq::Error::StatusCode(401) => {
            anyhow!("the server rejected the saved token (401); run `rex login` to sign in again")
        }
        e => anyhow::Error::new(e).context(what.to_owned()),
    }
}

/// true when `e` failed because the server couldn't be reached, as opposed to an HTTP error
pub fn unreachable(e: &anyhow::Error) -> bool {
    e.chain()
        .filter_map(|c| c.downcast_ref::<ureq::Error>())
        .any(is_network)
}
