//! Short bundled speech with immediate selection and a brief outgoing fade.
use crate::{AudioPlayer, PlaybackStatus};
use std::time::Duration;

#[derive(Default)]
pub struct Narration {
    current: Option<AudioPlayer>,
    outgoing: Option<AudioPlayer>,
    paused: bool,
}

impl Narration {
    pub fn play(&mut self, bytes: &'static [u8]) -> Result<(), String> {
        self.transition();
        let mut player = AudioPlayer::new()?;
        player.load_embedded(bytes)?;
        if !self.paused {
            player.play();
        }
        self.current = Some(player);
        Ok(())
    }

    pub fn transition(&mut self) {
        if let Some(mut player) = self.current.take() {
            if player.get_status() == PlaybackStatus::Playing {
                player.fade_out(Duration::from_millis(180));
                self.outgoing = Some(player);
            }
        }
    }

    pub fn set_paused(&mut self, paused: bool) {
        if self.paused == paused {
            return;
        }
        self.paused = paused;
        if let Some(player) = &mut self.current {
            if paused {
                player.pause();
            } else if player.get_status() == PlaybackStatus::Paused || player.get_time_ms() == 0 {
                player.play();
            }
        }
    }

    pub fn update(&mut self) -> Option<String> {
        if self
            .outgoing
            .as_ref()
            .is_some_and(|p| p.get_status() == PlaybackStatus::Stopped)
        {
            self.outgoing = None;
        }
        self.current.as_mut().and_then(AudioPlayer::take_error)
    }

    pub fn stop(&mut self) {
        self.current = None;
        self.outgoing = None;
    }
}
