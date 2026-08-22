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
    (Feature::FloatingSubtitles, FeatureAccess::available()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn access_table_is_complete_and_unique() {
        let configured = FEATURE_ACCESS
            .iter()
            .map(|(feature, _)| *feature)
            .collect::<HashSet<_>>();
        assert_eq!(configured.len(), FEATURE_ACCESS.len());
        assert_eq!(configured.len(), Feature::ALL.len());
        assert!(
            Feature::ALL
                .iter()
                .all(|feature| configured.contains(feature))
        );
    }

    #[test]
    fn unavailable_features_have_a_reason() {
        assert!(FEATURE_ACCESS.iter().all(|(_, access)| {
            access.available
                || access
                    .unavailable_reason
                    .is_some_and(|reason| !reason.is_empty())
        }));
    }

    #[test]
    fn native_tts_is_available_with_a_configured_provider() {
        assert!(is_available(Feature::TtsPlayback));
    }
}
