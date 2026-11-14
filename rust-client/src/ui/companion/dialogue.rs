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

// Keep the copy catalogue and coverage checks on the same declaration.
macro_rules! cues {
    ($($name:ident => $text:literal),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq)]
        pub(super) enum Cue { $($name),+ }

        impl Cue {
            #[cfg(test)]
            pub const ALL: &[Self] = &[$(Self::$name),+];
            pub fn text(self) -> &'static str {
                match self { $(Self::$name => $text),+ }
            }
        }
    };
}

cues! {
    Quiet => "",
    Hello => "Hi! I'm your companion. Let's get started!",
    Models => "Choose speech recognition and translation models.",
    Optional => "Choose optional voice, assistant and image recognition features.",
    OptionalMobile => "Choose optional voice and assistant features.",
    ApiKey => "Enter the API keys for your selected services.",
    SignIn => "Sign in to your selected account services.",
    Resources => "Download the resources for your selected models.",
    Runtime => "Install the runtime for your selected local models.",
    Checking => "Checking installed resources.",
    ModelDownload => "Downloading selected models.",
    RuntimeDownload => "Downloading the runtime.",
    Extracting => "Extracting downloaded resources.",
    Working => "Preparing selected resources.",
    Failed => "Open the error details, then retry the failed task.",
    Agreement => "Read and accept the Usage Guidelines to continue.",
    Ready => "Setup is complete. Open Translation to begin.",
    Translation => "Choose languages, enable an audio source, then start translation.",
    Audio => "Use Quick controls to choose input devices and output volume.",
    Styles => "Choose a style card to apply its tone to subsequent translations.",
    VoiceCards => "Preview a voice, then select Use voice to apply it to TTS.",
    Terms => "Choose a domain, then add terms and their preferred translations.",
    Settings => "Adjust language, appearance, model limits and enabled plugins.",
    Osc => "Enable OSC and choose audio sources to send to game chat.",
    Meeting => "Create a meeting, select sources and recording, then start.",
    Player => "Import media, choose languages, then generate subtitles.",
    Vr => "Connect SteamVR and enable Subtitles to show translations in VR.",
    GamePlugins => "Install your game's package, then use its Avatar XRT control.",
    BigWalk => "Connect the bridge via Setup guide and choose chat audio sources.",
    Vocabulary => "Bookmark words from translations and organize them in wordbooks.",
    Motion => "Describe an action and generate its motion preview.",
    AudioInput => "I can hear your mic or computer audio.",
    TranslationChoice => "Choose local or cloud translation.",
    Integrations => "Subtitles can join you in VRChat!",
    NextModels => "Next, choose your speech and translation models.",
    NextResources => "Next, review and download the required resources.",
    Here => "Hehe, I'm right here!",
}

impl Cue {
    pub fn bit(self) -> u64 {
        1 << self as u32
    }

    pub fn repeat_after_change(self) -> bool {
        matches!(
            self,
            Self::ApiKey
                | Self::SignIn
                | Self::Resources
                | Self::Runtime
                | Self::Failed
                | Self::Agreement
                | Self::Ready
        )
    }

    fn for_requirement(requirement: &'static str) -> Self {
        match requirement {
            "Sign in to every selected account service to continue." => Self::SignIn,
            "Configure every required API key to continue." => Self::ApiKey,
            "Download every required model package to continue." => Self::Resources,
            "Install the runtime to continue." => Self::Runtime,
            "Wait for the current model task to finish."
            | "Wait for runtime preparation to finish." => Self::Working,
            "Please agree to the Usage Guidelines to continue." => Self::Agreement,
            _ => Self::Quiet,
        }
    }

    fn optional() -> Self {
        if cfg!(target_os = "android") {
            Self::OptionalMobile
        } else {
            Self::Optional
        }
    }

    fn for_page(page: Page) -> Self {
        match page {
            Page::Translation => Self::Translation,
            Page::AudioStudio => Self::Audio,
            Page::PromptStudio => Self::Styles,
            Page::TtsCenter => Self::VoiceCards,
            Page::CorpusStudio => Self::Terms,
            Page::Settings => Self::Settings,
            Page::Plugin(PluginId::OSC | PluginId::CHILLOUTVR_OSC) => Self::Osc,
            Page::Plugin(PluginId::MEETING) => Self::Meeting,
            Page::Plugin(PluginId::MEDIA) => Self::Player,
            Page::Plugin(PluginId::VR_OVERLAY) => Self::Vr,
            Page::Plugin(PluginId::AVATAR) => Self::GamePlugins,
            Page::Plugin(PluginId::VOCABULARY) => Self::Vocabulary,
            Page::Plugin(id) => match id.as_str() {
                "bigwalk" => Self::BigWalk,
                "motion" => Self::Motion,
                _ => Self::Quiet,
            },
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
    pub hint: Option<super::inbox::Message>,
    pub route: Route,
    pub cue: Cue,
    pub blocked: Option<&'static str>,
    pub automatic: bool,
}

impl Context {
    pub fn read(app: &crate::XRTranslateApp, layout: Option<&OnboardingLayout>) -> Self {
        if !app.first_run {
            return Self {
                hint: if let Page::Plugin(id) = app.navigation.page {
                    app.plugin_host.state(id.as_str())["hint"]
                        .as_str()
                        .map(|text| super::inbox::Message::Text {
                            source: id.as_str().into(),
                            text: text.into(),
                        })
                } else {
                    None
                },
                route: Route::Page(app.navigation.page),
                cue: Cue::for_page(app.navigation.page),
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
            2 => blocked.map_or(Cue::optional(), Cue::for_requirement),
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
            hint: None,
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
            Attention::Feature(0) => Cue::AudioInput.text(),
            Attention::Feature(1) => Cue::TranslationChoice.text(),
            Attention::Feature(_) => Cue::Integrations.text(),
            Attention::Avatar if self.route == Route::Onboarding(0) => Cue::Here.text(),
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
                    Route::Onboarding(0) => Cue::NextModels.text(),
                    Route::Onboarding(1) => Cue::optional().text(),
                    Route::Onboarding(2) => Cue::NextResources.text(),
                    _ => self.cue.text(),
                },
                |requirement| Cue::for_requirement(requirement).text(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_and_page_introductions_match_the_current_flow() {
        assert_eq!(
            Cue::for_requirement("Sign in to every selected account service to continue."),
            Cue::SignIn
        );
        assert_eq!(
            Cue::for_requirement("Download every required model package to continue."),
            Cue::Resources
        );
        assert_eq!(Cue::for_requirement("Unknown prerequisite"), Cue::Quiet);
        assert_eq!(
            Cue::for_page(Page::Plugin(PluginId::AVATAR)),
            Cue::GamePlugins
        );
        assert_eq!(
            Cue::for_page(Page::Plugin(PluginId::parse("unknown-plugin").unwrap())),
            Cue::Quiet
        );
        assert_eq!(Cue::for_page(Page::PromptStudio), Cue::Styles);
        assert!(!Cue::Styles.text().to_lowercase().contains("edit"));
        assert!(Cue::ALL.len() <= u64::BITS as usize);
    }
}
