//! One voice per selected clip; catalogue refreshes never restart speech.
use xrtranslate_audio::Narration;
use xrtranslate_plugin::guide::{Audio, State};

struct Clip {
    page: &'static str,
    topic: &'static str,
    language: &'static str,
    text: &'static str,
    file: &'static str,
    duration_ms: u64,
    bytes: &'static [u8],
}

include!(concat!(env!("OUT_DIR"), "/audio.rs"));

pub(crate) fn metadata(page: &str, topic: &str, language: &str, text: &str) -> Option<Audio> {
    let language = if ["en", "zh", "ja", "ko", "ru"].contains(&language) {
        language
    } else {
        "en"
    };
    CLIPS
        .iter()
        .find(|clip| {
            clip.page == page
                && clip.topic == topic
                && clip.language == language
                && clip.text == text
        })
        .map(|clip| Audio {
            file: clip.file.into(),
            duration_ms: clip.duration_ms,
        })
}

#[derive(Default)]
pub struct Narrator {
    cue: Cue,
    playback: Narration,
}

#[derive(Default)]
struct Cue {
    file: Option<String>,
}

impl Cue {
    fn update(&mut self, state: &State) -> (bool, bool) {
        let file = state
            .session
            .as_ref()
            .and_then(|s| s.current())
            .and_then(|(_, step)| step.audio.as_ref())
            .map(|a| a.file.as_str());
        let changed = self.file.as_deref() != file;
        if changed {
            self.file = file.map(str::to_owned);
        }
        (changed, changed && file.is_some())
    }
}

impl Narrator {
    pub fn transition(&mut self) {
        self.playback.transition();
    }

    pub fn sync(&mut self, state: &State) {
        if let Some(error) = self.playback.update() {
            eprintln!("Guide narration: {error}");
        }
        let (changed, start) = self.cue.update(state);
        if changed {
            self.transition();
            if state.session.is_none() {
                self.playback.stop();
            }
        }
        if !start {
            return;
        }
        let audio = state
            .session
            .as_ref()
            .and_then(|s| s.current())
            .and_then(|(_, s)| s.audio.as_ref());
        let Some(clip) = audio.and_then(|a| CLIPS.iter().find(|clip| clip.file == a.file)) else {
            return;
        };
        if let Err(error) = self.playback.play(clip.bytes) {
            eprintln!("Guide narration: {error}");
        }
    }

    pub fn stop(&mut self) {
        self.playback.stop();
        self.cue = Cue::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xrtranslate_audio::AudioPlayer;
    use xrtranslate_plugin::guide::{Chapter, Session, Step};

    #[test]
    fn narration_starts_on_selection_and_never_auto_repeats() {
        let mut cue = Cue::default();
        let mut state = State {
            revision: 1,
            session: Some(Session {
                chapters: vec![Chapter {
                    page: "Translation".into(),
                    label: "Translation".into(),
                    steps: vec![Step {
                        id: "translation".into(),
                        title: String::new(),
                        text: String::new(),
                        targets: vec![],
                        reveal: vec![],
                        audio: Some(Audio {
                            file: "en/translation.mp3".into(),
                            duration_ms: 1000,
                        }),
                    }],
                }],
                chapter: 0,
                step: 0,
            }),
            ready: false,
        };
        assert_eq!(cue.update(&state), (true, true));
        state.ready = true;
        assert_eq!(cue.update(&state), (false, false));
        assert_eq!(cue.update(&state), (false, false));
        state.revision += 1;
        state.ready = false;
        assert_eq!(cue.update(&state), (false, false)); // A catalogue/admission refresh keeps the current voice.
        state.session.as_mut().unwrap().chapters[0].steps[0]
            .audio
            .as_mut()
            .unwrap()
            .file = "zh/translation.mp3".into();
        assert_eq!(cue.update(&state), (true, true)); // Crossfade while revealing next.
        state.session = None;
        assert_eq!(cue.update(&state), (true, false));
        state.ready = true;
        assert_eq!(cue.update(&state), (false, false)); // Late acknowledgement cannot speak after exit.
    }

    #[test]
    fn every_embedded_clip_opens_with_the_expected_duration() {
        for clip in CLIPS {
            let mut player = AudioPlayer::new().unwrap();
            player
                .load_embedded(clip.bytes)
                .unwrap_or_else(|error| panic!("{}: {error}", clip.file));
            assert!(player.get_duration_ms().abs_diff(clip.duration_ms as i64) < 200);
        }
    }
}
