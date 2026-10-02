//! Remote model contracts shared by configuration, runtime routing, and UI.
//!
//! Remote models have no installation, hardware, or download requirements.
//! They intentionally live outside the downloadable model asset catalogue.

use serde::Serialize;

use crate::AsrDelivery;

/// Canonical language codes accepted by the Qwen Audio ASR models below.
/// Provider adapters normalize the legacy `tl` alias to `fil` on the wire.
pub const QWEN_AUDIO_LANGUAGES: &[&str] = &[
    "zh", "en", "ja", "ko", "vi", "th", "id", "ms", "fil", "hi", "ar", "fr", "de", "es", "pt",
    "ru", "it", "nl", "sv", "da", "fi", "no", "el", "pl", "cs", "hu", "ro", "bg", "hr", "sk",
];

/// Recognition features of one supported remote model and wire protocol.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct RemoteAsrModelManifest {
    pub providers: &'static [&'static str],
    pub transport: &'static str,
    pub model: &'static str,
    pub languages: &'static [&'static str],
    /// Accepted application codes mapped to one canonical provider language.
    /// Aliases do not increase the model's displayed language count.
    pub language_aliases: &'static [(&'static str, &'static str)],
    /// Audio delivery supported by the application adapter, independently of
    /// streaming capabilities advertised by the remote service.
    pub delivery: AsrDelivery,
    /// Whether the application adapter emits interim recognition results.
    pub supports_incremental_results: bool,
    /// Vendor-native transcript polishing, not an application request switch.
    /// `None` means this exact model's behavior has not been documented.
    pub native_text_polishing: Option<bool>,
    pub instruction: bool,
    pub context_bias: bool,
    pub context_max_chars: Option<usize>,
    pub vocabulary_bias: bool,
}

const QWEN_AUDIO_FLASH: RemoteAsrModelManifest = RemoteAsrModelManifest {
    providers: &["qwen", "qwen-intl"],
    transport: "dashscope",
    model: "qwen-audio-3.0-asr-flash",
    languages: QWEN_AUDIO_LANGUAGES,
    language_aliases: &[("tl", "fil")],
    delivery: AsrDelivery::Utterance,
    supports_incremental_results: false,
    native_text_polishing: Some(false),
    instruction: false,
    context_bias: true,
    context_max_chars: Some(400),
    vocabulary_bias: true,
};

/// Supported exact model IDs. Unknown revisions do not inherit capabilities.
///
/// The WebSocket adapter currently replays complete recorded utterances and
/// returns a final transcript. A streaming server does not make that adapter
/// a live audio or incremental-result implementation.
pub const REMOTE_ASR_MODEL_CATALOG: &[RemoteAsrModelManifest] = &[
    QWEN_AUDIO_FLASH,
    RemoteAsrModelManifest {
        model: "qwen-audio-3.1-asr-flash",
        native_text_polishing: Some(true),
        ..QWEN_AUDIO_FLASH
    },
    RemoteAsrModelManifest {
        providers: &["qwen-audio-streaming"],
        transport: "websocket",
        model: "qwen-audio-3.0-asr-flash-streaming",
        native_text_polishing: None,
        ..QWEN_AUDIO_FLASH
    },
    RemoteAsrModelManifest {
        providers: &["qwen-audio-streaming"],
        transport: "websocket",
        model: "qwen-audio-3.1-asr-flash-streaming",
        native_text_polishing: None,
        ..QWEN_AUDIO_FLASH
    },
];

#[must_use]
pub fn remote_asr_model(
    provider: &str,
    transport: &str,
    model: &str,
) -> Option<&'static RemoteAsrModelManifest> {
    REMOTE_ASR_MODEL_CATALOG.iter().find(|card| {
        card.providers.contains(&provider) && card.transport == transport && card.model == model
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn qwen_audio_has_thirty_distinct_canonical_languages() {
        assert_eq!(QWEN_AUDIO_LANGUAGES.len(), 30);
        assert_eq!(
            QWEN_AUDIO_LANGUAGES.iter().collect::<HashSet<_>>().len(),
            30
        );
        assert!(QWEN_AUDIO_LANGUAGES.contains(&"fil"));
        assert!(!QWEN_AUDIO_LANGUAGES.contains(&"tl"));
        assert!(!QWEN_AUDIO_LANGUAGES.contains(&"tr"));
        assert!(QWEN_AUDIO_LANGUAGES.contains(&"no"));
    }

    #[test]
    fn remote_lookup_requires_the_exact_model_protocol_and_provider() {
        for provider in ["qwen", "qwen-intl"] {
            for (model, polishing) in [
                ("qwen-audio-3.0-asr-flash", false),
                ("qwen-audio-3.1-asr-flash", true),
            ] {
                let card = remote_asr_model(provider, "dashscope", model).unwrap();
                assert_eq!(card.model, model);
                assert_eq!(card.native_text_polishing, Some(polishing));
                assert!(remote_asr_model(provider, "openai", model).is_none());
                assert!(remote_asr_model(provider, "websocket", model).is_none());
            }
        }
        for model in [
            "qwen-audio-3.0-asr-flash-streaming",
            "qwen-audio-3.1-asr-flash-streaming",
        ] {
            let card = remote_asr_model("qwen-audio-streaming", "websocket", model).unwrap();
            assert_eq!(card.native_text_polishing, None);
            assert!(remote_asr_model("qwen", "dashscope", model).is_none());
        }
        assert!(remote_asr_model("custom", "dashscope", "qwen-audio-3.0-asr-flash").is_none());
        assert!(remote_asr_model("qwen", "dashscope", "qwen-audio-3.0-asr-flash-future").is_none());
        assert!(remote_asr_model("qwen", "openai", "qwen3-asr-flash").is_none());
    }

    #[test]
    fn cards_have_unique_routes_and_declare_the_current_adapter_contract() {
        let mut routes = HashSet::new();
        for card in REMOTE_ASR_MODEL_CATALOG {
            for provider in card.providers {
                assert!(routes.insert((provider, card.transport, card.model)));
            }
            assert_eq!(card.delivery, AsrDelivery::Utterance);
            assert!(!card.supports_incremental_results);
            assert!(!card.instruction);
            assert!(card.context_bias);
            assert_eq!(card.context_max_chars, Some(400));
            assert!(card.vocabulary_bias);
            for (alias, canonical) in card.language_aliases {
                assert!(!card.languages.contains(alias));
                assert!(card.languages.contains(canonical));
            }
        }
    }
}
