//! Bundled voice for the companion's fixed dialogue; authoring stays outside the client.
use super::{Guide, inbox::Message};
use crate::{
    audio_playback::Narration as Playback,
    i18n::{self, UiLanguage},
};

struct Clip {
    key: &'static str,
    language: UiLanguage,
    text: &'static str,
    duration_ms: u64,
    bytes: &'static [u8],
}

include!(concat!(env!("OUT_DIR"), "/companion_audio.rs"));

fn clip(key: &'static str, language: UiLanguage) -> Option<&'static Clip> {
    CLIPS.iter().find(|clip| {
        clip.key == key && clip.language == language && clip.text == i18n::tr(language, key)
    })
}

fn selection(guide: &Guide) -> Option<(&'static Clip, u64)> {
    if guide.last_spoken >= 0.0
        && !guide.speech.finished(guide.clock)
        && let Message::Localized(key) = guide.line
    {
        clip(key, guide.language).map(|clip| (clip, guide.last_spoken.to_bits()))
    } else {
        None
    }
}

pub(super) fn duration(line: &Message, language: UiLanguage) -> Option<f64> {
    let Message::Localized(key) = line else {
        return None;
    };
    clip(key, language).map(|clip| clip.duration_ms as f64 / 1000.0)
}

#[derive(Default)]
pub(crate) struct Narration {
    selection: Option<(&'static str, UiLanguage, u64)>,
    playback: Playback,
}

impl Narration {
    pub(super) fn sync(&mut self, guide: &Guide, playing: bool) {
        let selected = selection(guide);
        let selection = selected.map(|(clip, spoken)| (clip.key, clip.language, spoken));
        self.playback.set_paused(!playing);
        if self.selection != selection {
            self.selection = selection;
            match selected {
                Some((clip, _)) => {
                    if let Err(error) = self.playback.play(clip.bytes) {
                        log::warn!("Companion narration: {error}");
                    }
                }
                None => self.playback.stop(),
            }
        }
        if let Some(error) = self.playback.update() {
            log::warn!("Companion narration: {error}");
        }
    }

    pub(crate) fn stop(&mut self) {
        self.selection = None;
        self.playback.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::super::dialogue::{Context, Cue, Route};
    use super::*;

    #[test]
    fn every_dialogue_line_has_matching_audio_in_every_language() {
        let keys: std::collections::BTreeSet<_> = Cue::ALL
            .iter()
            .map(|cue| cue.text())
            .filter(|text| !text.is_empty())
            .collect();
        for &key in &keys {
            for language in UiLanguage::ALL {
                let clip = clip(key, language)
                    .unwrap_or_else(|| panic!("Missing or stale narration: {key} / {language:?}"));
                assert!(
                    clip.text.chars().count() <= 96,
                    "Narrated introductions must fit one bubble page"
                );
                let mut player = crate::audio_playback::AudioPlayer::new().unwrap();
                player.load_embedded(clip.bytes).unwrap();
                assert!(player.get_duration_ms().abs_diff(clip.duration_ms as i64) < 200);
            }
        }
        assert_eq!(CLIPS.len(), keys.len() * UiLanguage::ALL.len());
    }

    #[test]
    fn all_setup_pages_can_speak_and_navigation_releases_the_previous_line() {
        let scene = Context {
            hint: None,
            route: Route::Onboarding(0),
            cue: Cue::Hello,
            blocked: None,
            automatic: true,
        };
        let mut guide = Guide::new(
            0.0,
            UiLanguage::English,
            eframe::egui::Pos2::ZERO,
            &scene,
            true,
        );
        assert!(selection(&guide).is_none());
        guide.announce(Cue::Hello);
        let (english, spoken) = selection(&guide).unwrap();
        guide.clock += 0.1;
        assert_eq!(selection(&guide).unwrap().1, spoken);
        guide.language = UiLanguage::Chinese;
        let (chinese, _) = selection(&guide).unwrap();
        assert_ne!(english.text, chinese.text);
        for (page, cue) in [(1, Cue::Models), (2, Cue::Optional), (3, Cue::Resources)] {
            let scene = Context {
                route: Route::Onboarding(page),
                cue,
                hint: None,
                blocked: None,
                automatic: true,
            };
            guide.follow_scene(&scene);
            assert!(selection(&guide).is_none());
            guide.announce(cue);
            assert_eq!(selection(&guide).unwrap().0.key, cue.text());
        }
        let scene = Context {
            route: Route::Page(crate::ui::Page::Translation),
            cue: Cue::Translation,
            automatic: false,
            hint: None,
            blocked: None,
        };
        guide.follow_scene(&scene);
        assert!(selection(&guide).is_none());
        assert!(guide.pending.is_none());
        guide.say(Cue::Translation.text());
        assert_eq!(selection(&guide).unwrap().0.key, Cue::Translation.text());
        guide.say(Message::Text {
            source: "plugin".into(),
            text: "Dynamic plugin message".into(),
        });
        assert!(selection(&guide).is_none());
        guide.say(Message::Tutorial {
            revision: 1,
            text: Cue::Translation.text().into(),
        });
        assert!(
            selection(&guide).is_none(),
            "The tutorial owns its own voice"
        );
    }
}
