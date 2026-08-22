//! ASR model packages.

use super::{
    ModelAssetId, ModelAssetManifest, ModelCapability, ModelFileRole, ModelLevel, ModelSource,
    RequiredModelFile,
};

const REQUIRED_FILES: &[RequiredModelFile] = &[
    RequiredModelFile {
        role: ModelFileRole::Weights,
        relative_path: "Qwen3-ASR-1.7B.Q4_K_M.gguf",
        purpose: "Qwen3-ASR quantized GGUF model",
        bytes: 1_282_435_552,
        sha256: "3893b8926065bbff3da7586d21d8711a9b4fa4fa8f12cd0cefad58e31b2660b6",
    },
    RequiredModelFile {
        role: ModelFileRole::MultimodalProjection,
        relative_path: "Qwen3-ASR-1.7B.mmproj-f16.gguf",
        purpose: "Qwen3-ASR multimodal projection GGUF",
        bytes: 641_774_112,
        sha256: "5bc361e19bfdf3617c85247f9b706f7186ce0d156d9ed3c5d8bca8900b8fc3b7",
    },
];

pub const QWEN3_ASR_GGUF: ModelAssetManifest = ModelAssetManifest {
    id: ModelAssetId::Qwen3AsrGguf,
    label: "Speech Recognition Model",
    capability: ModelCapability::Asr,
    level: ModelLevel::Normal,
    provider: "qwen3-gguf",
    audio_output: None,
    relative_directory: "Qwen3-ASR-1.7B-GGUF",
    required_files: REQUIRED_FILES,
    source: ModelSource {
        repository: "mradermacher/Qwen3-ASR-1.7B-GGUF",
        revision: "cc946c78d3804752f7ba1bc42720c0f7aaf3d1ad",
        include_patterns: &["*Q4_K_M.gguf", "*mmproj-f16.gguf"],
        file_overrides: &[],
        archive: None,
    },
};
