//! All companion dialogue decisions live here; pages contain no dialogue logic.
use super::attention::Attention;
use crate::{
    model_install::NativeModelTaskState, plugins::PluginId, runtime_install::RuntimeInstallState,
    ui::Page,
};

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Route {
    Onboarding(usize),
    Page(Page),
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Cue {
    Hello,
    Models,
    Voice,
    VoiceEnabled,
    Resources,
    Working,
    Failed,
    Agreement,
    Ready,
    Translation,
    Audio,
    Styles,
    VoiceCards,
    Vocabulary,
    Settings,
    Osc,
    Meeting,
    Player,
    Vr,
}

impl Cue {
    pub fn text(self) -> &'static str {
        match self {
            Self::Hello => "Hi! I'm your translation companion. Let's get started.",
            Self::Models => "Let's choose how to recognize and translate your speech.",
            Self::Voice => "Voice playback is optional. Subtitles work without it, too.",
            Self::VoiceEnabled => "You've enabled voice playback. Choose a voice you like.",
            Self::Resources => "Let's get the missing resources ready. Installed files can stay.",
            Self::Working => "Your resources are being prepared. I'll wait here with you.",
            Self::Failed => "Something needs attention. Check the error below before retrying.",
            Self::Agreement => "Resources are ready. Read the usage guidelines before continuing.",
            Self::Ready => "All set! You can open translation now.",
            Self::Translation => "Select audio and start.",
            Self::Audio => "Connect audio sources to the outputs you want.",
            Self::Styles => "Pick a style card to change how your translations sound.",
            Self::VoiceCards => "Choose a voice card to use with your TTS.",
            Self::Vocabulary => "Keep names and terms consistent across translations.",
            Self::Settings => "Let's make XRTranslate feel right for you.",
            Self::Osc => "Configure chatbox output.",
            Self::Meeting => "Your meeting transcript stays together here.",
            Self::Player => "Open a video and we can follow its dialogue together.",
            Self::Vr => "Bring your subtitles into VR.",
        }
    }

    pub fn bit(self) -> u32 {
        1 << self as u32
    }
}

pub(super) struct Context {
    pub route: Route,
    pub cue: Cue,
    pub blocked: Option<&'static str>,
}

impl Context {
    pub fn read(app: &crate::XRTranslateApp, blocked: Option<&'static str>) -> Self {
        if !app.first_run {
            return Self {
                route: Route::Page(app.navigation.page),
                cue: match app.navigation.page {
                    Page::Translation => Cue::Translation,
                    Page::AudioStudio => Cue::Audio,
                    Page::PromptStudio => Cue::Styles,
                    Page::TtsCenter => Cue::VoiceCards,
                    Page::CorpusStudio => Cue::Vocabulary,
                    Page::Settings => Cue::Settings,
                    Page::Plugin(PluginId::OSC) => Cue::Osc,
                    Page::Plugin(PluginId::MEETING) => Cue::Meeting,
                    Page::Plugin(PluginId::VIDEO_PLAYER) => Cue::Player,
                    Page::Plugin(PluginId::VR_OVERLAY) => Cue::Vr,
                    Page::Plugin(_) => Cue::Translation,
                },
                blocked: None,
            };
        }
        let voice = app.service_config.tts_is_configured();
        let cue = match app.onboarding_page {
            0 => Cue::Hello,
            1 => Cue::Models,
            2 if voice => Cue::VoiceEnabled,
            2 => Cue::Voice,
            _ if app.model_task_manager.is_busy() || app.runtime_installer.is_busy() => {
                Cue::Working
            }
            _ if matches!(
                app.model_task_manager.state(),
                NativeModelTaskState::Failed(_)
            ) || matches!(
                app.runtime_installer.state(),
                RuntimeInstallState::Failed(_)
            ) =>
            {
                Cue::Failed
            }
            _ if blocked.is_some() => Cue::Resources,
            _ if voice && !app.usage_guidelines_accepted => Cue::Agreement,
            _ => Cue::Ready,
        };
        Self {
            route: Route::Onboarding(app.onboarding_page),
            cue,
            blocked,
        }
    }

    pub fn reply(&self, attention: Attention) -> &'static str {
        match attention {
            Attention::Feature(0) => "I can listen to your microphone or computer audio.",
            Attention::Feature(1) => "We can translate locally or use a cloud service.",
            Attention::Feature(_) => "Your subtitles can come along into VRChat, too!",
            Attention::Avatar if self.route == Route::Onboarding(0) => "Hehe, I'm right here!",
            Attention::Avatar | Attention::Control(_) => self.cue.text(),
            Attention::Next => self.blocked.unwrap_or(match self.route {
                Route::Onboarding(0) => "Ready? Let's choose your models.",
                Route::Onboarding(1) => Cue::Voice.text(),
                Route::Onboarding(2) => "Next, let's check the resources you need.",
                _ => self.cue.text(),
            }),
        }
    }
}
