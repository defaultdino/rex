use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::*};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, Stream, StreamConfig};
use rtrb::{Consumer, Producer, RingBuffer};
use tracing::{debug, error, info};

#[derive(Default)]
struct Shared {
    played: AtomicU64,
    gain: AtomicU32,
    paused: AtomicBool,
    flush: AtomicBool,
    failed: AtomicBool,
}

/// The device stream plus the producer
/// side of its ~0.5 s stereo ring buffer
pub struct Output {
    stream: Stream,
    producer: Producer<f32>,
    shared: Arc<Shared>,
    pub rate: u32,
    capacity: usize,
    running: bool,
}

impl Output {
    pub fn open(volume: f32) -> Result<Self> {
        let device = cpal::default_host()
            .default_output_device()
            .context("no default audio output device")?;
        let config = device.default_output_config()?;
        info!("audio output: {config:?}");
        let rate = config.sample_rate();
        let capacity = rate as usize;
        let (producer, consumer) = RingBuffer::new(capacity);
        let shared = Arc::new(Shared::default());
        let format = config.sample_format();
        let config: StreamConfig = config.into();
        let stream = match format {
            SampleFormat::F32 => build::<f32>(&device, &config, consumer, &shared),
            SampleFormat::I16 => build::<i16>(&device, &config, consumer, &shared),
            SampleFormat::I32 => build::<i32>(&device, &config, consumer, &shared),
            SampleFormat::U16 => build::<u16>(&device, &config, consumer, &shared),
            f => bail!("unsupported device sample format {f}"),
        }?;
        stream.play()?;
        let out = Self {
            stream,
            producer,
            shared,
            rate,
            capacity,
            running: true,
        };
        out.set_volume(volume);
        Ok(out)
    }

    pub fn set_volume(&self, v: f32) {
        self.shared
            .gain
            .store(perceptual_gain(v).to_bits(), Relaxed);
    }

    pub fn set_paused(&self, paused: bool) {
        self.shared.paused.store(paused, Relaxed);
    }

    /// Frames handed to the device since the stream opened, excluding flushed audio
    pub fn played(&self) -> u64 {
        self.shared.played.load(Acquire)
    }

    /// the device reported a fatal stream error (e.g. it was unplugged)
    pub fn failed(&self) -> bool {
        self.shared.failed.load(Relaxed)
    }

    /// Free space in samples
    pub fn free(&self) -> usize {
        self.producer.slots()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Writes whole stereo frames
    /// Returns how many samples were taken
    pub fn write(&mut self, samples: &[f32]) -> usize {
        let n = self.producer.slots().min(samples.len()) & !1;
        if n == 0 {
            return 0;
        }
        let Ok(mut chunk) = self.producer.write_chunk(n) else {
            return 0;
        };
        let (a, b) = chunk.as_mut_slices();
        a.copy_from_slice(&samples[..a.len()]);
        b.copy_from_slice(&samples[a.len()..n]);
        chunk.commit_all();
        n
    }

    /// Discards everything queued in the ring buffer
    pub fn flush(&mut self) {
        if !self.running {
            return;
        }
        self.shared.flush.store(true, Release);
        let deadline = Instant::now() + Duration::from_millis(200);
        while self.shared.flush.load(Acquire) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
    }

    /// Stops the device callback while idle; only call with an empty ring buffer
    pub fn suspend(&mut self) {
        if self.running && self.stream.pause().is_ok() {
            self.running = false;
        }
    }

    pub fn resume(&mut self) {
        if !self.running && self.stream.play().is_ok() {
            self.running = true;
        }
    }
}

fn build<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    mut consumer: Consumer<f32>,
    shared: &Arc<Shared>,
) -> Result<Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;
    let sh = shared.clone();
    let failed = shared.clone();
    let stream = device.build_output_stream(
        *config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            if sh.flush.load(Acquire) {
                if let Ok(c) = consumer.read_chunk(consumer.slots()) {
                    c.commit_all();
                }
                sh.flush.store(false, Release);
            }
            let mut done = 0;
            if !sh.paused.load(Relaxed) {
                let gain = f32::from_bits(sh.gain.load(Relaxed));
                let frames = (consumer.slots() / 2).min(data.len() / channels);
                if let Ok(chunk) = consumer.read_chunk(frames * 2) {
                    let (a, b) = chunk.as_slices();
                    let mut src = a.iter().chain(b);
                    for frame in data.chunks_exact_mut(channels).take(frames) {
                        let l = src.next().copied().unwrap_or(0.0) * gain;
                        let r = src.next().copied().unwrap_or(0.0) * gain;
                        if channels == 1 {
                            frame[0] = T::from_sample((l + r) * 0.5);
                        } else {
                            frame[0] = T::from_sample(l);
                            frame[1] = T::from_sample(r);
                            frame[2..].fill(T::EQUILIBRIUM);
                        }
                    }
                    chunk.commit_all();
                    done = frames * channels;
                    sh.played.fetch_add(frames as u64, Release);
                }
            }
            data[done..].fill(T::EQUILIBRIUM);
        },
        move |e: cpal::Error| match e.kind() {
            cpal::ErrorKind::Xrun | cpal::ErrorKind::RealtimeDenied => debug!("audio: {e}"),
            _ => {
                error!("audio stream error: {e}");
                failed.failed.store(true, Relaxed);
            }
        },
        None,
    )?;
    Ok(stream)
}

fn perceptual_gain(volume: f32) -> f32 {
    let v = volume.clamp(0.0, 1.0);
    v * v * v
}
