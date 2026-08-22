use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    MODEL_ASSET_CATALOG, ModelAssetId, ModelAssetManifest, ModelCapability, ModelFileRole,
    manifest_for,
};

/// Optional path overrides read from the model-manager configuration.
///
/// Every relative value is interpreted relative to the project root, never to
/// the current working directory. In an unchanged `config.json` this struct is
/// simply its default and paths resolve beneath `<project-root>/models`.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelAssetsConfig {
    /// Override the default `<project-root>/models` directory.
    #[serde(alias = "models_root", alias = "model_root")]
    pub models_directory: Option<PathBuf>,
    /// Provider-neutral active package selection. Selection cardinality is a
    /// capability rule: ASR and translation replace; TTS language packs compose.
    active_assets: Vec<ModelAssetId>,
    /// Legacy Qwen3 ASR package-directory override retained for config
    /// compatibility. It does not apply to other providers or asset IDs.
    pub qwen3_asr_gguf_directory: Option<PathBuf>,
    /// Legacy Hy-MT2 1.8B package-directory override retained for config
    /// compatibility. It does not apply to other levels or providers.
    pub hunyuan_mt_gguf_directory: Option<PathBuf>,
    /// Legacy active ASR selection. Prefer [`Self::select_asset`].
    pub qwen3_asr_asset: Option<ModelAssetId>,
    /// Legacy active translation selection. Prefer [`Self::select_asset`].
    pub hunyuan_mt_asset: Option<ModelAssetId>,
}

impl ModelAssetsConfig {
    /// Builds path configuration from the model-manager compatibility fields.
    #[must_use]
    pub fn with_directory_overrides(
        models_directory: Option<PathBuf>,
        qwen3_asr_gguf_directory: Option<PathBuf>,
        hunyuan_mt_gguf_directory: Option<PathBuf>,
    ) -> Self {
        Self {
            models_directory,
            qwen3_asr_gguf_directory,
            hunyuan_mt_gguf_directory,
            ..Self::default()
        }
    }

    /// Selects a package for its declared capability without matching a model
    /// family in the caller.
    pub fn select_asset(&mut self, id: ModelAssetId) {
        let capability = manifest_for(id).capability;
        if capability.allows_multiple_assets() {
            let next = manifest_for(id);
            self.active_assets.retain(|selected| {
                let existing = manifest_for(*selected);
                existing.provider != next.provider
                    || existing.languages.is_empty()
                    || next.languages.is_empty()
                    || existing.languages.iter().all(|language| {
                        !next.languages.iter().any(|candidate| candidate == language)
                    })
            });
            if !self.active_assets.contains(&id) {
                self.active_assets.push(id);
            }
            return;
        }
        if let Some(selected) = self
            .active_assets
            .iter_mut()
            .find(|selected| manifest_for(**selected).capability == capability)
        {
            *selected = id;
        } else {
            self.active_assets.push(id);
        }
    }

    pub fn deselect_asset(&mut self, id: ModelAssetId) {
        self.active_assets.retain(|selected| *selected != id);
    }

    /// Iterates the normalized explicit selections in insertion order.
    pub fn selected_asset_ids(&self) -> impl ExactSizeIterator<Item = ModelAssetId> + '_ {
        self.active_assets.iter().copied()
    }

    /// Resolves configured paths with a stable project-root base.
    #[must_use]
    pub fn resolve(&self, project_root: impl AsRef<Path>) -> ResolvedModelAssets {
        self.resolve_inner(project_root, true)
    }

    /// Resolves paths while treating only explicitly selected packages as
    /// active. This is used by provider-driven flows where an empty selection
    /// means every active provider is remote.
    #[must_use]
    pub fn resolve_selected(&self, project_root: impl AsRef<Path>) -> ResolvedModelAssets {
        self.resolve_inner(project_root, false)
    }

    fn resolve_inner(
        &self,
        project_root: impl AsRef<Path>,
        include_default_assets: bool,
    ) -> ResolvedModelAssets {
        let project_root = project_root.as_ref().to_path_buf();
        let models_directory = resolve_from_project_root(
            &project_root,
            self.models_directory
                .as_deref()
                .unwrap_or_else(|| Path::new("models")),
        );
        let qwen3_id = self
            .selected_asset(ModelCapability::Asr)
            .or(self.qwen3_asr_asset)
            .unwrap_or(ModelAssetId::Qwen3AsrGguf);
        let hunyuan_id = self
            .selected_asset(ModelCapability::Translation)
            .or(self.hunyuan_mt_asset)
            .unwrap_or(ModelAssetId::HunyuanMtGguf);
        let audio8_id = self
            .selected_asset(ModelCapability::Tts)
            .unwrap_or(ModelAssetId::Audio8TtsOnnxFp16);

        let catalog = MODEL_ASSET_CATALOG
            .iter()
            .map(|manifest| {
                // These two fields predate the catalog. Keep their path
                // meaning tied to the original package without making new
                // catalog IDs modify this resolver.
                let configured_directory = if manifest.id == ModelAssetId::Qwen3AsrGguf {
                    self.qwen3_asr_gguf_directory.as_deref()
                } else if manifest.id == ModelAssetId::HunyuanMtGguf {
                    self.hunyuan_mt_gguf_directory.as_deref()
                } else {
                    None
                };
                let directory = configured_directory
                    .map(|path| resolve_from_project_root(&project_root, path))
                    .unwrap_or_else(|| models_directory.join(manifest.relative_directory));
                ResolvedModelAsset::new(manifest, directory)
            })
            .collect::<Vec<_>>();

        let qwen3_asr = catalog_asset(&catalog, qwen3_id).clone();
        let hunyuan_mt = catalog_asset(&catalog, hunyuan_id).clone();
        let audio8_tts = catalog_asset(&catalog, audio8_id).clone();
        let active_asset_ids = if include_default_assets && self.active_assets.is_empty() {
            vec![qwen3_id, hunyuan_id]
        } else {
            self.active_assets.clone()
        };
        ResolvedModelAssets {
            project_root,
            models_directory,
            qwen3_asr,
            hunyuan_mt,
            audio8_tts,
            active_asset_ids,
            catalog,
        }
    }

    fn selected_asset(&self, capability: ModelCapability) -> Option<ModelAssetId> {
        self.active_assets
            .iter()
            .copied()
            .find(|id| manifest_for(*id).capability == capability)
    }
}

fn catalog_asset(catalog: &[ResolvedModelAsset], id: ModelAssetId) -> &ResolvedModelAsset {
    catalog
        .iter()
        .find(|asset| asset.manifest.id == id)
        .unwrap_or_else(|| panic!("model asset {id} has no resolved catalog manifest"))
}

/// Resolves a path relative to `project_root`, preserving absolute paths.
#[must_use]
pub fn resolve_from_project_root(project_root: &Path, configured_path: &Path) -> PathBuf {
    if configured_path.is_absolute() {
        configured_path.to_path_buf()
    } else {
        project_root.join(configured_path)
    }
}

/// The active runtime model packages plus the complete installable catalog.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedModelAssets {
    pub project_root: PathBuf,
    pub models_directory: PathBuf,
    /// Active ASR package. Prefer [`Self::active_asset`] in new code.
    pub qwen3_asr: ResolvedModelAsset,
    /// Active translation package. Prefer [`Self::active_asset`] in new code.
    pub hunyuan_mt: ResolvedModelAsset,
    /// Active TTS package. Prefer [`Self::active_asset`] in new code.
    pub audio8_tts: ResolvedModelAsset,
    active_asset_ids: Vec<ModelAssetId>,
    catalog: Vec<ResolvedModelAsset>,
}

impl ResolvedModelAssets {
    /// Builds the default project layout without any configuration overrides.
    #[must_use]
    pub fn for_project_root(project_root: impl AsRef<Path>) -> Self {
        ModelAssetsConfig::default().resolve(project_root)
    }

    /// Returns the deterministic paths needed by the two default
    /// `llama-server` specifications. Call [`Self::check`] before spawning.
    #[must_use]
    pub fn llama_cpp_paths(&self) -> DefaultLlamaCppPaths {
        DefaultLlamaCppPaths {
            qwen3_asr_model: self
                .qwen3_asr
                .file_path(ModelFileRole::Weights)
                .expect("default ASR manifest declares weights"),
            qwen3_asr_mmproj: self
                .qwen3_asr
                .file_path(ModelFileRole::MultimodalProjection)
                .expect("default ASR manifest declares a multimodal projection"),
            hunyuan_mt_model: self
                .hunyuan_mt
                .file_path(ModelFileRole::Weights)
                .expect("default translation manifest declares weights"),
        }
    }

    /// Returns the resolved install location for any catalog package.
    #[must_use]
    pub fn asset(&self, id: ModelAssetId) -> &ResolvedModelAsset {
        catalog_asset(&self.catalog, id)
    }

    /// Iterates every installable package in manifest order.
    pub fn catalog_assets(&self) -> impl ExactSizeIterator<Item = &ResolvedModelAsset> {
        self.catalog.iter()
    }

    /// Returns the active package for a runtime capability.
    #[must_use]
    pub fn active_asset(&self, capability: ModelCapability) -> &ResolvedModelAsset {
        match capability {
            ModelCapability::Asr => &self.qwen3_asr,
            ModelCapability::Translation => &self.hunyuan_mt,
            ModelCapability::Tts => &self.audio8_tts,
        }
    }

    /// Iterates every active package for a capability in configuration order.
    pub fn active_assets_for(
        &self,
        capability: ModelCapability,
    ) -> impl Iterator<Item = &ResolvedModelAsset> {
        self.active_asset_ids
            .iter()
            .map(|id| self.asset(*id))
            .filter(move |asset| asset.manifest.capability == capability)
    }

    /// Iterates active runtime packages in ASR, translation order.
    pub fn active_assets(&self) -> impl ExactSizeIterator<Item = &ResolvedModelAsset> {
        self.active_asset_ids.iter().map(|id| self.asset(*id))
    }
}

/// Concrete paths consumed by the default Qwen3-ASR and Hy-MT2 servers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DefaultLlamaCppPaths {
    pub qwen3_asr_model: PathBuf,
    pub qwen3_asr_mmproj: PathBuf,
    pub hunyuan_mt_model: PathBuf,
}

/// A resolved static manifest and the directory where it should be installed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedModelAsset {
    pub(crate) manifest: &'static ModelAssetManifest,
    pub(crate) directory: PathBuf,
}

impl ResolvedModelAsset {
    pub(crate) fn new(manifest: &'static ModelAssetManifest, directory: PathBuf) -> Self {
        Self {
            manifest,
            directory,
        }
    }

    #[must_use]
    pub const fn manifest(&self) -> &'static ModelAssetManifest {
        self.manifest
    }

    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Resolves a declared required file by its manifest index.
    #[must_use]
    pub fn required_file_path(&self, index: usize) -> PathBuf {
        let required_file = self.manifest.required_files.get(index).unwrap_or_else(|| {
            panic!(
                "model asset {} has no required file at index {index}",
                self.manifest.id
            )
        });
        self.directory.join(required_file.relative_path)
    }

    /// Resolves a required file by runtime role rather than manifest order.
    #[must_use]
    pub fn file_path(&self, role: ModelFileRole) -> Option<PathBuf> {
        self.manifest
            .required_files
            .iter()
            .find(|file| file.role == role)
            .map(|file| self.directory.join(file.relative_path))
    }
}
