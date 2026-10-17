//! Packet decoding and accurate seeks reuse the client's existing Symphonia codecs.
use std::{fs::File, path::Path};
use symphonia::core::{
    audio::{Channels, SampleBuffer},
    codecs::{CODEC_TYPE_NULL, Decoder, DecoderOptions},
    errors::Error,
    formats::{FormatReader, SeekMode, SeekTo},
    io::MediaSourceStream,
    units::{Time, TimeBase},
};

pub(super) struct Source {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track: u32,
    time_base: TimeBase,
    skip_until: u64,
    pub rate: u32,
    pub channels: usize,
    pub duration_ms: i64,
}

impl Source {
    pub fn open(path: &Path) -> Result<Self, String> {
        let stream = MediaSourceStream::new(
            Box::new(File::open(path).map_err(|e| e.to_string())?),
            Default::default(),
        );
        let mut hint = symphonia::core::probe::Hint::new();
        if let Some(extension) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(extension);
        }
        let format = symphonia::default::get_probe()
            .format(&hint, stream, &Default::default(), &Default::default())
            .map_err(|e| e.to_string())?
            .format;
        let (track, params, decoder) = format
            .tracks()
            .iter()
            .find_map(|track| {
                if track.codec_params.codec == CODEC_TYPE_NULL {
                    return None;
                }
                let decoder = symphonia::default::get_codecs()
                    .make(&track.codec_params, &DecoderOptions::default())
                    .ok()?;
                Some((track.id, track.codec_params.clone(), decoder))
            })
            .ok_or("Playback is unavailable for this file.")?;
        let rate = params
            .sample_rate
            .filter(|rate| *rate > 0)
            .ok_or("Unknown audio sample rate")?;
        let channels = params
            .channels
            .map(|channels| channels.count())
            .filter(|count| *count > 0)
            .ok_or("Unknown audio channels")?;
        let time_base = params.time_base.unwrap_or(TimeBase::new(1, rate));
        let duration_ms = params
            .n_frames
            .map(|frames| {
                let time = time_base.calc_time(frames);
                (time.seconds as f64 * 1000.0 + time.frac * 1000.0) as i64
            })
            .unwrap_or(0);
        Ok(Self {
            format,
            decoder,
            track,
            time_base,
            skip_until: 0,
            rate,
            channels,
            duration_ms,
        })
    }
    pub fn seek(&mut self, ms: i64) -> Result<(), String> {
        let secs = ms.max(0) as f64 / 1000.0;
        let result = self
            .format
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time: Time::new(secs as u64, secs.fract()),
                    track_id: Some(self.track),
                },
            )
            .map_err(|e| e.to_string())?;
        self.skip_until = result.required_ts;
        self.decoder.reset();
        Ok(())
    }
    pub fn read(&mut self, enabled: &[bool]) -> Result<Option<Vec<[f32; 2]>>, String> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(packet) => packet,
                Err(Error::IoError(error)) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                    return Ok(None);
                }
                Err(error) => return Err(error.to_string()),
            };
            if packet.track_id() != self.track {
                continue;
            }
            let decoded = match self.decoder.decode(&packet) {
                Ok(decoded) => decoded,
                Err(Error::DecodeError(_)) => continue,
                Err(error) => return Err(error.to_string()),
            };
            let spec = *decoded.spec();
            if spec.rate != self.rate || spec.channels.count() != self.channels {
                return Err("Audio format changed during playback".into());
            }
            let mut samples = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
            samples.copy_interleaved_ref(decoded);
            let delta = self.skip_until.saturating_sub(packet.ts());
            let skip_time = self.time_base.calc_time(delta);
            let skip =
                ((skip_time.seconds as f64 + skip_time.frac) * self.rate as f64).round() as usize;
            let weights: Vec<_> = spec
                .channels
                .iter()
                .enumerate()
                .map(|(index, channel)| {
                    if enabled.get(index) == Some(&false) {
                        [0.0, 0.0]
                    } else if self.channels == 1 {
                        [1.0, 1.0]
                    } else if channel == Channels::FRONT_LEFT {
                        [1.0, 0.0]
                    } else if channel == Channels::FRONT_RIGHT {
                        [0.0, 1.0]
                    } else {
                        [0.5, 0.5]
                    }
                })
                .collect();
            let frames = samples
                .samples()
                .chunks_exact(self.channels)
                .skip(skip)
                .map(|frame| {
                    let mut stereo = [0.0f32; 2];
                    for (sample, weight) in frame.iter().zip(&weights) {
                        stereo[0] += sample * weight[0];
                        stereo[1] += sample * weight[1];
                    }
                    [stereo[0].clamp(-1.0, 1.0), stereo[1].clamp(-1.0, 1.0)]
                })
                .collect();
            return Ok(Some(frames));
        }
    }
}
