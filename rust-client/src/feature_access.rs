#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Feature {
    TtsPlayback,
    FloatingSubtitles,
    OscChatbox,
    SpeakerNumbers,
    MuteSync,
}

impl Feature {
    pub const ALL: [Self; 5] = [
        Self::TtsPlayback,
        Self::FloatingSubtitles,
        Self::OscChatbox,
        Self::SpeakerNumbers,
        Self::MuteSync,
    ];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeatureAccess {
    pub available: bool,
    pub unavailable_reason: Option<&'static str>,
}

impl FeatureAccess {
    const fn available() -> Self {
        Self {
            available: true,
            unavailable_reason: None,
        }
    }
}

const FEATURE_ACCESS: &[(Feature, FeatureAccess)] = &[
    (Feature::TtsPlayback, FeatureAccess::available()),
    (
        Feature::FloatingSubtitles,
        FeatureAccess {
            available: !cfg!(target_os = "android"),
            unavailable_reason: Some("Floating subtitles are available on desktop."),
        },
    ),
    (Feature::OscChatbox, FeatureAccess::available()),
    (Feature::SpeakerNumbers, FeatureAccess::available()),
    (Feature::MuteSync, FeatureAccess::available()),
];

pub fn access(feature: Feature) -> FeatureAccess {
    debug_assert_eq!(FEATURE_ACCESS.len(), Feature::ALL.len());
    FEATURE_ACCESS
        .iter()
        .find_map(|(configured, access)| (*configured == feature).then_some(*access))
        .expect("every client feature must have an access-table entry")
}

pub fn is_available(feature: Feature) -> bool {
    access(feature).available
}
