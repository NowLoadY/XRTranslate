//! Translation model packages.

use super::{
    ModelAssetId, ModelAssetManifest, ModelCapability, ModelFileRole, ModelLevel, ModelSource,
    RequiredModelFile,
};

const HUNYUAN_MT_REQUIRED_FILES: &[RequiredModelFile] = &[RequiredModelFile {
    role: ModelFileRole::Weights,
    relative_path: "Hy-MT2-1.8B-Q4_K_M.gguf",
    purpose: "Hy-MT2 quantized GGUF model",
    bytes: 1_133_080_448,
    sha256: "dc5f44fcf1fa496ee7ad725982c0c8c553a4de00259b53af84c4b89fb0c06699",
}];

const HUNYUAN_MT_7B_REQUIRED_FILES: &[RequiredModelFile] = &[RequiredModelFile {
    role: ModelFileRole::Weights,
    relative_path: "Hy-MT2-7B-Q4_K_M.gguf",
    purpose: "Hy-MT2 7B quantized GGUF model",
    bytes: 4_624_648_896,
    sha256: "9f96256500f3fc1ab4d64336b58f52a949a95ad7516b0c229476eef782f9f77b",
}];

pub const HUNYUAN_MT_GGUF: ModelAssetManifest = ModelAssetManifest {
    id: ModelAssetId::HunyuanMtGguf,
    label: "Translation Model",
    capability: ModelCapability::Translation,
    level: ModelLevel::Normal,
    provider: "hunyuan",
    audio_output: None,
    relative_directory: "HY-MT2",
    required_files: HUNYUAN_MT_REQUIRED_FILES,
    source: ModelSource {
        repository: "tencent/Hy-MT2-1.8B-GGUF",
        revision: "1cd5208700acedef4ef93019b6cfc148b8522d45",
        include_patterns: &["Hy-MT2-1.8B-Q4_K_M.gguf"],
        file_overrides: &[],
        archive: None,
    },
};

pub const HUNYUAN_MT_7B_GGUF: ModelAssetManifest = ModelAssetManifest {
    id: ModelAssetId::HunyuanMt7bGguf,
    label: "Translation Model",
    capability: ModelCapability::Translation,
    level: ModelLevel::Big,
    provider: "hunyuan",
    audio_output: None,
    relative_directory: "Hy-MT2-7B-GGUF",
    required_files: HUNYUAN_MT_7B_REQUIRED_FILES,
    source: ModelSource {
        repository: "tencent/Hy-MT2-7B-GGUF",
        revision: "707464294cf5b2a5a69982855020858ed58cf1d1",
        include_patterns: &["Hy-MT2-7B-Q4_K_M.gguf"],
        file_overrides: &[],
        archive: None,
    },
};
