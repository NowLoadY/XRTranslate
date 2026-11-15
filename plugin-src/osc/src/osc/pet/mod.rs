//! A small expressive companion, driven by time and caption activity.
//! No needs, scores or simulated maintenance: personality lives in its gestures.
mod poses;
#[cfg(test)]
mod tests;
pub(super) mod theater;

use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

pub const MAX_WIDTH: usize = 10;
pub const REFRESH_INTERVAL: Duration = Duration::from_millis(1200);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PetMood {
    #[default]
    Natural,
    Happy,
    Shy,
    Sleepy,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PetSettings {
    pub mood: PetMood,
}

impl PetMood {
    pub fn label(self) -> &'static str {
        match self {
            Self::Natural => "Natural mood",
            Self::Happy => "Happy",
            Self::Shy => "Shy",
            Self::Sleepy => "Sleepy",
        }
    }
}

#[derive(Clone, Copy, Default)]
pub(super) struct Activity {
    pub speaking: bool,
    pub latest_message: Option<Instant>,
    pub music: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Scene {
    #[default]
    Relaxed,
    Curious,
    Grooming,
    Stretching,
    Listening,
    Happy,
    Shy,
    Sleepy,
    Dancing,
    Waking,
}

/// Full, compact and tiny poses are all complete faces, never text slices.
#[derive(Clone, Default)]
pub(super) struct PetFrame {
    full: String,
    compact: String,
    tiny: String,
}

impl PetFrame {
    pub fn fit(&self, budget: usize) -> String {
        [&self.full, &self.compact, &self.tiny]
            .into_iter()
            .find(|text| text.chars().count() <= budget)
            .cloned()
            .unwrap_or_default()
    }
}

#[derive(Default)]
pub(super) struct Pet {
    settings: Option<PetSettings>,
    last_company: Option<Instant>,
    last_message: Option<Instant>,
    was_speaking: bool,
    happy_until: Option<Instant>,
    waking_until: Option<Instant>,
    scene: Scene,
    scene_started: Option<Instant>,
}

impl Pet {
    pub fn frame(
        &mut self,
        settings: PetSettings,
        activity: Activity,
        now: Instant,
        personality: usize,
    ) -> PetFrame {
        if self.settings != Some(settings) {
            *self = Self {
                settings: Some(settings),
                last_company: Some(now),
                ..Default::default()
            };
        }
        let new_message = activity.latest_message.is_some_and(|at| {
            self.last_message.is_none_or(|previous| at > previous)
                && now.saturating_duration_since(at) < Duration::from_secs(6)
        });
        if let Some(at) = activity.latest_message {
            self.last_message = Some(self.last_message.map_or(at, |previous| previous.max(at)));
        }
        if activity.speaking || activity.music || new_message {
            self.last_company = Some(now);
            if self.scene == Scene::Sleepy {
                self.waking_until = Some(now + Duration::from_millis(2400));
            }
        }
        if new_message || (self.was_speaking && !activity.speaking) {
            self.happy_until = Some(now + Duration::from_secs(6));
        }
        self.was_speaking = activity.speaking;
        let quiet = now.saturating_duration_since(self.last_company.unwrap_or(now));
        let scene = match settings.mood {
            PetMood::Happy => Scene::Happy,
            PetMood::Shy => Scene::Shy,
            PetMood::Sleepy => Scene::Sleepy,
            PetMood::Natural if self.waking_until.is_some_and(|until| now < until) => Scene::Waking,
            PetMood::Natural if activity.speaking => Scene::Listening,
            PetMood::Natural if self.happy_until.is_some_and(|until| now < until) => Scene::Happy,
            PetMood::Natural if activity.music => Scene::Dancing,
            PetMood::Natural => {
                // After a nap, occasionally peek out or stretch before settling again.
                let quiet = quiet.as_secs();
                if quiet >= 75 && (quiet - 75) % 120 < 100 {
                    Scene::Sleepy
                } else {
                    match (quiet / 12 + personality as u64 * 2) % 6 {
                        0 | 2 => Scene::Relaxed,
                        1 => Scene::Curious,
                        3 => Scene::Grooming,
                        4 => Scene::Stretching,
                        _ => Scene::Shy,
                    }
                }
            }
        };
        if self.scene != scene || self.scene_started.is_none() {
            self.scene = scene;
            self.scene_started = Some(now);
        }
        poses::frame(
            scene,
            now.saturating_duration_since(self.scene_started.unwrap()),
        )
    }
}
