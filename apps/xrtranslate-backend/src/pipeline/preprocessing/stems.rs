//! Retain the existing separation pass, without buffering a recording in memory.
use hound::{SampleFormat, WavSpec, WavWriter};
use std::{collections::VecDeque, fs::File, io::BufWriter, path::Path};
use xrtranslate_protocol::SeparatedAudio;

type Writer = WavWriter<BufWriter<File>>;

pub(super) struct StemWriter {
    vocals: Writer,
    background: Writer,
    // Drop incomplete outputs on cancellation, reconfiguration or any write error.
    directory: tempfile::TempDir,
    source: VecDeque<f32>,
}

impl StemWriter {
    pub(super) fn new(root: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
        let directory = tempfile::Builder::new()
            .prefix("stems-")
            .tempdir_in(root)
            .map_err(|e| e.to_string())?;
        let spec = WavSpec {
            channels: 2,
            sample_rate: xrtranslate_separation::SAMPLE_RATE_HZ,
            bits_per_sample: 32,
            sample_format: SampleFormat::Float,
        };
        let vocals = WavWriter::create(directory.path().join("vocals.wav"), spec)
            .map_err(|e| e.to_string())?;
        let background = WavWriter::create(directory.path().join("background.wav"), spec)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            vocals,
            background,
            directory,
            source: VecDeque::new(),
        })
    }

    pub(super) fn push_source(&mut self, stereo: &[f32]) {
        self.source.extend(stereo);
    }

    pub(super) fn write(&mut self, vocals: &[f32]) -> Result<(), String> {
        if vocals.len() > self.source.len() {
            return Err("Separated audio exceeded its source duration".into());
        }
        for &vocal in vocals {
            let original = self.source.pop_front().unwrap();
            self.vocals.write_sample(vocal).map_err(|e| e.to_string())?;
            self.background
                .write_sample(original - vocal)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub(super) fn finish(self) -> Result<SeparatedAudio, String> {
        if !self.source.is_empty() || self.vocals.len() == 0 {
            return Err("Separated audio is incomplete".into());
        }
        self.vocals.finalize().map_err(|e| e.to_string())?;
        self.background.finalize().map_err(|e| e.to_string())?;
        let result = SeparatedAudio {
            vocals: self.directory.path().join("vocals.wav"),
            background: self.directory.path().join("background.wav"),
        };
        let _ = self.directory.keep();
        Ok(result)
    }
}
