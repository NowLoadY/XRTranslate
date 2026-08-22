//! Audio8 model package.

use super::super::{
    ModelAssetId, ModelAssetManifest, ModelAudioOutput, ModelAudioSampleFormat, ModelCapability,
    ModelFileRole, ModelFileSource, ModelLevel, ModelSource, RequiredModelFile,
};

const REQUIRED_FILES: &[RequiredModelFile] = &[
    RequiredModelFile {
        role: ModelFileRole::SlowArGraph,
        relative_path: "slow_ar_fp16.onnx",
        purpose: "Audio8 slow autoregressive FP16 ONNX graph",
        bytes: 1_348_765_672,
        sha256: "7b58e12eddca63b45d52a833d6f697b02d5431a8538b6cd8f1b115ecd9bded82",
    },
    RequiredModelFile {
        role: ModelFileRole::FastArGraph,
        relative_path: "fast_ar_fp16.onnx",
        purpose: "Audio8 fast autoregressive FP16 ONNX graph",
        bytes: 134_041_582,
        sha256: "33dd894dc73dad4c3f74fc1a3505b88a2489684a441052bc94fc6700ea106ccd",
    },
    RequiredModelFile {
        role: ModelFileRole::CodecDecoder,
        relative_path: "codec_decoder_fp16.onnx",
        purpose: "Audio8 FP16 codec decoder graph",
        bytes: 594_319,
        sha256: "6e379be31db6c1b0c111e0e3d2aeb10717ee96b197462b926de411e75a1fd019",
    },
    RequiredModelFile {
        role: ModelFileRole::CodecDecoderData,
        relative_path: "codec_decoder_fp16.onnx.data",
        purpose: "Audio8 FP16 codec decoder weights",
        bytes: 260_741_440,
        sha256: "18838f686aa7c1528fb69ec11e1ab404fdc4dc823d13219abfd4b327988527c0",
    },
    RequiredModelFile {
        role: ModelFileRole::CodecEncoder,
        relative_path: "registration/codec_encoder_fp16.onnx",
        purpose: "Audio8 voice registration codec encoder",
        bytes: 940_787,
        sha256: "e856d7999442cdc8f1f2ed0d2c055532cf359f0dd6d9a44fd4b98584c5d5dfa5",
    },
    RequiredModelFile {
        role: ModelFileRole::CodecEncoderData,
        relative_path: "registration/codec_encoder_fp16.onnx.data",
        purpose: "Audio8 voice registration codec encoder weights",
        bytes: 414_425_088,
        sha256: "19c740fcc4d45aa2546e9ab86e31c6200955c4b0a139758296fbf1064bf009cd",
    },
    RequiredModelFile {
        role: ModelFileRole::RuntimeManifest,
        relative_path: "runtime_manifest.json",
        purpose: "Audio8 runtime manifest",
        bytes: 1_080,
        sha256: "6473ae7d0106a2e369e442c72a71d2d46d8fbd3fe18c80d80b1b46e4aa241930",
    },
    RequiredModelFile {
        role: ModelFileRole::RegistrationManifest,
        relative_path: "registration/registration_manifest.json",
        purpose: "Audio8 voice registration manifest",
        bytes: 165,
        sha256: "36ef9d2f435f0f7b5ab66dc78a44411a24c0ab9e3a2c63738babe575747a584f",
    },
    RequiredModelFile {
        role: ModelFileRole::Tokenizer,
        relative_path: "tokenizer/tokenizer.json",
        purpose: "Audio8 tokenizer",
        bytes: 12_217_872,
        sha256: "f24e08099d45a8adf3f52f5f0b03276e433bb9d689bb15fcbcc48ce58744588b",
    },
];

const FP16_SOURCE_OVERRIDES: &[ModelFileSource] = &[
    ModelFileSource {
        relative_path: "slow_ar_fp16.onnx",
        repository: "OpenVoiceOS/phoonnx-audio8-tts",
        revision: "6e4de996325cebb25df81efd6b0adc08792cd21f",
        remote_path: "slow_ar_fp16.onnx",
    },
    ModelFileSource {
        relative_path: "fast_ar_fp16.onnx",
        repository: "OpenVoiceOS/phoonnx-audio8-tts",
        revision: "6e4de996325cebb25df81efd6b0adc08792cd21f",
        remote_path: "fast_ar_fp16.onnx",
    },
];

pub const AUDIO8_TTS_ONNX_FP16: ModelAssetManifest = ModelAssetManifest {
    id: ModelAssetId::Audio8TtsOnnxFp16,
    label: "Audio8 TTS (ONNX FP16, slow)",
    capability: ModelCapability::Tts,
    level: ModelLevel::Normal,
    provider: "audio8",
    languages: &[],
    voice_presets: &[],
    hardware: super::super::MANAGED_LOCAL_MODEL_HARDWARE,
    audio_output: Some(ModelAudioOutput {
        sample_rate_hz: 44_100,
        channels: 1,
        sample_format: ModelAudioSampleFormat::PcmI16Le,
    }),
    relative_directory: "Audio8-TTS-Preview-0.6B-ONNX-FP16",
    required_files: REQUIRED_FILES,
    source: ModelSource {
        repository: "Audio8/Audio8-TTS-Preview-0.6B-ONNX-INT4",
        revision: "818569c6b832118ad68d61bbd873abe250fcd68a",
        remote_directory: "",
        include_patterns: &["*.onnx", "*.onnx.data", "*.json"],
        file_overrides: FP16_SOURCE_OVERRIDES,
        archive: None,
    },
};
