//! Bounded waveform summaries, read once off the UI thread for the active source.
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

pub(super) struct Waveform {
    pub duration_ms: i64,
    pub peaks: Vec<f32>,
}

impl Waveform {
    pub fn peak(&self, start_ms: f64, end_ms: f64) -> f32 {
        let index = |time: f64| {
            (time.max(0.0) / self.duration_ms.max(1) as f64 * self.peaks.len() as f64) as usize
        };
        let start = index(start_ms).min(self.peaks.len());
        let end = index(end_ms).saturating_add(1).min(self.peaks.len());
        self.peaks[start..end.max(start)]
            .iter()
            .copied()
            .fold(0.0, f32::max)
    }
}

#[derive(Default)]
pub(super) struct Cache {
    path: Option<PathBuf>,
    pub waveform: Option<Waveform>,
    pub error: Option<String>,
    pending: Option<mpsc::Receiver<Result<Waveform, String>>>,
    cancelled: Arc<AtomicBool>,
}

impl Drop for Cache {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

impl Cache {
    pub fn update(&mut self, path: Option<&Path>, ctx: &eframe::egui::Context) {
        if self.path.as_deref() != path {
            self.cancelled.store(true, Ordering::Relaxed);
            *self = Self::default();
            self.path = path.map(Path::to_owned);
            if let Some(path) = path {
                let (send, receive) = mpsc::channel();
                let (path, cancel, ctx) = (path.to_owned(), self.cancelled.clone(), ctx.clone());
                match std::thread::Builder::new()
                    .name("media-waveform".into())
                    .spawn(move || {
                        let result = read(&path, &cancel);
                        let _ = send.send(result);
                        ctx.request_repaint();
                    }) {
                    Ok(_) => self.pending = Some(receive),
                    Err(error) => self.error = Some(error.to_string()),
                }
            }
        }
        if let Some(pending) = &self.pending {
            match pending.try_recv() {
                Ok(result) => {
                    self.pending = None;
                    match result {
                        Ok(waveform) => self.waveform = Some(waveform),
                        Err(error) => self.error = Some(error),
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.pending = None;
                    self.error = Some("Could not read the vocal waveform.".into());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
    }

    pub fn loading(&self) -> bool {
        self.pending.is_some()
    }
}

fn read(path: &Path, cancelled: &AtomicBool) -> Result<Waveform, String> {
    let mut reader = hound::WavReader::open(path).map_err(|e| e.to_string())?;
    let spec = reader.spec();
    if spec.sample_rate == 0 || spec.channels == 0 || reader.duration() == 0 {
        return Err("Empty vocal track.".into());
    }
    let frames = u64::from(reader.duration());
    // About 20 ms per bin for typical clips; memory stays bounded for long films.
    let per_bin = frames
        .div_ceil(65_536)
        .max(u64::from(spec.sample_rate) / 50)
        .max(1);
    let mut peaks = vec![0.0_f32; frames.div_ceil(per_bin) as usize];
    let samples_per_bin = per_bin * u64::from(spec.channels);
    let mut push = |index: usize, value: f32| -> Result<(), String> {
        if index % 65_536 == 0 && cancelled.load(Ordering::Relaxed) {
            return Err("Cancelled".into());
        }
        if value.is_finite() {
            let bin = index as u64 / samples_per_bin;
            if let Some(peak) = peaks.get_mut(bin as usize) {
                *peak = peak.max(value.abs());
            }
        }
        Ok(())
    };
    match spec.sample_format {
        hound::SampleFormat::Float => {
            for (index, sample) in reader.samples::<f32>().enumerate() {
                push(index, sample.map_err(|e| e.to_string())?)?;
            }
        }
        hound::SampleFormat::Int => {
            let scale = 2.0_f32.powi(i32::from(spec.bits_per_sample) - 1);
            for (index, sample) in reader.samples::<i32>().enumerate() {
                push(index, sample.map_err(|e| e.to_string())? as f32 / scale)?;
            }
        }
    }
    Ok(Waveform {
        duration_ms: (frames * 1000 / u64::from(spec.sample_rate)) as i64,
        peaks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn waveform_retains_both_channels_and_silence_and_can_cancel() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("vocals.wav");
        let mut writer = hound::WavWriter::create(
            &path,
            hound::WavSpec {
                channels: 2,
                sample_rate: 16_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for frame in 0..32_000 {
            writer.write_sample(0_i16).unwrap();
            writer
                .write_sample(if frame < 16_000 { 16_384_i16 } else { 0 })
                .unwrap();
        }
        writer.finalize().unwrap();
        let waveform = read(&path, &AtomicBool::new(false)).unwrap();
        assert_eq!(waveform.duration_ms, 2000);
        assert!(waveform.peaks.len() <= 65_536);
        assert_eq!(waveform.peak(0.0, 500.0), 0.5);
        assert_eq!(waveform.peak(1100.0, 2000.0), 0.0);
        assert_eq!(waveform.peak(3000.0, 4000.0), 0.0);
        assert!(read(&path, &AtomicBool::new(true)).is_err());
    }
}
