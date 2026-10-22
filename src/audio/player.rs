use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{
    Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError, channel, sync_channel,
};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tracing::{debug, warn};

use super::decoder::Decoder;
use super::output::Output;
use super::resample::Resampler;
use super::source::CachedHttpSource;
use crate::backend::{Channels, PlayerCmd, PlayerEvent};
use crate::plex::PlexClient;
use crate::plex::models::Track;

const POSITION_EVERY: Duration = Duration::from_millis(250);
const IDLE_POLL: Duration = Duration::from_millis(20);
const FILL_BUDGET: Duration = Duration::from_millis(50);
const DECODE_AHEAD_PACKETS: usize = 32;

pub fn spawn(client: Arc<PlexClient>, cache_dir: PathBuf, volume: f32) -> Result<Channels> {
    let (cmd_tx, cmd_rx) = sync_channel(32);
    let (ev_tx, ev_rx) = sync_channel(256);
    let (ready_tx, ready_rx) = channel();
    let _ = std::fs::remove_dir_all(&cache_dir);
    thread::Builder::new()
        .name("player".into())
        .spawn(move || {
            let out = match Output::open(volume) {
                Ok(o) => o,
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            let _ = ready_tx.send(Ok(()));
            Player {
                out,
                client,
                cache_dir,
                events: ev_tx,
                decoding: None,
                next: None,
                starved: false,
                resampler: None,
                pcm: Vec::new(),
                ready: Vec::new(),
                ready_pos: 0,
                written: 0,
                starts: VecDeque::new(),
                end_at: None,
                playing: None,
                paused: false,
                volume,
                last_position: Instant::now(),
            }
            .run(cmd_rx);
        })?;
    ready_rx.recv().context("player thread died")??;
    Ok((cmd_tx, ev_rx))
}

enum Chunk {
    Pcm(u32, Vec<f32>),
    Seeked {
        generation: u64,
        reached_ms: Result<u64, String>,
    },
    End,
    Failed(String),
}

struct DecodeThread {
    track: Arc<Track>,
    chunks: Receiver<Chunk>,
    seeks: Sender<(u64, u64)>,
    start_queued: bool,
    seek_gen: u64,
    awaiting_seek: bool,
}

impl DecodeThread {
    fn spawn(
        client: Arc<PlexClient>,
        cache_dir: PathBuf,
        track: Arc<Track>,
        events: SyncSender<PlayerEvent>,
    ) -> Self {
        let (chunk_tx, chunks) = sync_channel(DECODE_AHEAD_PACKETS);
        let (seeks, seek_rx) = channel();
        let t = track.clone();
        let spawned = thread::Builder::new()
            .name("decode".into())
            .spawn(move || decode(&client, &cache_dir, t, &events, &seek_rx, &chunk_tx));
        if let Err(e) = spawned {
            warn!("spawning decode: {e}");
        }
        Self {
            track,
            chunks,
            seeks,
            start_queued: false,
            seek_gen: 0,
            awaiting_seek: false,
        }
    }
}

fn decode(
    client: &Arc<PlexClient>,
    cache_dir: &std::path::Path,
    track: Arc<Track>,
    events: &SyncSender<PlayerEvent>,
    seeks: &Receiver<(u64, u64)>,
    out: &SyncSender<Chunk>,
) {
    let mut dec = match open(client, cache_dir, &track, events) {
        Ok(d) => d,
        Err(e) => {
            let _ = out.send(Chunk::Failed(format!("{}: {e:#}", track.title)));
            return;
        }
    };
    loop {
        if let Some((generation, ms)) = seeks.try_iter().last() {
            let reached_ms = dec.seek(ms).map_err(|e| format!("seek: {e:#}"));
            if out
                .send(Chunk::Seeked {
                    generation,
                    reached_ms,
                })
                .is_err()
            {
                return;
            }
        }
        let mut pcm = Vec::new();
        let chunk = match dec.next(&mut pcm) {
            Ok(true) => Chunk::Pcm(dec.rate(), pcm),
            Ok(false) => Chunk::End,
            Err(e) => {
                warn!("decode error in {}: {e:#}", track.title);
                Chunk::Failed(format!("{}: {e:#}", track.title))
            }
        };
        let last = !matches!(chunk, Chunk::Pcm(..));
        if out.send(chunk).is_err() || last {
            return;
        }
    }
}

struct Playing {
    track: Arc<Track>,
    start: u64,
    offset_ms: u64,
}

struct Player {
    out: Output,
    client: Arc<PlexClient>,
    cache_dir: PathBuf,
    events: SyncSender<PlayerEvent>,
    decoding: Option<DecodeThread>,
    next: Option<DecodeThread>,
    starved: bool,
    resampler: Option<Resampler>,
    pcm: Vec<f32>,
    ready: Vec<f32>,
    ready_pos: usize,
    /// Frames written into the ring buffer, on the same scale as `Output::played`
    written: u64,
    /// Tracks whose first frame is queued at the given output frame index
    starts: VecDeque<(u64, Arc<Track>)>,
    /// Output frame index at which the last decoded track finishes
    end_at: Option<u64>,
    playing: Option<Playing>,
    paused: bool,
    volume: f32,
    last_position: Instant,
}

impl Player {
    fn run(mut self, cmds: Receiver<PlayerCmd>) {
        loop {
            let cmd = if self.wants_data() && !self.starved {
                cmds.try_recv()
                    .map_err(|e| matches!(e, TryRecvError::Disconnected))
            } else if self.active() && !self.paused {
                let wait = if self.starved {
                    IDLE_POLL
                } else {
                    self.idle_wait()
                };
                cmds.recv_timeout(wait)
                    .map_err(|e| matches!(e, RecvTimeoutError::Disconnected))
            } else {
                cmds.recv().map_err(|_| true)
            };
            match cmd {
                Ok(cmd) => self.handle(cmd),
                Err(true) => return,
                Err(false) => {}
            }
            if self.out.failed() && !self.paused {
                self.recover_device();
            }
            self.fill();
            self.track_output();
        }
    }

    fn emit(&self, ev: PlayerEvent) {
        if let PlayerEvent::Position(_) = ev {
            let _ = self.events.try_send(ev);
        } else if self.events.send(ev).is_err() {
            debug!("player event receiver gone");
        }
    }

    /// reopens the default device after the current one failed; pauses if that fails too
    fn recover_device(&mut self) {
        warn!("audio output failed, reopening the default device");
        match Output::open(self.volume) {
            Ok(out) => {
                // frame indices restart at 0 on the new device, which picks up at the first
                // frame the old one never received; whatever sat in its buffer is lost
                let base = self.written;
                let rate = self.out.rate as u64;
                if let Some(p) = self.playing.as_mut() {
                    p.offset_ms += base.saturating_sub(p.start) * 1000 / rate;
                    p.start = p.start.saturating_sub(base);
                }
                for (at, _) in self.starts.iter_mut() {
                    *at = at.saturating_sub(base);
                }
                if let Some(end) = self.end_at.as_mut() {
                    *end = end.saturating_sub(base);
                }
                if out.rate != self.out.rate {
                    self.resampler = None;
                    self.ready.clear();
                    self.ready_pos = 0;
                }
                self.written = 0;
                self.out = out;
            }
            Err(e) => {
                self.paused = true;
                self.emit(PlayerEvent::Error {
                    rating_key: self.playing.as_ref().map(|p| p.track.rating_key.clone()),
                    message: format!("audio device lost: {e:#}"),
                });
                self.emit(PlayerEvent::Paused);
            }
        }
    }

    fn error(&self, rating_key: &str, message: String) {
        self.emit(PlayerEvent::Error {
            rating_key: Some(rating_key.to_owned()),
            message,
        });
    }

    fn queued_frames(&self) -> u64 {
        ((self.ready.len() - self.ready_pos) / 2) as u64
    }

    fn wants_data(&self) -> bool {
        !self.paused
            && (self.decoding.is_some() || self.ready_pos < self.ready.len())
            && self.out.free() >= self.out.capacity() / 4
    }

    fn idle_wait(&self) -> Duration {
        let frames = (self.out.capacity() / 4).saturating_sub(self.out.free()) / 2;
        Duration::from_millis(frames as u64 * 1000 / self.out.rate as u64).max(IDLE_POLL)
    }

    fn active(&self) -> bool {
        self.playing.is_some() || self.decoding.is_some() || !self.starts.is_empty()
    }

    fn handle(&mut self, cmd: PlayerCmd) {
        debug!("player cmd {cmd:?}");
        match cmd {
            PlayerCmd::Load(track) => self.load(track),
            PlayerCmd::EnqueueNext(track) => self.enqueue_next(track),
            PlayerCmd::Play => {
                self.paused = false;
                self.out.set_paused(false);
            }
            PlayerCmd::Pause => {
                self.paused = true;
                self.out.set_paused(true);
            }
            PlayerCmd::Seek(ms) => self.seek(ms),
            PlayerCmd::Stop => {
                self.reset_pipeline();
                self.next = None;
                self.out.suspend();
            }
            PlayerCmd::SetVolume(v) => {
                self.volume = v;
                self.out.set_volume(v);
            }
            PlayerCmd::ClearNext => self.next = None,
        }
    }

    fn reset_pipeline(&mut self) {
        self.decoding = None;
        self.playing = None;
        self.starts.clear();
        self.end_at = None;
        self.resampler = None;
        self.ready.clear();
        self.ready_pos = 0;
        self.out.flush();
        self.written = self.out.played();
    }

    fn spawn_decoding(&self, track: Arc<Track>) -> DecodeThread {
        DecodeThread::spawn(
            self.client.clone(),
            self.cache_dir.clone(),
            track,
            self.events.clone(),
        )
    }

    fn load(&mut self, track: Arc<Track>) {
        self.reset_pipeline();
        let next = self
            .next
            .take()
            .filter(|n| n.track.rating_key == track.rating_key);
        self.decoding = Some(next.unwrap_or_else(|| self.spawn_decoding(track)));
        self.out.resume();
        self.paused = false;
        self.out.set_paused(false);
    }

    fn enqueue_next(&mut self, track: Arc<Track>) {
        if self
            .next
            .as_ref()
            .is_some_and(|n| n.track.rating_key == track.rating_key)
        {
            return;
        }
        self.next = Some(self.spawn_decoding(track));
        // decoding already ran out (or playback fully ended): chain it in right away
        if self.decoding.is_none() {
            self.end_at = None;
            self.out.resume();
            self.advance();
        }
    }

    fn seek(&mut self, ms: u64) {
        let Some(playing) = &self.playing else { return };
        let Some(cur) = self.decoding.as_mut() else {
            return;
        };
        if !self.starts.is_empty() || cur.track.rating_key != playing.track.rating_key {
            debug!("seek ignored: decoder already moved past the playing track");
            return;
        }
        cur.seek_gen += 1;
        if cur.seeks.send((cur.seek_gen, ms)).is_err() {
            debug!("seek ignored: the track already finished decoding");
            return;
        }
        cur.awaiting_seek = true;
        if let Some(r) = self.resampler.as_mut() {
            r.reset();
        }
        self.ready.clear();
        self.ready_pos = 0;
        self.end_at = None;
        self.out.flush();
        self.written = self.out.played();
        if let Some(p) = self.playing.as_mut() {
            p.start = self.written;
            p.offset_ms = ms;
        }
        self.last_position = Instant::now()
            .checked_sub(POSITION_EVERY)
            .unwrap_or_else(Instant::now);
    }

    fn on_seeked(&mut self, generation: u64, reached_ms: Result<u64, String>) {
        let Some(cur) = self.decoding.as_mut() else {
            return;
        };
        if generation != cur.seek_gen {
            return;
        }
        cur.awaiting_seek = false;
        match reached_ms {
            Ok(ms) => {
                if let Some(p) = self.playing.as_mut() {
                    p.offset_ms = ms;
                }
            }
            Err(e) => {
                let rk = cur.track.rating_key.clone();
                self.error(&rk, e);
            }
        }
    }

    fn fill(&mut self) {
        self.starved = false;
        if self.paused {
            return;
        }
        // on a slow link the ring buffer may never fill, so yield regularly to report
        // track starts and positions and to take commands
        let deadline = Instant::now() + FILL_BUDGET;
        while Instant::now() < deadline {
            if self.ready_pos < self.ready.len() {
                let n = self.out.write(&self.ready[self.ready_pos..]);
                self.ready_pos += n;
                self.written += (n / 2) as u64;
                if self.ready_pos < self.ready.len() {
                    return;
                }
            }
            self.ready.clear();
            self.ready_pos = 0;

            let Some(cur) = self.decoding.as_mut() else {
                return;
            };
            let chunk = match cur.chunks.try_recv() {
                Ok(c) => c,
                Err(TryRecvError::Empty) => {
                    self.starved = true;
                    return;
                }
                Err(TryRecvError::Disconnected) => Chunk::Failed("decoder thread exited".into()),
            };
            let (track, awaiting_seek) = (cur.track.clone(), cur.awaiting_seek);
            match chunk {
                Chunk::Pcm(..) if awaiting_seek => {}
                Chunk::Pcm(rate, pcm) => {
                    if !cur.start_queued {
                        cur.start_queued = true;
                        self.queue_start(track.clone(), rate);
                    }
                    self.pcm = pcm;
                    if let Err(e) = self.resample(rate) {
                        self.error(&track.rating_key, format!("resampling: {e:#}"));
                        self.advance();
                    }
                }
                Chunk::Seeked {
                    generation,
                    reached_ms,
                } => self.on_seeked(generation, reached_ms),
                Chunk::End => self.advance(),
                Chunk::Failed(msg) => {
                    self.error(&track.rating_key, msg);
                    self.advance();
                }
            }
        }
    }

    fn queue_start(&mut self, track: Arc<Track>, rate: u32) {
        if self
            .resampler
            .as_ref()
            .is_some_and(|r| r.input_rate() != rate)
        {
            self.flush_resampler();
        }
        let owed = self.resampler.as_ref().map_or(0, Resampler::owed);
        let at = self.written + self.queued_frames() + owed;
        self.starts.push_back((at, track));
    }

    fn resample(&mut self, rate: u32) -> Result<()> {
        if rate == self.out.rate {
            self.ready.extend_from_slice(&self.pcm);
            return Ok(());
        }
        if self.resampler.as_ref().map(Resampler::input_rate) != Some(rate) {
            self.flush_resampler();
            self.resampler = Some(Resampler::new(rate, self.out.rate)?);
        }
        if let Some(r) = self.resampler.as_mut() {
            r.push(&self.pcm, &mut self.ready)?;
        }
        Ok(())
    }

    fn flush_resampler(&mut self) {
        if let Some(mut r) = self.resampler.take()
            && let Err(e) = r.flush(&mut self.ready)
        {
            warn!("resampler flush: {e:#}");
        }
    }

    /// chains the pre-opened next track or marks the end
    /// if the decoding track is exhausted
    fn advance(&mut self) {
        self.decoding = self.next.take();
        if self.decoding.is_none() {
            self.flush_resampler();
            self.end_at = Some(self.written + self.queued_frames());
        }
    }

    fn track_output(&mut self) {
        let played = self.out.played();
        while self.starts.front().is_some_and(|(at, _)| played >= *at) {
            let Some((at, track)) = self.starts.pop_front() else {
                break;
            };
            if self.playing.take().is_some() {
                self.emit(PlayerEvent::TrackEnded);
            }
            self.emit(PlayerEvent::TrackStarted(track.clone()));
            self.playing = Some(Playing {
                track,
                start: at,
                offset_ms: 0,
            });
        }
        if let Some(end) = self.end_at
            && played >= end
            && self.starts.is_empty()
            && self.decoding.is_none()
            && self.ready_pos >= self.ready.len()
        {
            self.end_at = None;
            if self.playing.take().is_some() {
                self.emit(PlayerEvent::TrackEnded);
            }
            self.out.suspend();
        }
        if let Some(p) = &self.playing
            && !self.paused
            && self.last_position.elapsed() >= POSITION_EVERY
        {
            let frames = played.saturating_sub(p.start);
            let ms = p.offset_ms + frames * 1000 / self.out.rate as u64;
            self.last_position = Instant::now();
            self.emit(PlayerEvent::Position(ms));
        }
    }
}

fn open(
    client: &Arc<PlexClient>,
    cache_dir: &std::path::Path,
    track: &Track,
    events: &SyncSender<PlayerEvent>,
) -> Result<Decoder> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    if track.part_key.is_empty() {
        anyhow::bail!("{} has no playable media", track.title);
    }
    let path = cache_dir.join(SEQ.fetch_add(1, Ordering::Relaxed).to_string());
    let ev = events.clone();
    let on_buffering = Arc::new(move |b: bool| {
        let _ = ev.try_send(PlayerEvent::BufferingChanged(b));
    });
    let source = CachedHttpSource::open(client, &track.part_key, path, on_buffering)?;
    Decoder::open(Box::new(source), track.container.as_deref())
        .with_context(|| format!("{} ({:?})", track.title, track.audio_codec))
}
