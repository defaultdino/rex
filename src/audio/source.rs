use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use symphonia::core::io::MediaSource;
use tracing::{debug, warn};

use crate::plex::PlexClient;

const BUFFERING_AFTER: Duration = Duration::from_millis(200);
/// reads this close ahead of the active download wait for it instead of opening a new range
const JUMP_AHEAD: u64 = 256 * 1024;
/// a blocked read with no download progress for this long restarts the download
const STALL_AFTER: Duration = Duration::from_secs(10);
/// a read blocked this long without progress fails, so a decode thread is never stuck for good
const GIVE_UP_AFTER: Duration = Duration::from_secs(30);
/// network errors tolerated in a row (2 + 4 + 8 + 8 + 8 s of backoff) before giving up
const RETRIES: u32 = 5;

/// sorted, non-overlapping, non-adjacent `[start, end)` byte ranges present in the cache file
#[derive(Default)]
struct Ranges(Vec<(u64, u64)>);

impl Ranges {
    fn insert(&mut self, start: u64, end: u64) {
        self.0.push((start, end));
        self.0.sort_unstable();
        let mut merged: Vec<(u64, u64)> = Vec::with_capacity(self.0.len());
        for &(s, e) in &self.0 {
            match merged.last_mut() {
                Some(last) if s <= last.1 => last.1 = last.1.max(e),
                _ => merged.push((s, e)),
            }
        }
        self.0 = merged;
    }

    /// end of the range containing `pos`
    fn end_of(&self, pos: u64) -> Option<u64> {
        self.0
            .iter()
            .find(|&&(s, e)| s <= pos && pos < e)
            .map(|&(_, e)| e)
    }

    /// first missing byte at or after `from`
    fn first_gap(&self, from: u64, len: u64) -> Option<u64> {
        let mut pos = from;
        while let Some(end) = self.end_of(pos) {
            pos = end;
        }
        (pos < len).then_some(pos)
    }
}

struct State {
    ranges: Ranges,
    len: Option<u64>,
    started: bool,
    /// every byte is present, or the length is unknown and the download hit EOF
    finished: bool,
    error: Option<String>,
    cancelled: bool,
    /// where the active request writes next
    cursor: u64,
    /// a position a reader is blocked on that the active request won't reach soon
    want: Option<u64>,
    /// the server ignored a range request, so only a single linear download works
    linear: bool,
    /// bumped whenever a fresh fetch thread takes over; older threads exit without writing
    generation: u64,
    last_progress: Instant,
    restarted_at: Option<Instant>,
}

struct Fetcher {
    client: Arc<PlexClient>,
    key: String,
    path: PathBuf,
    state: Mutex<State>,
    cv: Condvar,
}

/// a seekable `MediaSource` over a sparse cache file that a fetch thread fills,
/// jumping with HTTP range requests to wherever the decoder needs data
pub struct CachedHttpSource {
    fetcher: Arc<Fetcher>,
    file: File,
    pos: u64,
    len: Option<u64>,
    on_buffering: Arc<dyn Fn(bool) + Send + Sync>,
}

impl CachedHttpSource {
    /// starts downloading `part_key` into `path` and returns once response headers arrived
    pub fn open(
        client: &Arc<PlexClient>,
        part_key: &str,
        path: PathBuf,
        on_buffering: Arc<dyn Fn(bool) + Send + Sync>,
    ) -> Result<Self> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        File::create(&path)?;
        let file = File::open(&path)?;
        let fetcher = Arc::new(Fetcher {
            client: client.clone(),
            key: part_key.to_owned(),
            path,
            state: Mutex::new(State {
                ranges: Ranges::default(),
                len: None,
                started: false,
                finished: false,
                error: None,
                cancelled: false,
                cursor: 0,
                want: None,
                linear: false,
                generation: 0,
                last_progress: Instant::now(),
                restarted_at: None,
            }),
            cv: Condvar::new(),
        });
        let generation = fetcher.lock().generation;
        fetcher.spawn(generation, 0)?;

        let st = fetcher
            .cv
            .wait_while(fetcher.lock(), |s| !s.started)
            .unwrap_or_else(|e| e.into_inner());
        if let Some(e) = &st.error {
            bail!("{e}");
        }
        let len = st.len;
        drop(st);
        Ok(Self {
            fetcher,
            file,
            pos: 0,
            len,
            on_buffering,
        })
    }
}

impl Fetcher {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn spawn(self: &Arc<Self>, generation: u64, from: u64) -> io::Result<()> {
        let f = self.clone();
        thread::Builder::new().name("fetch".into()).spawn(move || {
            let result = f.fetch(generation, from);
            let mut st = f.lock();
            if st.generation != generation {
                return;
            }
            st.started = true;
            match result {
                Ok(()) => st.finished = !st.cancelled,
                Err(e) => {
                    warn!("download {}: {e:#}", f.key);
                    st.error = Some(format!("{e:#}"));
                }
            }
            f.cv.notify_all();
        })?;
        Ok(())
    }

    /// hands the download to a new thread starting at the first missing byte from `pos`
    fn restart(self: &Arc<Self>, st: &mut State, pos: u64) {
        st.generation += 1;
        st.restarted_at = Some(Instant::now());
        let from = match st.len {
            Some(len) if !st.linear => st.ranges.first_gap(pos, len).unwrap_or(pos),
            _ => 0,
        };
        warn!("download {} stalled, restarting at byte {from}", self.key);
        if let Err(e) = self.spawn(st.generation, from) {
            st.error = Some(format!("restarting download: {e}"));
        }
    }

    /// true once this thread should stop: the source is gone or a newer thread took over
    fn stale(&self, generation: u64) -> bool {
        let st = self.lock();
        st.cancelled || st.generation != generation
    }

    /// waits before retrying after a network error; false once retries are used up
    fn backoff(&self, generation: u64, attempts: &mut u32, e: &anyhow::Error) -> bool {
        *attempts += 1;
        if *attempts > RETRIES || !self.lock().started {
            return false;
        }
        let delay = Duration::from_secs(2u64.pow(*attempts).min(8));
        warn!("download {}: {e:#}; retrying in {delay:?}", self.key);
        let until = Instant::now() + delay;
        while Instant::now() < until {
            if self.stale(generation) {
                return false;
            }
            thread::sleep(Duration::from_millis(100));
        }
        true
    }

    /// downloads until every byte is present, restarting at a wanted offset when a reader jumps
    fn fetch(&self, generation: u64, mut from: u64) -> Result<()> {
        let mut out = OpenOptions::new().write(true).open(&self.path)?;
        let mut buf = vec![0u8; 64 * 1024];
        let mut attempts = 0;
        loop {
            if self.stale(generation) {
                return Ok(());
            }
            let mut stream = match self.client.stream(&self.key, from) {
                Ok(s) => s,
                Err(e) if self.backoff(generation, &mut attempts, &e) => continue,
                Err(e) => return Err(e),
            };
            let mut pos = stream.start;
            {
                let mut st = self.lock();
                if stream.start != from {
                    debug!(
                        "download {}: server ignored range request, falling back to linear",
                        self.key
                    );
                    st.linear = true;
                    st.ranges = Ranges::default();
                }
                if st.len.is_none() {
                    st.len = stream.len;
                    if let Some(len) = stream.len {
                        // sparse on Linux, so unfetched ranges cost no disk
                        out.set_len(len)?;
                    }
                }
                st.started = true;
                st.cursor = pos;
                self.cv.notify_all();
            }
            out.seek(SeekFrom::Start(pos))?;

            let jump = loop {
                let n = match stream.body.read(&mut buf) {
                    Ok(n) => n,
                    Err(e) => {
                        let e = anyhow::Error::new(e);
                        if self.backoff(generation, &mut attempts, &e) {
                            break Some(pos);
                        }
                        return Err(e);
                    }
                };
                if self.stale(generation) {
                    return Ok(());
                }
                if n == 0 {
                    break None;
                }
                out.write_all(&buf[..n])?;
                attempts = 0;
                let mut st = self.lock();
                st.ranges.insert(pos, pos + n as u64);
                pos += n as u64;
                st.cursor = pos;
                st.last_progress = Instant::now();
                self.cv.notify_all();
                if st.linear {
                    continue;
                }
                if let Some(w) = st.want.take() {
                    break Some(w);
                }
                // ran into bytes fetched earlier; move on to the next gap
                if st.ranges.end_of(pos).is_some() {
                    break Some(pos);
                }
            };

            let st = self.lock();
            let Some(len) = st.len.filter(|_| !st.linear) else {
                if jump.is_some() {
                    // a linear download can only resume from the start
                    from = 0;
                    continue;
                }
                return Ok(());
            };
            let next = jump
                .and_then(|w| st.ranges.first_gap(w, len))
                .or_else(|| st.ranges.first_gap(0, len));
            match next {
                Some(n) => {
                    debug!("download {}: continuing at byte {n}", self.key);
                    from = n;
                }
                None => return Ok(()),
            }
        }
    }
}

impl Read for CachedHttpSource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() || self.len.is_some_and(|len| self.pos >= len) {
            return Ok(0);
        }
        let f = &self.fetcher;
        let mut st = f.lock();
        let mut buffering = false;
        let mut asked = false;
        let blocked_since = Instant::now();
        let end = loop {
            if let Some(end) = st.ranges.end_of(self.pos) {
                break end;
            }
            if let Some(e) = &st.error {
                return Err(io::Error::other(e.clone()));
            }
            if st.finished {
                return Ok(0);
            }
            let near = st.cursor <= self.pos && self.pos < st.cursor + JUMP_AHEAD;
            if !asked && !near && st.len.is_some() && !st.linear {
                st.want = Some(self.pos);
                asked = true;
            }
            if blocked_since.elapsed() > GIVE_UP_AFTER && st.last_progress < blocked_since {
                if buffering {
                    (self.on_buffering)(false);
                }
                return Err(io::Error::new(io::ErrorKind::TimedOut, "download stalled"));
            }
            let stalled = st.last_progress.elapsed() > STALL_AFTER;
            if stalled && st.restarted_at.is_none_or(|t| t.elapsed() > STALL_AFTER) {
                f.restart(&mut st, self.pos);
            }
            let (guard, timeout) =
                f.cv.wait_timeout(st, BUFFERING_AFTER)
                    .unwrap_or_else(|e| e.into_inner());
            st = guard;
            if timeout.timed_out() && !buffering {
                buffering = true;
                (self.on_buffering)(true);
            }
        };
        drop(st);
        if buffering {
            (self.on_buffering)(false);
        }
        let avail = (end - self.pos).min(buf.len() as u64) as usize;
        self.file.seek(SeekFrom::Start(self.pos))?;
        let n = self.file.read(&mut buf[..avail])?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for CachedHttpSource {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let pos = match to {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
            SeekFrom::End(d) => {
                let len = self.len.or_else(|| {
                    let st = self.fetcher.lock();
                    st.finished.then(|| st.ranges.0.last().map_or(0, |r| r.1))
                });
                match len {
                    Some(len) => len.checked_add_signed(d),
                    None => return Err(io::Error::other("seek from end with unknown length")),
                }
            }
        };
        self.pos = pos.ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        Ok(self.pos)
    }
}

impl MediaSource for CachedHttpSource {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        self.len
    }
}

impl Drop for CachedHttpSource {
    fn drop(&mut self) {
        self.fetcher.lock().cancelled = true;
        if let Err(e) = fs::remove_file(&self.fetcher.path) {
            debug!("removing {}: {e}", self.fetcher.path.display());
        }
    }
}
