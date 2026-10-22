use anyhow::{Context, Result, bail};
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{Time, TimeBase};
use tracing::warn;

pub struct Decoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    time_base: Option<TimeBase>,
    rate: u32,
    channels: usize,
    skip_frames: usize,
    scratch: Vec<f32>,
}

impl Decoder {
    pub fn open(source: Box<dyn MediaSource>, extension: Option<&str>) -> Result<Self> {
        let mss = MediaSourceStream::new(source, Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = extension {
            hint.with_extension(ext);
        }
        let format = symphonia::default::get_probe()
            .probe(
                &hint,
                mss,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .context("unrecognised container")?;
        let track = format
            .default_track(TrackType::Audio)
            .context("no audio track")?;
        let params = track
            .codec_params
            .as_ref()
            .and_then(|p| p.audio())
            .context("no audio codec parameters")?;
        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(params, &AudioDecoderOptions::default().gapless(true))
            .context("unsupported codec")?;
        let (Some(rate), Some(channels)) = (params.sample_rate, params.channels.as_ref()) else {
            bail!("stream lacks sample rate or channel layout");
        };
        Ok(Self {
            track_id: track.id,
            time_base: track.time_base,
            rate,
            channels: channels.count(),
            format,
            decoder,
            skip_frames: 0,
            scratch: Vec::new(),
        })
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Decodes the next packet and appends it to `out` as interleaved stereo
    /// Returns `Ok(false)` at end of a stream
    pub fn next(&mut self, out: &mut Vec<f32>) -> Result<bool> {
        loop {
            let Some(packet) = self.format.next_packet()? else {
                return Ok(false);
            };
            if packet.track_id != self.track_id {
                continue;
            }
            let buf = match self.decoder.decode(&packet) {
                Ok(buf) => buf,
                Err(Error::DecodeError(e)) => {
                    warn!("skipping undecodable packet: {e}");
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            let spec = buf.spec();
            if spec.rate() != self.rate || spec.channels().count() != self.channels {
                bail!("stream format changed mid-track");
            }
            buf.copy_to_vec_interleaved(&mut self.scratch);
            let skip = (self.skip_frames * self.channels).min(self.scratch.len());
            self.skip_frames -= skip / self.channels.max(1);
            to_stereo(&self.scratch[skip..], self.channels, out);
            return Ok(true);
        }
    }

    /// Seeks to `ms` and returns the position actually reached in milliseconds
    pub fn seek(&mut self, ms: u64) -> Result<u64> {
        let seeked = self.format.seek(
            SeekMode::Accurate,
            SeekTo::Time {
                time: Time::from_millis_u64(ms),
                track_id: Some(self.track_id),
            },
        )?;
        self.decoder.reset();
        let Some(tb) = self.time_base else {
            return Ok(ms);
        };
        let required = tb.calc_time_saturating(seeked.required_ts);
        let actual = tb.calc_time_saturating(seeked.actual_ts);
        let gap = (required.as_secs_f64() - actual.as_secs_f64()).max(0.0);
        self.skip_frames = (gap * self.rate as f64).round() as usize;
        Ok(required.as_millis().max(0) as u64)
    }
}

fn to_stereo(src: &[f32], channels: usize, out: &mut Vec<f32>) {
    match channels {
        0 => {}
        1 => out.extend(src.iter().flat_map(|&s| [s, s])),
        2 => out.extend_from_slice(src),
        n => out.extend(src.chunks_exact(n).flat_map(|f| {
            let m = f.iter().sum::<f32>() / n as f32;
            [m, m]
        })),
    }
}
