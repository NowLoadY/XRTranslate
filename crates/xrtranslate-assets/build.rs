//! Turn the single reviewed JSON model catalog into immutable Rust manifests.
//! Build-time generation keeps runtime lookup allocation-free and lets one card
//! own its capabilities, resource estimate, files, hashes and source revision.

use serde_json::Value;
use std::{collections::HashSet, fs, path::PathBuf};

fn field<'a>(value: &'a Value, name: &str) -> &'a Value {
    value
        .get(name)
        .unwrap_or_else(|| panic!("missing model catalog field {name}"))
}
fn string(value: &Value) -> String {
    format!(
        "{:?}",
        value.as_str().expect("expected model catalog string")
    )
}
fn number(value: &Value) -> String {
    value
        .as_u64()
        .expect("expected model catalog nonnegative integer")
        .to_string()
}
fn boolean(value: &Value) -> String {
    value
        .as_bool()
        .expect("expected model catalog boolean")
        .to_string()
}
fn enum_value(ty: &str, value: &Value) -> String {
    let variant = value.as_str().expect("expected model catalog enum");
    assert!(
        variant
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_')
    );
    format!("{ty}::{variant}")
}
fn strings(value: &Value) -> String {
    let items = value
        .as_array()
        .expect("expected model catalog string array");
    format!(
        "&[{}]",
        items.iter().map(string).collect::<Vec<_>>().join(",")
    )
}
fn required_file(value: &Value) -> String {
    format!(
        "RequiredModelFile {{ role: {}, relative_path: {}, purpose: {}, bytes: {}, sha256: {} }}",
        enum_value("ModelFileRole", field(value, "role")),
        string(field(value, "relative_path")),
        string(field(value, "purpose")),
        number(field(value, "bytes")),
        string(field(value, "sha256"))
    )
}
fn file_override(value: &Value) -> String {
    if let Some(source) = value.get("HuggingFace") {
        format!(
            "ModelFileSource::HuggingFace {{ relative_path: {}, repository: {}, revision: {}, remote_path: {} }}",
            string(field(source, "relative_path")),
            string(field(source, "repository")),
            string(field(source, "revision")),
            string(field(source, "remote_path"))
        )
    } else {
        let source = field(value, "DirectUrl");
        format!(
            "ModelFileSource::DirectUrl {{ relative_path: {}, url: {} }}",
            string(field(source, "relative_path")),
            string(field(source, "url"))
        )
    }
}
fn archive(value: &Value) -> String {
    if value.is_null() {
        return "None".into();
    }
    let entries = field(value, "entries")
        .as_array()
        .expect("archive entries")
        .iter()
        .map(|entry| {
            format!(
                "ModelArchiveEntry {{ relative_path: {}, archive_path: {} }}",
                string(field(entry, "relative_path")),
                string(field(entry, "archive_path"))
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "Some(ModelArchiveSource {{ filename: {}, url: {}, bytes: {}, sha256: {}, entries: &[{}] }})",
        string(field(value, "filename")),
        string(field(value, "url")),
        number(field(value, "bytes")),
        string(field(value, "sha256")),
        entries
    )
}
fn source(value: &Value) -> String {
    let overrides = field(value, "file_overrides")
        .as_array()
        .expect("file overrides")
        .iter()
        .map(file_override)
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "ModelSource {{ repository: {}, revision: {}, remote_directory: {}, include_patterns: {}, file_overrides: &[{}], archive: {} }}",
        string(field(value, "repository")),
        string(field(value, "revision")),
        string(field(value, "remote_directory")),
        strings(field(value, "include_patterns")),
        overrides,
        archive(field(value, "archive"))
    )
}
fn benchmark(value: &Value) -> String {
    if value.is_null() {
        return "None".into();
    }
    format!(
        "Some(ModelBenchmark {{ benchmark: {}, metric: {}, score_centipercent: {}, source_url: {}, note: {} }})",
        string(field(value, "benchmark")),
        string(field(value, "metric")),
        number(field(value, "score_centipercent")),
        string(field(value, "source_url")),
        string(field(value, "note"))
    )
}
fn audio_output(value: &Value) -> String {
    if value.is_null() {
        return "None".into();
    }
    format!(
        "Some(ModelAudioOutput {{ sample_rate_hz: {}, channels: {}, sample_format: {} }})",
        number(field(value, "sample_rate_hz")),
        number(field(value, "channels")),
        enum_value("ModelAudioSampleFormat", field(value, "sample_format"))
    )
}
fn runtime(card: &Value) -> String {
    let Some(value) = card.get("runtime").filter(|value| !value.is_null()) else {
        assert_eq!(field(card, "capability").as_str(), Some("tts"));
        return "None".into();
    };
    let kind = field(value, "kind").as_str().expect("runtime kind");
    let capability = field(card, "capability").as_str().expect("capability");
    let files = field(card, "required_files").as_array().expect("files");
    let has_role = |role: &str| {
        files
            .iter()
            .any(|file| field(file, "role").as_str() == Some(role))
    };
    let delivery = || match field(value, "delivery").as_str().expect("ASR delivery") {
        "utterance" => "AsrDelivery::Utterance",
        "live" => "AsrDelivery::Live",
        other => panic!("unknown ASR delivery {other}"),
    };
    let alias = || {
        let alias = field(value, "model_alias");
        assert!(!alias.as_str().expect("model alias").trim().is_empty());
        string(alias)
    };
    let variant = match kind {
        "llama-audio-chat" => {
            assert_eq!(capability, "asr");
            assert!(has_role("Weights") && has_role("MultimodalProjection"));
            let style = match field(value, "prompt_style")
                .as_str()
                .expect("ASR prompt style")
            {
                "qwen-asr" => "AsrPromptStyle::QwenAsr",
                other => panic!("unknown ASR prompt style {other}"),
            };
            format!(
                "ModelRuntime::LlamaAudioChat {{ model_alias: {}, delivery: {}, prompt_style: {style}, context_bias: {}, vocabulary_bias: {} }}",
                alias(),
                delivery(),
                boolean(field(value, "context_bias")),
                boolean(field(value, "vocabulary_bias"))
            )
        }
        "sherpa-offline-asr" => {
            assert_eq!(capability, "asr");
            assert!(has_role("Weights") && has_role("Tokenizer"));
            assert_eq!(delivery(), "AsrDelivery::Utterance");
            "ModelRuntime::SherpaOfflineAsr { delivery: AsrDelivery::Utterance }".into()
        }
        "llama-text-chat" => {
            assert_eq!(capability, "translation");
            assert!(has_role("Weights") && !has_role("MultimodalProjection"));
            let style = match field(value, "prompt_style").as_str().expect("prompt style") {
                "contextual" => "TranslationPromptStyle::Contextual",
                "bilingual" => "TranslationPromptStyle::Bilingual",
                other => panic!("unknown translation prompt style {other}"),
            };
            format!(
                "ModelRuntime::LlamaTextChat {{ model_alias: {}, prompt_style: {style}, flash_attention: {}, allow_reference_context: {} }}",
                alias(),
                boolean(field(value, "flash_attention")),
                boolean(field(value, "allow_reference_context"))
            )
        }
        other => panic!("unknown model runtime kind {other}"),
    };
    format!("Some({variant})")
}
fn manifest(value: &Value, variant: &str) -> String {
    let level = match field(value, "level").as_str().expect("level") {
        "small" => "ModelLevel::Small",
        "normal" => "ModelLevel::Normal",
        "big" => "ModelLevel::Big",
        "ultra" => "ModelLevel::Ultra",
        level => panic!("unknown model level {level}"),
    };
    let capability = match field(value, "capability").as_str().expect("capability") {
        "asr" => "ModelCapability::Asr",
        "translation" => "ModelCapability::Translation",
        "tts" => "ModelCapability::Tts",
        capability => panic!("unknown model capability {capability}"),
    };
    let hardware = field(value, "hardware");
    let files = field(value, "required_files")
        .as_array()
        .expect("files")
        .iter()
        .map(required_file)
        .collect::<Vec<_>>()
        .join(",");
    let presets = field(value, "voice_presets")
        .as_array()
        .expect("presets")
        .iter()
        .map(|preset| {
            format!(
                "ModelVoicePreset {{ key: {}, label: {}, language: {}, is_default: {} }}",
                string(field(preset, "key")),
                string(field(preset, "label")),
                string(field(preset, "language")),
                boolean(field(preset, "is_default"))
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let parameters = field(value, "parameters_millions");
    let parameters = if parameters.is_null() {
        "None".into()
    } else {
        format!("Some({})", number(parameters))
    };
    format!(
        "ModelAssetManifest {{ id: ModelAssetId::{variant}, label: {}, capability: {capability}, level: {level}, tier_default: {}, estimated_vram_bytes: {}, parameters_millions: {parameters}, benchmark: {}, provider: {}, languages: {}, voice_presets: &[{presets}], hardware: ModelHardwareRequirements {{ accelerator: {}, minimum_memory_bytes: {} }}, audio_output: {}, runtime: {}, relative_directory: {}, required_files: &[{files}], source: {} }}",
        string(field(value, "label")),
        boolean(field(value, "tier_default")),
        number(field(value, "estimated_vram_bytes")),
        benchmark(field(value, "benchmark")),
        string(field(value, "provider")),
        strings(field(value, "languages")),
        enum_value("ModelAccelerator", field(hardware, "accelerator")),
        number(field(hardware, "minimum_memory_bytes")),
        audio_output(field(value, "audio_output")),
        runtime(value),
        string(field(value, "relative_directory")),
        source(field(value, "source"))
    )
}

fn main() {
    println!("cargo:rerun-if-changed=model_catalog.json");
    let cards: Vec<Value> =
        serde_json::from_slice(&fs::read("model_catalog.json").expect("model catalog JSON"))
            .expect("valid model catalog JSON");
    let mut seen = HashSet::new();
    for card in &cards {
        for key in ["id", "rust_variant", "const_name"] {
            let value = field(card, key).as_str().expect("catalog symbol");
            assert!(
                seen.insert((key, value)),
                "duplicate model catalog {key}: {value}"
            );
            if key != "id" {
                assert!(
                    value
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                );
            }
        }
    }

    let mut ids = String::from(
        "// Generated from model_catalog.json; edit that file.\n\
         #[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, serde::Serialize, serde::Deserialize)]\n\
         pub enum ModelAssetId {\n",
    );
    for card in &cards {
        ids.push_str(&format!(
            "#[serde(rename = {})] {},\n",
            string(field(card, "id")),
            field(card, "rust_variant").as_str().expect("Rust variant")
        ));
    }
    ids.push_str("}\nimpl ModelAssetId {\n#[must_use] pub const fn as_str(self) -> &'static str { match self {\n");
    for card in &cards {
        ids.push_str(&format!(
            "Self::{} => {},\n",
            field(card, "rust_variant").as_str().expect("Rust variant"),
            string(field(card, "id"))
        ));
    }
    ids.push_str(
        "}}\n#[must_use] pub fn from_config_key(value: &str) -> Option<Self> {\n\
         MODEL_ASSET_CATALOG.iter().find(|manifest| manifest.id.as_str() == value).map(|manifest| manifest.id)\n\
         }}\nimpl std::fmt::Display for ModelAssetId {\n\
         fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {\n\
         formatter.write_str(self.as_str()) } }\n",
    );

    let mut output = String::from("// Generated from model_catalog.json; edit that file.\n");
    for card in &cards {
        let variant = field(card, "rust_variant").as_str().expect("Rust variant");
        let name = field(card, "const_name").as_str().expect("constant name");
        output.push_str(&format!(
            "pub const {name}: ModelAssetManifest = {};\n",
            manifest(card, variant)
        ));
    }
    output.push_str("pub const MODEL_ASSET_CATALOG: &[ModelAssetManifest] = &[\n");
    for card in &cards {
        let name = field(card, "const_name").as_str().expect("constant name");
        output.push_str(&format!("{name},\n"));
    }
    output.push_str(
        "];\npub const DEFAULT_GGUF_MANIFEST: &[ModelAssetManifest] = MODEL_ASSET_CATALOG;\n",
    );
    let directory = PathBuf::from(std::env::var("OUT_DIR").expect("Cargo OUT_DIR"));
    fs::write(directory.join("model_ids.rs"), ids).expect("write generated model IDs");
    fs::write(directory.join("model_catalog.rs"), output).expect("write generated model catalog");
}
