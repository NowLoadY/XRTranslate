//! Language capability metadata shared by configuration and provider adapters.

use crate::{
    ModelAssetId, ModelAssetManifest, ModelCapability, ModelLevel, manifest_for,
    tier_default_manifest,
};

pub fn provider_model(
    provider: &str,
    asset: Option<&str>,
    capability: ModelCapability,
) -> Result<&'static ModelAssetManifest, String> {
    let model = match asset {
        Some(key) => ModelAssetId::from_config_key(key).map(manifest_for),
        None => tier_default_manifest(provider, capability, ModelLevel::Normal),
    }
    .ok_or_else(|| format!("No model card for {provider:?} {capability:?} ({asset:?})"))?;
    if model.provider != provider || model.capability != capability {
        return Err(format!(
            "Model {} does not belong to {provider:?} for {capability:?}",
            model.id.as_str()
        ));
    }
    Ok(model)
}
