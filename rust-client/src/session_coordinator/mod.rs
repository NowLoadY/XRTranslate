//! Host-side contract between translation infrastructure and feature plugins.
//!
//! This module deliberately contains no concrete plugin imports. Plugins
//! describe how they want to use a translation session and may subscribe to
//! the resulting generic session events; the network/audio implementation
//! remains unaware of every plugin.

mod owner;
mod plugin_session;
mod request;
mod subscriber;

pub use owner::{PluginSessionOwner, TranslationSessionOwner};
pub use plugin_session::{PluginSessionBinding, SessionOutputPolicy, TranslationSessionPlugin};
pub(crate) use request::{TranslationInput, TranslationTask};
pub use subscriber::{
    CaptionUpdate, HostOutputEvent, HostOutputSubscriber, SessionEventSubscriber,
};

#[cfg(test)]
mod architecture_tests {
    const SHARED_INFRASTRUCTURE: &[(&str, &str)] = &[
        ("network.rs", include_str!("../network.rs")),
        ("audio.rs", include_str!("../audio.rs")),
        (
            "media_import/mod.rs",
            include_str!("../media_import/mod.rs"),
        ),
        (
            "media_import/api.rs",
            include_str!("../media_import/api.rs"),
        ),
        (
            "media_import/types.rs",
            include_str!("../media_import/types.rs"),
        ),
        (
            "media_import/decode.rs",
            include_str!("../media_import/decode.rs"),
        ),
        (
            "media_import/stream.rs",
            include_str!("../media_import/stream.rs"),
        ),
        (
            "media_import/mpv_extract.rs",
            include_str!("../media_import/mpv_extract.rs"),
        ),
        ("session_coordinator/owner.rs", include_str!("owner.rs")),
        ("session_coordinator/request.rs", include_str!("request.rs")),
        (
            "session_coordinator/plugin_session.rs",
            include_str!("plugin_session.rs"),
        ),
        (
            "session_coordinator/subscriber.rs",
            include_str!("subscriber.rs"),
        ),
    ];

    #[test]
    fn shared_infrastructure_does_not_import_concrete_plugins() {
        for (path, source) in SHARED_INFRASTRUCTURE {
            assert!(
                !source.contains("crate::plugins")
                    && !source.contains("plugins::meeting")
                    && !source.contains("plugins::player")
                    && !source.contains("plugins::osc"),
                "shared infrastructure must not import a concrete plugin: {path}"
            );
        }
    }

    #[test]
    fn plugin_session_contract_cannot_disable_recognition_metadata() {
        let contract = include_str!("plugin_session.rs");
        assert!(!contract.contains("SpeakerRecognition"));
        assert!(!contract.contains("speaker_recognition"));

        let osc_ui = include_str!("../plugins/osc/ui/mod.rs");
        assert!(osc_ui.contains("SetSpeakerNumberVisible"));
        assert!(!osc_ui.contains("SetSpeakerRecognitionEnabled"));
    }
}
