//! Shared model-catalog schema.

use std::fmt;

use serde::{Deserialize, Serialize};

use super::MODEL_ASSET_CATALOG;

/// Stable identifier for a model package required by the initial native route.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModelAssetId {
    /// Qwen3-ASR GGUF plus its multimodal projection.
    Qwen3AsrGguf,
    /// Hunyuan MT2 GGUF used by the local translation server.
    HunyuanMtGguf,
    HunyuanMt7bGguf,
    /// Audio8 multilingual TTS ONNX FP16 package, including voice registration.
    Audio8TtsOnnxFp16,
    /// NVIDIA OpenVoice v3 ONNX package with MeloTTS English v3.
    OpenVoiceV3OnnxFp16,
    /// NVIDIA OpenVoice v2 ONNX package with five English accents.
    OpenVoiceV2OnnxFp16,
    /// XRTranslate reproducible OpenVoice v2 Chinese ONNX language pack.
    OpenVoiceV2ZhOnnxFp16,
}

impl ModelAssetId {
    /// Stable, machine-readable identifier used in diagnostics and packaging.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Qwen3AsrGguf => "qwen3-asr-gguf",
            Self::HunyuanMtGguf => "hy-mt2",
            Self::HunyuanMt7bGguf => "hy-mt2-big",
            Self::Audio8TtsOnnxFp16 => "audio8-tts-onnx-fp16",
            Self::OpenVoiceV3OnnxFp16 => "openvoice-v3-onnx-fp16",
            Self::OpenVoiceV2OnnxFp16 => "openvoice-v2-onnx-fp16",
            Self::OpenVoiceV2ZhOnnxFp16 => "openvoice-v2-zh-onnx-fp16",
        }
    }

    /// Resolves a stable `model_asset` key stored in a provider object.
    #[must_use]
    pub fn from_config_key(value: &str) -> Option<Self> {
        MODEL_ASSET_CATALOG
            .iter()
            .find(|manifest| manifest.id.as_str() == value)
            .map(|manifest| manifest.id)
    }
}

impl fmt::Display for ModelAssetId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Native backend capability provided by a model asset.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModelCapability {
    Asr,
    Translation,
    Tts,
}

impl ModelCapability {
    /// Whether a provider may activate one package or several complementary
    /// packages for this capability. TTS language packs compose; recognition
    /// and translation model variants replace one another.
    #[must_use]
    pub const fn allows_multiple_assets(self) -> bool {
        matches!(self, Self::Tts)
    }
}

/// Hardware contract for a downloadable native model package. Small ONNX
/// components bundled with the application (VAD, denoise and speaker helpers)
/// are intentionally outside this catalogue and therefore outside this gate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelHardwareRequirements {
    pub accelerator: ModelAccelerator,
    pub minimum_memory_bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelAccelerator {
    NvidiaCuda,
}

/// Minimum reported VRAM for managed local model packages.
///
/// An 8 GB product may report slightly less than 8 GiB to the driver, so the
/// eligibility threshold intentionally uses 7 GiB.
pub const MANAGED_LOCAL_MODEL_MINIMUM_VRAM_BYTES: u64 = 7 * 1024 * 1024 * 1024;
pub const MANAGED_LOCAL_MODEL_HARDWARE: ModelHardwareRequirements = ModelHardwareRequirements {
    accelerator: ModelAccelerator::NvidiaCuda,
    minimum_memory_bytes: MANAGED_LOCAL_MODEL_MINIMUM_VRAM_BYTES,
};

/// Wire-level audio produced by a native model package. The desktop uses this
/// immutable capability instead of trusting an editable provider setting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelAudioOutput {
    pub sample_rate_hz: u32,
    pub channels: u16,
    pub sample_format: ModelAudioSampleFormat,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelAudioSampleFormat {
    PcmI16Le,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelLevel {
    Normal,
    Big,
    Ultra,
}

/// Runtime role of a file inside a model package. Server factories query this
/// role instead of relying on manifest array position.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ModelFileRole {
    Weights,
    MultimodalProjection,
    RuntimeManifest,
    Tokenizer,
    CodecDecoder,
    CodecEncoder,
    FastArGraph,
    CodecDecoderData,
    CodecEncoderData,
    RegistrationManifest,
    SlowArGraph,
    ModelConfig,
    BertGraph,
    BaseTtsGraph,
    ToneConverterGraph,
    SpeakerEncoderGraph,
    PhonemeMap,
    LanguageLexicon,
    PronunciationDictionary,
    Vocabulary,
    SpeakerEmbedding,
    License,
}

impl ModelLevel {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Big => "big",
            Self::Ultra => "ultra",
        }
    }
}

/// A file that must exist within a [`ModelAssetManifest::relative_directory`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RequiredModelFile {
    pub role: ModelFileRole,
    /// File path relative to the asset directory. This is intentionally not a
    /// glob: runtime startup must use a deterministic artifact.
    pub relative_path: &'static str,
    /// Human-readable purpose shown in preflight diagnostics.
    pub purpose: &'static str,
    /// Exact byte length recorded in the versioned source manifest.
    pub bytes: u64,
    /// Lowercase SHA-256 digest of the complete file.
    pub sha256: &'static str,
}

/// Repository metadata retained for installers and release packaging.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelSource {
    /// Source repository expected to contain this asset.
    pub repository: &'static str,
    /// Immutable Hugging Face revision from which every declared file came.
    pub revision: &'static str,
    /// Optional repository directory containing the package files. Local
    /// required-file paths remain normalized and provider-independent.
    pub remote_directory: &'static str,
    /// Exact source-file patterns used by an installer, if it has one.
    pub include_patterns: &'static [&'static str],
    /// Per-file source overrides for packages assembled from compatible,
    /// independently versioned exports.
    pub file_overrides: &'static [ModelFileSource],
    /// Optional immutable archive used for most files in a model package.
    /// Entries are extracted declaratively; per-file overrides still use the
    /// normal verified downloader.
    pub archive: Option<ModelArchiveSource>,
}

impl ModelSource {
    /// Builds a pinned Hugging Face resolve URL for a manifest file.
    #[must_use]
    pub fn hugging_face_resolve_url(&self, relative_path: &str) -> String {
        if let Some(source) = self
            .file_overrides
            .iter()
            .find(|source| source.relative_path == relative_path)
        {
            return format!(
                "https://huggingface.co/{}/resolve/{}/{}",
                source.repository, source.revision, source.remote_path
            );
        }
        let remote_path = if self.remote_directory.is_empty() {
            relative_path.to_owned()
        } else {
            format!(
                "{}/{}",
                self.remote_directory.trim_end_matches('/'),
                relative_path
            )
        };
        format!(
            "https://huggingface.co/{}/resolve/{}/{}",
            self.repository, self.revision, remote_path
        )
    }
}

/// Immutable source of one file that differs from the package's primary
/// repository. Download and verification still use the shared installer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelFileSource {
    pub relative_path: &'static str,
    pub repository: &'static str,
    pub revision: &'static str,
    pub remote_path: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelArchiveSource {
    pub filename: &'static str,
    pub url: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
    pub entries: &'static [ModelArchiveEntry],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelArchiveEntry {
    pub relative_path: &'static str,
    pub archive_path: &'static str,
}

/// A stable, user-selectable base voice contained in one model package.
/// Language routing remains a package capability; this metadata only chooses
/// a speaker/accent within the selected package and never creates a download.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelVoicePreset {
    pub key: &'static str,
    pub label: &'static str,
    pub language: &'static str,
    pub is_default: bool,
}

/// Static description of one locally-installed model package.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelAssetManifest {
    pub id: ModelAssetId,
    pub label: &'static str,
    pub capability: ModelCapability,
    pub level: ModelLevel,
    pub provider: &'static str,
    /// BCP-47-style language tags covered by this package. An empty list means
    /// the model is not partitioned into selectable language packs.
    pub languages: &'static [&'static str],
    pub voice_presets: &'static [ModelVoicePreset],
    pub hardware: ModelHardwareRequirements,
    pub audio_output: Option<ModelAudioOutput>,
    /// Directory relative to the models root.
    pub relative_directory: &'static str,
    pub required_files: &'static [RequiredModelFile],
    pub source: ModelSource,
}

impl ModelAssetManifest {
    /// Bytes transferred by the shared installer. Archive-backed packages use
    /// the compressed archive size plus independently sourced file overrides.
    #[must_use]
    pub fn download_bytes(&self) -> u64 {
        let archive = self.source.archive;
        archive.map_or(0, |archive| archive.bytes)
            + self
                .required_files
                .iter()
                .filter(|file| {
                    !archive.is_some_and(|archive| {
                        archive
                            .entries
                            .iter()
                            .any(|entry| entry.relative_path == file.relative_path)
                    })
                })
                .map(|file| file.bytes)
                .sum::<u64>()
    }

    /// Final bytes occupied by the verified files declared by this package.
    /// This deliberately excludes staging and filesystem allocation overhead.
    #[must_use]
    pub fn installed_bytes(&self) -> u64 {
        self.required_files.iter().map(|file| file.bytes).sum()
    }
}
