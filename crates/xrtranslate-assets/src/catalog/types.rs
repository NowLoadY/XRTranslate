//! Shared model-catalog schema.

use serde::{Deserialize, Serialize};

use super::ModelAssetId;

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
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ModelHardwareRequirements {
    pub accelerator: ModelAccelerator,
    pub minimum_memory_bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum ModelAccelerator {
    NvidiaCuda,
    Cpu,
}

/// Minimum reported VRAM for managed local model packages.
///
/// An 8 GB product may report slightly less than 8 GiB to the driver, so the
/// eligibility threshold intentionally uses 7 GiB.
pub const MANAGED_LOCAL_MODEL_MINIMUM_VRAM_BYTES: u64 = 7 * 1024 * 1024 * 1024;
/// Lower entry threshold for the small ASR/translation pair on nominal 4 GB GPUs.
pub const MANAGED_SMALL_MODEL_MINIMUM_VRAM_BYTES: u64 = 3 * 1024 * 1024 * 1024;
pub const MANAGED_LOCAL_MODEL_HARDWARE: ModelHardwareRequirements = ModelHardwareRequirements {
    accelerator: ModelAccelerator::NvidiaCuda,
    minimum_memory_bytes: MANAGED_LOCAL_MODEL_MINIMUM_VRAM_BYTES,
};
pub const MANAGED_SMALL_MODEL_HARDWARE: ModelHardwareRequirements = ModelHardwareRequirements {
    accelerator: ModelAccelerator::NvidiaCuda,
    minimum_memory_bytes: MANAGED_SMALL_MODEL_MINIMUM_VRAM_BYTES,
};
pub const CPU_MODEL_HARDWARE: ModelHardwareRequirements = ModelHardwareRequirements {
    accelerator: ModelAccelerator::Cpu,
    minimum_memory_bytes: 0,
};

/// Wire-level audio produced by a native model package. The desktop uses this
/// immutable capability instead of trusting an editable provider setting.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ModelAudioOutput {
    pub sample_rate_hz: u32,
    pub channels: u16,
    pub sample_format: ModelAudioSampleFormat,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum ModelAudioSampleFormat {
    PcmI16Le,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelLevel {
    Small,
    Normal,
    Big,
    Ultra,
}

/// How audio reaches an ASR adapter. Local cards currently use complete
/// speech segments; a future live adapter can opt into incremental delivery.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum AsrDelivery {
    Utterance,
    Live,
}

/// Request format used by an audio-chat recognition adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum AsrPromptStyle {
    QwenAsr,
}

/// Prompt and decoding contract for a text chat translation model.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum TranslationPromptStyle {
    Contextual,
    Bilingual,
}

/// Executable interface selected by the model card, independent of its name.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum ModelRuntime {
    LlamaAudioChat {
        model_alias: &'static str,
        delivery: AsrDelivery,
        prompt_style: AsrPromptStyle,
        context_bias: bool,
        vocabulary_bias: bool,
        extra_args: &'static [&'static str],
    },
    SherpaOfflineAsr {
        delivery: AsrDelivery,
    },
    LlamaTextChat {
        model_alias: &'static str,
        prompt_style: TranslationPromptStyle,
        flash_attention: bool,
        allow_reference_context: bool,
        extra_args: &'static [&'static str],
    },
}

impl ModelRuntime {
    #[must_use]
    pub const fn extra_args(self) -> &'static [&'static str] {
        match self {
            Self::LlamaAudioChat { extra_args, .. } | Self::LlamaTextChat { extra_args, .. } => {
                extra_args
            }
            Self::SherpaOfflineAsr { .. } => &[],
        }
    }
    #[must_use]
    pub const fn uses_llama_cpp(self) -> bool {
        matches!(
            self,
            Self::LlamaAudioChat { .. } | Self::LlamaTextChat { .. }
        )
    }

    #[must_use]
    pub const fn transport(self) -> &'static str {
        match self {
            Self::LlamaAudioChat { .. } | Self::LlamaTextChat { .. } => "local",
            Self::SherpaOfflineAsr { .. } => "onnx-cpu",
        }
    }

    #[must_use]
    pub const fn model_alias(self) -> Option<&'static str> {
        match self {
            Self::LlamaAudioChat { model_alias, .. } | Self::LlamaTextChat { model_alias, .. } => {
                Some(model_alias)
            }
            Self::SherpaOfflineAsr { .. } => None,
        }
    }
}

/// Runtime role of a file inside a model package. Server factories query this
/// role instead of relying on manifest array position.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize)]
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
            Self::Small => "small",
            Self::Normal => "normal",
            Self::Big => "big",
            Self::Ultra => "ultra",
        }
    }
}

/// A file that must exist within a [`ModelAssetManifest::relative_directory`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
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
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
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
    /// Per-file source overrides for compatible exports and pinned notices.
    pub file_overrides: &'static [ModelFileSource],
    /// Optional immutable archive used for most files in a model package.
    /// Entries are extracted declaratively; per-file overrides still use the
    /// normal verified downloader.
    pub archive: Option<ModelArchiveSource>,
}

impl ModelSource {
    /// Builds the pinned download URL for a manifest file.
    #[must_use]
    pub fn file_url(&self, relative_path: &str) -> String {
        if let Some(source) = self
            .file_overrides
            .iter()
            .find(|source| source.relative_path() == relative_path)
        {
            return source.url();
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
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum ModelFileSource {
    HuggingFace {
        relative_path: &'static str,
        repository: &'static str,
        revision: &'static str,
        remote_path: &'static str,
    },
    DirectUrl {
        relative_path: &'static str,
        /// Use an immutable revision in the URL; bytes and SHA-256 are checked.
        url: &'static str,
    },
}

impl ModelFileSource {
    fn relative_path(self) -> &'static str {
        match self {
            Self::HuggingFace { relative_path, .. } | Self::DirectUrl { relative_path, .. } => {
                relative_path
            }
        }
    }

    fn url(self) -> String {
        match self {
            Self::HuggingFace {
                repository,
                revision,
                remote_path,
                ..
            } => format!("https://huggingface.co/{repository}/resolve/{revision}/{remote_path}"),
            Self::DirectUrl { url, .. } => url.to_owned(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ModelArchiveSource {
    pub filename: &'static str,
    pub url: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
    pub entries: &'static [ModelArchiveEntry],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ModelArchiveEntry {
    pub relative_path: &'static str,
    pub archive_path: &'static str,
}

/// A stable, user-selectable base voice contained in one model package.
/// Language routing remains a package capability; this metadata only chooses
/// a speaker/accent within the selected package and never creates a download.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ModelVoicePreset {
    pub key: &'static str,
    pub label: &'static str,
    pub language: &'static str,
    pub is_default: bool,
}

/// One published result for comparing base model families. Scores of a
/// quantized export must only be attributed to that export when measured.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ModelBenchmark {
    pub benchmark: &'static str,
    pub metric: &'static str,
    /// Score in hundredths of a percent, preserving published precision.
    pub score_centipercent: u16,
    pub source_url: &'static str,
    pub note: &'static str,
}

/// Static description of one locally-installed model package.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ModelAssetManifest {
    pub id: ModelAssetId,
    pub label: &'static str,
    pub capability: ModelCapability,
    pub level: ModelLevel,
    /// Explicit default within this provider, capability and size tier.
    /// TTS language packs are composable and do not use tier defaults.
    pub tier_default: bool,
    /// Approximate peak device memory during inference, including model
    /// weights and a modest runtime/context allowance. Not a hardware gate.
    pub estimated_vram_bytes: u64,
    pub parameters_millions: Option<u32>,
    pub benchmark: Option<ModelBenchmark>,
    pub provider: &'static str,
    /// Supported BCP-47 language tags. `zh-Hant` denotes Traditional Chinese;
    /// the legacy UI value `zh-TW` is normalized when checking routes.
    pub languages: &'static [&'static str],
    pub voice_presets: &'static [ModelVoicePreset],
    pub hardware: ModelHardwareRequirements,
    pub audio_output: Option<ModelAudioOutput>,
    /// Local execution contract. TTS packages use their existing provider
    /// adapters and therefore leave this unset.
    pub runtime: Option<ModelRuntime>,
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
