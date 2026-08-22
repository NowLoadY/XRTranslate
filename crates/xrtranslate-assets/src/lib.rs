//! Local model manifests, resolution, installation, and preflight checks.
//!
//! Backend startup remains read-only. Explicit installers may download the
//! immutable assets declared by the catalog.

#![forbid(unsafe_code)]

mod catalog;
mod install;
mod preflight;
mod resolve;

pub use catalog::{
    AUDIO8_TTS_ONNX_FP16, DEFAULT_GGUF_MANIFEST, HUNYUAN_MT_7B_GGUF, HUNYUAN_MT_GGUF,
    MANAGED_LOCAL_MODEL_HARDWARE, MANAGED_LOCAL_MODEL_MINIMUM_VRAM_BYTES, MODEL_ASSET_CATALOG,
    ModelAccelerator, ModelArchiveEntry, ModelArchiveSource, ModelAssetId, ModelAssetManifest,
    ModelAudioOutput, ModelAudioSampleFormat, ModelCapability, ModelFileRole, ModelFileSource,
    ModelHardwareRequirements, ModelLevel, ModelSource, ModelVoicePreset, OPENVOICE_V2_ONNX_FP16,
    OPENVOICE_V3_ONNX_FP16, QWEN3_ASR_GGUF, RequiredModelFile, manifest_for,
    manifests_for_capability,
};
pub use install::{
    AtomicInstallError, DownloadProgress, ModelDownloadError, NativeModelInstaller,
    clear_model_staging, remove_model_asset,
};
pub use preflight::{
    ModelAssetDiagnostic, ModelAssetProblem, ModelAssetsPreflight, ModelAssetsPreflightError,
};
pub use resolve::{
    DefaultLlamaCppPaths, ModelAssetsConfig, ResolvedModelAsset, ResolvedModelAssets,
    resolve_from_project_root,
};

#[cfg(test)]
mod tests;
