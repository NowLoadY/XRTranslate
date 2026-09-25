include!(concat!(env!("OUT_DIR"), "/model_ids.rs"));
mod types;

pub use types::{
    AsrDelivery, AsrPromptStyle, CPU_MODEL_HARDWARE, MANAGED_LOCAL_MODEL_HARDWARE,
    MANAGED_LOCAL_MODEL_MINIMUM_VRAM_BYTES, MANAGED_SMALL_MODEL_HARDWARE,
    MANAGED_SMALL_MODEL_MINIMUM_VRAM_BYTES, ModelAccelerator, ModelArchiveEntry,
    ModelArchiveSource, ModelAssetManifest, ModelAudioOutput, ModelAudioSampleFormat,
    ModelBenchmark, ModelCapability, ModelFileRole, ModelFileSource, ModelHardwareRequirements,
    ModelLevel, ModelRuntime, ModelSource, ModelVoicePreset, RequiredModelFile,
    TranslationPromptStyle,
};

// All model cards, including download artifacts and language capabilities,
// come from one versioned JSON file. Cargo generates these static values.
include!(concat!(env!("OUT_DIR"), "/model_catalog.rs"));

pub fn manifests_for_capability(
    capability: ModelCapability,
) -> impl Iterator<Item = &'static ModelAssetManifest> {
    MODEL_ASSET_CATALOG
        .iter()
        .filter(move |manifest| manifest.capability == capability)
}

/// Returns the declared default package for one provider and user-facing tier.
/// Additional variants in the same tier never change this choice by catalogue order.
#[must_use]
pub fn tier_default_manifest(
    provider: &str,
    capability: ModelCapability,
    level: ModelLevel,
) -> Option<&'static ModelAssetManifest> {
    manifests_for_capability(capability).find(|manifest| {
        manifest.provider == provider && manifest.level == level && manifest.tier_default
    })
}

/// Returns the static manifest for `id`.
#[must_use]
pub fn manifest_for(id: ModelAssetId) -> &'static ModelAssetManifest {
    MODEL_ASSET_CATALOG
        .iter()
        .find(|manifest| manifest.id == id)
        .expect("every model asset id must have a catalog manifest")
}
