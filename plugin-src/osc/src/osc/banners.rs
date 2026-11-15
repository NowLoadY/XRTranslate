//! OSC-only banner extensions. Existing banner JSON stays compatible with the host.
use super::pet::{self, Activity, Pet, PetFrame, PetSettings};
use crate::{
    i18n::{UiLanguage, tr},
    text_output::{self, SystemMetrics},
};
use serde::{Deserialize, Serialize};
use std::time::Instant;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BannerContentType {
    #[default]
    None,
    CustomText,
    SystemTime,
    CpuStatus,
    GpuStatus,
    Lyrics,
    TextPet,
}

impl BannerContentType {
    fn shared(self) -> Option<text_output::BannerContentType> {
        use text_output::BannerContentType as Shared;
        Some(match self {
            Self::None => Shared::None,
            Self::CustomText => Shared::CustomText,
            Self::SystemTime => Shared::SystemTime,
            Self::CpuStatus => Shared::CpuStatus,
            Self::GpuStatus => Shared::GpuStatus,
            Self::Lyrics => Shared::Lyrics,
            Self::TextPet => return None,
        })
    }

    pub fn label(self, language: UiLanguage) -> &'static str {
        self.shared()
            .map_or_else(|| tr(language, "Text pet"), |kind| kind.label(language))
    }

    pub fn is_live(self) -> bool {
        matches!(self, Self::Lyrics | Self::TextPet)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BannerConfig {
    pub content_type: BannerContentType,
    pub custom_text: String,
    pub show_device_name: bool,
    pub pet: PetSettings,
}

impl BannerConfig {
    fn render_standard(&self, metrics: &SystemMetrics, lyric: &str) -> String {
        self.content_type
            .shared()
            .map_or_else(String::new, |content_type| {
                text_output::BannerConfig {
                    content_type,
                    custom_text: self.custom_text.clone(),
                    show_device_name: self.show_device_name,
                }
                .render_text(metrics, lyric)
            })
    }

    pub(super) fn reserved_chars(&self, text: &str, pet_budget: usize) -> usize {
        if text.is_empty() {
            0
        } else if self.content_type == BannerContentType::TextPet {
            // A blink or paw movement must not resize the caption's scroll window.
            pet_budget
        } else {
            text.chars().count()
        }
    }
}

pub(super) fn pet_budget(configs: [&BannerConfig; 2], text_limit: usize) -> usize {
    let slots = configs
        .into_iter()
        .filter(|b| b.content_type == BannerContentType::TextPet)
        .count();
    if slots == 0 {
        0
    } else {
        (text_limit / 3 / slots).min(pet::MAX_WIDTH)
    }
}

#[derive(Default)]
pub(super) struct Banners {
    pets: [Pet; 2],
    frames: [PetFrame; 2],
}

impl Banners {
    pub fn render(
        &mut self,
        configs: [&BannerConfig; 2],
        text_limit: usize,
        metrics: &SystemMetrics,
        lyrics: [&str; 2],
        activity: Activity,
        now: Instant,
    ) -> [String; 2] {
        let budget = pet_budget(configs, text_limit);
        std::array::from_fn(|index| {
            let config = configs[index];
            if config.content_type == BannerContentType::TextPet {
                self.frames[index] = self.pets[index].frame(config.pet, activity, now, index);
                self.fit_pet(index, budget)
            } else {
                self.pets[index] = Pet::default();
                self.frames[index] = PetFrame::default();
                config.render_standard(metrics, lyrics[index])
            }
        })
    }

    pub fn fit_pet(&self, index: usize, budget: usize) -> String {
        self.frames[index].fit(budget)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_banner_settings_and_pet_choices_round_trip_independently() {
        let old = text_output::BannerConfig {
            content_type: text_output::BannerContentType::CustomText,
            custom_text: "keep me".into(),
            show_device_name: true,
        };
        let mut current: BannerConfig =
            serde_json::from_value(serde_json::to_value(&old).unwrap()).unwrap();
        assert_eq!(
            current.render_standard(&SystemMetrics::default(), ""),
            "keep me"
        );
        assert!(current.show_device_name);
        assert_eq!(current.pet, PetSettings::default());
        current.content_type = BannerContentType::TextPet;
        current.pet.mood = pet::PetMood::Shy;
        let saved: BannerConfig =
            serde_json::from_value(serde_json::to_value(&current).unwrap()).unwrap();
        assert_eq!(saved, current);
        assert_eq!(saved.custom_text, "keep me");
    }
}
