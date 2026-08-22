use super::super::{
    ModelArchiveEntry, ModelArchiveSource, ModelAssetId, ModelAssetManifest, ModelAudioOutput,
    ModelAudioSampleFormat, ModelCapability, ModelFileRole, ModelFileSource, ModelLevel,
    ModelSource, RequiredModelFile,
};

const OPENVOICE_ARCHIVE_ENTRIES: &[ModelArchiveEntry] = &[
    ModelArchiveEntry {
        relative_path: "model_config.json",
        archive_path: "{428E784B-61DC-4393-8F37-52C12DBAD791}/nvigi.model.config.json",
    },
    ModelArchiveEntry {
        relative_path: "models/bert.onnx",
        archive_path: "{428E784B-61DC-4393-8F37-52C12DBAD791}/Bert-Base-Uncased-QINT8.onnx",
    },
    ModelArchiveEntry {
        relative_path: "models/melo.onnx",
        archive_path: "{428E784B-61DC-4393-8F37-52C12DBAD791}/SynthesizerTrnBase_onnx16_v3_float16.onnx",
    },
    ModelArchiveEntry {
        relative_path: "models/converter.onnx",
        archive_path: "{428E784B-61DC-4393-8F37-52C12DBAD791}/SynthesizerTrnConverter_onnx16_v3_float16.onnx",
    },
    ModelArchiveEntry {
        relative_path: "frontend/cmudict.json",
        archive_path: "{428E784B-61DC-4393-8F37-52C12DBAD791}/cmudict.json",
    },
    ModelArchiveEntry {
        relative_path: "frontend/bert_vocab.txt",
        archive_path: "{428E784B-61DC-4393-8F37-52C12DBAD791}/vocab_tokenizer.txt",
    },
    ModelArchiveEntry {
        relative_path: "voices/en_newest.bin",
        archive_path: "{428E784B-61DC-4393-8F37-52C12DBAD791}/spectrograms_base/speaker0_se.bin",
    },
    ModelArchiveEntry {
        relative_path: "licenses/openvoice-melotts.txt",
        archive_path: "{428E784B-61DC-4393-8F37-52C12DBAD791}/licences/openVoice_melloTTS/LICENCE.txt",
    },
    ModelArchiveEntry {
        relative_path: "licenses/bert-apache-2.0.txt",
        archive_path: "{428E784B-61DC-4393-8F37-52C12DBAD791}/licences/bert_base_uncased/LICENCE.txt",
    },
    ModelArchiveEntry {
        relative_path: "licenses/cmudict.txt",
        archive_path: "{428E784B-61DC-4393-8F37-52C12DBAD791}/licences/CMUDict/ReadME.txt",
    },
];

const OPENVOICE_REQUIRED_FILES: &[RequiredModelFile] = &[
    RequiredModelFile {
        role: ModelFileRole::ModelConfig,
        relative_path: "model_config.json",
        purpose: "OpenVoice acoustic model configuration",
        bytes: 4_539,
        sha256: "d4747fbe59d10669cbdc4d819537005534934adc1da7b34e511f1e3ef3ff323b",
    },
    RequiredModelFile {
        role: ModelFileRole::BertGraph,
        relative_path: "models/bert.onnx",
        purpose: "BERT base uncased QINT8 text embedding graph",
        bytes: 95_482_314,
        sha256: "bb8ccc9916e02055b883386957f275797c5e320082ea7adb3f7c10a4af50236e",
    },
    RequiredModelFile {
        role: ModelFileRole::BaseTtsGraph,
        relative_path: "models/melo.onnx",
        purpose: "MeloTTS English v3 FP16 base voice graph",
        bytes: 86_056_493,
        sha256: "1edd539359d9b1a38080736f975077857b556b0377393d939e557d28f4c0f7fe",
    },
    RequiredModelFile {
        role: ModelFileRole::ToneConverterGraph,
        relative_path: "models/converter.onnx",
        purpose: "OpenVoice V2 FP16 tone-color converter graph",
        bytes: 66_284_171,
        sha256: "72e65cd74c273df1677f8941affcbfef974f48066ab71d0658b1ef4b42c90d8b",
    },
    RequiredModelFile {
        role: ModelFileRole::SpeakerEncoderGraph,
        relative_path: "models/reference_encoder.onnx",
        purpose: "OpenVoice V2 FP32 reference speaker encoder graph",
        bytes: 3_259_275,
        sha256: "3dd4918cab90e1acf7fa5c6f7539c27710e7a3cdfba550468c5ea49399178bf7",
    },
    RequiredModelFile {
        role: ModelFileRole::PronunciationDictionary,
        relative_path: "frontend/cmudict.json",
        purpose: "English CMU pronunciation dictionary",
        bytes: 4_498_493,
        sha256: "b0609f32b65f4d04466897a37fb55d3d5e877b65f3df46c145c3c2217b1ff55d",
    },
    RequiredModelFile {
        role: ModelFileRole::Vocabulary,
        relative_path: "frontend/bert_vocab.txt",
        purpose: "BERT base uncased WordPiece vocabulary",
        bytes: 231_508,
        sha256: "07eced375cec144d27c900241f3e339478dec958f92fddbc551f295c992038a3",
    },
    RequiredModelFile {
        role: ModelFileRole::SpeakerEmbedding,
        relative_path: "voices/en_newest.bin",
        purpose: "MeloTTS English v3 EN-Newest source speaker embedding",
        bytes: 1_024,
        sha256: "8c14e1a9e6db9eabda1e4a0c5f81bda5e1569fab6299bc65df7b13f7e25df34d",
    },
    RequiredModelFile {
        role: ModelFileRole::License,
        relative_path: "licenses/openvoice-melotts.txt",
        purpose: "OpenVoice and MeloTTS MIT license",
        bytes: 1_071,
        sha256: "7018bd16b0dca61f76e7a7c901c3e97b64a0636b9574ae8be36856cade9e11a1",
    },
    RequiredModelFile {
        role: ModelFileRole::License,
        relative_path: "licenses/bert-apache-2.0.txt",
        purpose: "BERT Apache 2.0 license",
        bytes: 11_502,
        sha256: "0c18145191a225d94bb6d36a1dbc993c73b56ffc480af12b000b7b1f493e7789",
    },
    RequiredModelFile {
        role: ModelFileRole::License,
        relative_path: "licenses/cmudict.txt",
        purpose: "CMU pronunciation dictionary license notice",
        bytes: 107,
        sha256: "cb5f74a5aa680e8253784fe72d4b4b8d0c5ae8ba253ea092a229c4a2acc42026",
    },
];

const OPENVOICE_FILE_OVERRIDES: &[ModelFileSource] = &[ModelFileSource {
    relative_path: "models/reference_encoder.onnx",
    repository: "TigreGotico/voiceclonnx-openvoice-v2",
    revision: "34d010c192c97f763207f488f6057fd07fee42ad",
    remote_path: "tone_ref_encoder.onnx",
}];

pub const OPENVOICE_V3_ONNX_FP16: ModelAssetManifest = ModelAssetManifest {
    id: ModelAssetId::OpenVoiceV3OnnxFp16,
    label: "OpenVoice v3 (English, ONNX FP16)",
    capability: ModelCapability::Tts,
    level: ModelLevel::Normal,
    provider: "openvoice",
    audio_output: Some(ModelAudioOutput {
        sample_rate_hz: 22_050,
        channels: 1,
        sample_format: ModelAudioSampleFormat::PcmI16Le,
    }),
    relative_directory: "OpenVoice-v3-ONNX-FP16",
    required_files: OPENVOICE_REQUIRED_FILES,
    source: ModelSource {
        repository: "nvidia/nvigisdk/openvoice",
        revision: "OpenVoice v3",
        include_patterns: &["*.zip"],
        file_overrides: OPENVOICE_FILE_OVERRIDES,
        archive: Some(ModelArchiveSource {
            filename: "openvoice-v3-ngc.zip",
            url: "https://api.ngc.nvidia.com/v2/models/nvidia/nvigisdk/openvoice/versions/OpenVoice%20v3/files/%7B428E784B-61DC-4393-8F37-52C12DBAD791%7D.zip",
            bytes: 204_513_198,
            sha256: "08b3e41a93cd598b2dc0b712da9d313d1cf54099d8a915f8a916deb1b2874a59",
            entries: OPENVOICE_ARCHIVE_ENTRIES,
        }),
    },
};
