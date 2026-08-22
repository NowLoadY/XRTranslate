mod asr;
mod translation;
mod tts;
mod types;

pub use asr::QWEN3_ASR_GGUF;
pub use translation::{HUNYUAN_MT_7B_GGUF, HUNYUAN_MT_GGUF};
pub use tts::{AUDIO8_TTS_ONNX_FP16, OPENVOICE_V3_ONNX_FP16};
pub use types::{
    ModelArchiveEntry, ModelArchiveSource, ModelAssetId, ModelAssetManifest, ModelAudioOutput,
    ModelAudioSampleFormat, ModelCapability, ModelFileRole, ModelFileSource, ModelLevel,
    ModelSource, RequiredModelFile,
};

/// Complete immutable catalog of native model packages.
pub const MODEL_ASSET_CATALOG: &[ModelAssetManifest] = &[
    QWEN3_ASR_GGUF,
    HUNYUAN_MT_GGUF,
    HUNYUAN_MT_7B_GGUF,
    AUDIO8_TTS_ONNX_FP16,
    OPENVOICE_V3_ONNX_FP16,
];

/// Compatibility name retained for callers of the original GGUF-only catalog.
pub const DEFAULT_GGUF_MANIFEST: &[ModelAssetManifest] = MODEL_ASSET_CATALOG;

pub fn manifests_for_capability(
    capability: ModelCapability,
) -> impl Iterator<Item = &'static ModelAssetManifest> {
    MODEL_ASSET_CATALOG
        .iter()
        .filter(move |manifest| manifest.capability == capability)
}

/// Returns the static manifest for `id`.
#[must_use]
pub fn manifest_for(id: ModelAssetId) -> &'static ModelAssetManifest {
    MODEL_ASSET_CATALOG
        .iter()
        .find(|manifest| manifest.id == id)
        .expect("every model asset id must have a catalog manifest")
}
