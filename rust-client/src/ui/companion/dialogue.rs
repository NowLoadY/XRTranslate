//! All companion dialogue decisions live here; pages contain no dialogue logic.
use super::{OnboardingLayout, attention::Attention};
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
    ApiKey,
    Resources,
    Runtime,
    Checking,
    ModelDownload,
    RuntimeDownload,
    Extracting,
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
            Self::Hello => "Hi! I'm your companion. Let's get started!",
            Self::Models => "Let's pick your speech and translation models.",
            Self::Voice => "Voice is optional. Subtitles work on their own.",
            Self::VoiceEnabled => "Voice is on! Pick one you like.",
            Self::ApiKey => "Add the required API keys to continue.",
            Self::Resources => "Let's download the missing models.",
            Self::Runtime => "Install the runtime, then we're ready to go.",
            Self::Checking => "Let me check what's already here.",
            Self::ModelDownload => "Downloading models. We can resume if interrupted.",
            Self::RuntimeDownload => "Getting the runtime. I'll wait with you.",
            Self::Extracting => "Downloaded! Let's unpack the runtime.",
            Self::Working => "Getting things ready. I'll stay with you.",
            Self::Failed => "Not quite there. Check the error and try again.",
            Self::Agreement => "One last step: read and accept the guidelines.",
            Self::Ready => "All set! Let's open translation.",
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

    pub fn repeat_after_change(self) -> bool {
        matches!(
            self,
            Self::ApiKey
                | Self::Resources
                | Self::Runtime
                | Self::Failed
                | Self::Agreement
                | Self::Ready
        )
    }

    fn for_requirement(requirement: &'static str) -> Self {
        match requirement {
            "Configure every required API key to continue." => Self::ApiKey,
            "Install the runtime to continue." => Self::Runtime,
            "Wait for the current model task to finish."
            | "Wait for runtime preparation to finish." => Self::Working,
            "Please agree to the Usage Guidelines to continue." => Self::Agreement,
            _ => Self::Resources,
        }
    }

    fn preparation(model: &NativeModelTaskState, runtime: &RuntimeInstallState) -> Option<Self> {
        match (model, runtime) {
            (_, RuntimeInstallState::Extracting) => Some(Self::Extracting),
            (
                NativeModelTaskState::Installing {
                    downloaded_bytes,
                    total_bytes,
                    ..
                },
                _,
            ) => Some(if *total_bytes > 0 && downloaded_bytes >= total_bytes {
                Self::Working
            } else {
                Self::ModelDownload
            }),
            (_, RuntimeInstallState::Downloading { .. }) => Some(Self::RuntimeDownload),
            (NativeModelTaskState::Discovering, _) | (_, RuntimeInstallState::Detecting) => {
                Some(Self::Checking)
            }
            (NativeModelTaskState::Failed(_), _) | (_, RuntimeInstallState::Failed(_)) => {
                Some(Self::Failed)
            }
            _ => None,
        }
    }
}

pub(super) struct Context {
    pub route: Route,
    pub cue: Cue,
    pub blocked: Option<&'static str>,
    pub automatic: bool,
}

impl Context {
    pub fn read(app: &crate::XRTranslateApp, layout: Option<&OnboardingLayout>) -> Self {
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
                    Page::Plugin(PluginId::MEDIA) => Cue::Player,
                    Page::Plugin(PluginId::VR_OVERLAY) => Cue::Vr,
                    Page::Plugin(_) => Cue::Translation,
                },
                blocked: None,
                automatic: false,
            };
        }
        let blocked = layout.and_then(|layout| layout.requirement);
        let voice = app.service_config.tts_is_configured();
        let activity = if app.onboarding_page >= 3 {
            Cue::preparation(
                app.model_task_manager.state(),
                app.runtime_installer.state(),
            )
        } else {
            None
        };
        let cue = match app.onboarding_page {
            0 => Cue::Hello,
            1 => blocked.map_or(Cue::Models, Cue::for_requirement),
            2 => blocked.map_or(
                if voice { Cue::VoiceEnabled } else { Cue::Voice },
                Cue::for_requirement,
            ),
            _ => activity
                .or_else(|| blocked.map(Cue::for_requirement))
                .unwrap_or(if voice && !app.usage_guidelines_accepted {
                    Cue::Agreement
                } else if layout.is_some() {
                    Cue::Ready
                } else {
                    Cue::Resources
                }),
        };
        Self {
            route: Route::Onboarding(app.onboarding_page),
            cue,
            blocked,
            // Without a footer snapshot, background surfaces can report real
            // activity but must not guess whether setup is blocked or complete.
            automatic: layout.is_some() || activity.is_some(),
        }
    }

    pub fn reply(&self, attention: Attention) -> &'static str {
        match attention {
            Attention::Feature(0) => "I can hear your mic or computer audio.",
            Attention::Feature(1) => "Local or cloud translation? You choose.",
            Attention::Feature(_) => "Subtitles can join you in VRChat!",
            Attention::Avatar if self.route == Route::Onboarding(0) => "Hehe, I'm right here!",
            Attention::Avatar | Attention::Control(_) => self.cue.text(),
            Attention::Next
                if matches!(
                    self.cue,
                    Cue::Failed
                        | Cue::Checking
                        | Cue::ModelDownload
                        | Cue::RuntimeDownload
                        | Cue::Extracting
                        | Cue::Working
                ) =>
            {
                self.cue.text()
            }
            Attention::Next => self.blocked.map_or_else(
                || match self.route {
                    Route::Onboarding(0) => "Ready? Let's pick your models.",
                    Route::Onboarding(1) => Cue::Voice.text(),
                    Route::Onboarding(2) => "Let's see which resources you need.",
                    _ => self.cue.text(),
                },
                |requirement| Cue::for_requirement(requirement).text(),
            ),
        }
    }
}
