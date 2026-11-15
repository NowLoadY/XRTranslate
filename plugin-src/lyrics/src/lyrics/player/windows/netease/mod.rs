//! NetEase's native playback events supply the timeline missing from its SMTC.
//! Only the current player's local log is read; no window or player is modified.
mod events;
mod log;
mod process;
#[cfg(test)]
mod tests;

use super::{Playback, Track};
use std::{path::PathBuf, time::Instant};
use windows::Win32::System::SystemInformation::GetTickCount64;

#[derive(Default)]
pub(super) struct ProgressReader {
    log: Option<log::Reader>,
    process: Option<process::PlayerProcess>,
}

impl ProgressReader {
    pub fn enrich(&mut self, app: &str, playback: &mut Playback) {
        if !playback.is_music || playback.position_ms.is_some() || !is_cloudmusic(app) {
            self.clear();
            return;
        }
        if self.log.is_none() {
            let Some(directory) = std::env::var_os("LOCALAPPDATA") else {
                return;
            };
            self.log = Some(log::Reader::new(
                PathBuf::from(directory).join("NetEase/CloudMusic/cloudmusic.elog"),
            ));
        }
        let reader = self.log.as_mut().unwrap();
        if reader.refresh().is_err() {
            // Missing, replaced or unreadable logs never keep an old clock alive.
            self.clear();
            return;
        }
        let now = unsafe { GetTickCount64() };
        let Some(sample) = reader.state.sample(&playback.track, playback.playing, now) else {
            return;
        };
        if self
            .process
            .as_ref()
            .is_none_or(|process| process.pid != sample.pid || !process.accepts(sample.started))
        {
            self.process = process::PlayerProcess::open(sample.pid, now);
        }
        if !self
            .process
            .as_ref()
            .is_some_and(|process| process.accepts(sample.started))
        {
            return;
        }
        playback.position_ms = Some(sample.position_ms);
        playback.duration_ms = sample.duration_ms;
        playback.observed = Instant::now();
    }

    pub fn clear(&mut self) {
        self.log = None;
        self.process = None;
    }
}

fn is_cloudmusic(app: &str) -> bool {
    app.rsplit(['/', '\\'])
        .next()
        .is_some_and(|name| name.eq_ignore_ascii_case("cloudmusic.exe"))
}
