use super::task::PlaybackTrack;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Project mix; the player's master volume remains a listening preference.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MixSettings {
    pub original: BTreeMap<i64, u16>,
    pub vocals: u16,
    pub background: u16,
    pub speech: u16,
    pub duck_vocals: bool,
}

impl Default for MixSettings {
    fn default() -> Self {
        Self {
            original: BTreeMap::new(),
            vocals: 100,
            background: 100,
            speech: 100,
            duck_vocals: false,
        }
    }
}

impl MixSettings {
    pub fn volume(&self, track: PlaybackTrack) -> u16 {
        match track {
            PlaybackTrack::Original(id) => self
                .original
                .get(&id.unwrap_or(0))
                .or_else(|| self.original.get(&0))
                .copied()
                .unwrap_or(100),
            PlaybackTrack::Vocals => self.vocals,
            PlaybackTrack::Background => self.background,
        }
        .min(200)
    }

    pub fn set_volume(&mut self, track: PlaybackTrack, volume: u16) {
        let volume = volume.min(200);
        match track {
            PlaybackTrack::Original(id) => {
                self.original.insert(id.unwrap_or(0), volume);
            }
            PlaybackTrack::Vocals => self.vocals = volume,
            PlaybackTrack::Background => self.background = volume,
        }
    }
}
