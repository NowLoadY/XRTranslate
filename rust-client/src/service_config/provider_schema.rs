#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum ProviderFieldEditor {
    Default,
    ModelLevel,
    UnsignedRange {
        minimum: u32,
        maximum: u32,
        speed: f64,
    },
    Options(&'static [&'static str]),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProviderFieldVisibility {
    Default,
    NativeModel,
    Hidden,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ProviderFieldDescriptor {
    pub name: &'static str,
    pub label: &'static str,
    pub help: Option<&'static str>,
    pub editor: ProviderFieldEditor,
    visibility: ProviderFieldVisibility,
}

impl ProviderFieldDescriptor {
    pub(super) const fn is_visible(self, native_model: bool) -> bool {
        match self.visibility {
            ProviderFieldVisibility::Default => !native_model,
            ProviderFieldVisibility::NativeModel => native_model,
            ProviderFieldVisibility::Hidden => false,
        }
    }
}

const DEVICE_OPTIONS: &[&str] = &["auto", "cuda"];

const PROVIDER_FIELDS: &[ProviderFieldDescriptor] = &[
    ProviderFieldDescriptor {
        name: "transport",
        label: "Transport",
        help: Some(
            "local uses the managed llama.cpp model; openai uses an OpenAI-compatible HTTP API; websocket uses a provider-native WebSocket API.",
        ),
        editor: ProviderFieldEditor::Options(&["local", "openai", "websocket"]),
        visibility: ProviderFieldVisibility::Default,
    },
    ProviderFieldDescriptor {
        name: "sample_rate",
        label: "Output sample rate",
        help: Some("PCM sample rate returned by the TTS provider."),
        editor: ProviderFieldEditor::UnsignedRange {
            minimum: 8_000,
            maximum: 96_000,
            speed: 100.0,
        },
        visibility: ProviderFieldVisibility::Default,
    },
    ProviderFieldDescriptor {
        name: "max_input_chars",
        label: "Characters per request",
        help: Some("Long translations are split at sentence boundaries before synthesis."),
        editor: ProviderFieldEditor::UnsignedRange {
            minimum: 32,
            maximum: 500,
            speed: 1.0,
        },
        visibility: ProviderFieldVisibility::Default,
    },
    ProviderFieldDescriptor {
        name: "api_key",
        label: "API key",
        help: Some(
            "Bearer credential for the selected remote API. Leave empty only when the endpoint does not require authentication.",
        ),
        editor: ProviderFieldEditor::Default,
        visibility: ProviderFieldVisibility::Default,
    },
    ProviderFieldDescriptor {
        name: "context_window_tokens",
        label: "Context tokens per request",
        help: Some("Input and output context available to each parallel model request."),
        editor: ProviderFieldEditor::UnsignedRange {
            minimum: 256,
            maximum: 32_768,
            speed: 128.0,
        },
        visibility: ProviderFieldVisibility::NativeModel,
    },
    ProviderFieldDescriptor {
        name: "max_tokens",
        label: "Max output tokens",
        help: Some("Maximum tokens generated for one result."),
        editor: ProviderFieldEditor::UnsignedRange {
            minimum: 16,
            maximum: 4_096,
            speed: 1.0,
        },
        visibility: ProviderFieldVisibility::NativeModel,
    },
    ProviderFieldDescriptor {
        name: "parallel_slots",
        label: "Parallel requests",
        help: Some(
            "Concurrent llama.cpp request slots. Total context cache is context tokens multiplied by this value.",
        ),
        editor: ProviderFieldEditor::UnsignedRange {
            minimum: 1,
            maximum: 16,
            speed: 1.0,
        },
        visibility: ProviderFieldVisibility::Hidden,
    },
    ProviderFieldDescriptor {
        name: "model_asset",
        label: "Level",
        help: None,
        editor: ProviderFieldEditor::ModelLevel,
        visibility: ProviderFieldVisibility::NativeModel,
    },
    ProviderFieldDescriptor {
        name: "supports_prompt_context",
        label: "Prompt context",
        help: None,
        editor: ProviderFieldEditor::Default,
        visibility: ProviderFieldVisibility::Hidden,
    },
    ProviderFieldDescriptor {
        name: "asr_prompt_mode",
        label: "ASR text mode",
        help: Some(
            "instruction is a semantic recognition prompt; context_bias is lexical context and must not be treated as an instruction.",
        ),
        editor: ProviderFieldEditor::Options(&["none", "instruction", "context_bias"]),
        visibility: ProviderFieldVisibility::Hidden,
    },
    ProviderFieldDescriptor {
        name: "asr_context_max_chars",
        label: "ASR context character limit",
        help: None,
        editor: ProviderFieldEditor::Default,
        visibility: ProviderFieldVisibility::Hidden,
    },
    ProviderFieldDescriptor {
        name: "supports_vocabulary_bias",
        label: "Weighted vocabulary",
        help: None,
        editor: ProviderFieldEditor::Default,
        visibility: ProviderFieldVisibility::Hidden,
    },
    ProviderFieldDescriptor {
        name: "vocabulary_weight",
        label: "Hotword weight",
        help: Some(
            "Default weight for dynamic XR Corpus vocabulary. Use 1-5, or 50 for a super hotword.",
        ),
        editor: ProviderFieldEditor::Options(&["1", "2", "3", "4", "5", "50"]),
        visibility: ProviderFieldVisibility::Default,
    },
    ProviderFieldDescriptor {
        name: "supports_language",
        label: "Language selection",
        help: None,
        editor: ProviderFieldEditor::Default,
        visibility: ProviderFieldVisibility::Hidden,
    },
    ProviderFieldDescriptor {
        name: "supports_prompt",
        label: "Custom prompt",
        help: None,
        editor: ProviderFieldEditor::Default,
        visibility: ProviderFieldVisibility::Hidden,
    },
    ProviderFieldDescriptor {
        name: "prompt_field",
        label: "Prompt field",
        help: None,
        editor: ProviderFieldEditor::Default,
        visibility: ProviderFieldVisibility::Hidden,
    },
    ProviderFieldDescriptor {
        name: "url",
        label: "Endpoint URL",
        help: None,
        editor: ProviderFieldEditor::Default,
        visibility: ProviderFieldVisibility::Default,
    },
    ProviderFieldDescriptor {
        name: "model",
        label: "Model",
        help: None,
        editor: ProviderFieldEditor::Default,
        visibility: ProviderFieldVisibility::Default,
    },
    ProviderFieldDescriptor {
        name: "device",
        label: "Device",
        help: Some(
            "Auto selects the newest compatible managed CUDA and cuDNN runtime. Managed local models never fall back to CPU.",
        ),
        editor: ProviderFieldEditor::Options(DEVICE_OPTIONS),
        visibility: ProviderFieldVisibility::Default,
    },
];

pub(super) fn provider_field_descriptor(name: &str) -> Option<ProviderFieldDescriptor> {
    PROVIDER_FIELDS
        .iter()
        .copied()
        .find(|descriptor| descriptor.name == name)
}

#[cfg(test)]
mod tests {
    use super::{ProviderFieldEditor, provider_field_descriptor};

    #[test]
    fn native_visibility_matches_the_existing_provider_form() {
        assert!(
            provider_field_descriptor("model_asset")
                .unwrap()
                .is_visible(true)
        );
        assert!(
            provider_field_descriptor("context_window_tokens")
                .unwrap()
                .is_visible(true)
        );
        assert!(
            provider_field_descriptor("max_tokens")
                .unwrap()
                .is_visible(true)
        );
        assert!(!provider_field_descriptor("url").unwrap().is_visible(true));
        assert!(provider_field_descriptor("url").unwrap().is_visible(false));
        assert!(
            !provider_field_descriptor("parallel_slots")
                .unwrap()
                .is_visible(false)
        );
    }

    #[test]
    fn numeric_editor_keeps_context_range_and_speed() {
        assert_eq!(
            provider_field_descriptor("context_window_tokens")
                .unwrap()
                .editor,
            ProviderFieldEditor::UnsignedRange {
                minimum: 256,
                maximum: 32_768,
                speed: 128.0,
            }
        );
    }

    #[test]
    fn device_editor_exposes_only_supported_tts_backends() {
        assert_eq!(
            provider_field_descriptor("device").unwrap().editor,
            ProviderFieldEditor::Options(&["auto", "cuda"])
        );
        assert_eq!(
            provider_field_descriptor("device").unwrap().help,
            Some(
                "Auto selects the newest compatible managed CUDA and cuDNN runtime. Managed local models never fall back to CPU."
            )
        );
    }
}
