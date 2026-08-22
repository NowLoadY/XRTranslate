use super::super::{
    ModelArchiveEntry, ModelArchiveSource, ModelAssetId, ModelAssetManifest, ModelAudioOutput,
    ModelAudioSampleFormat, ModelCapability, ModelFileRole, ModelFileSource, ModelLevel,
    ModelSource, ModelVoicePreset, RequiredModelFile,
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

const OPENVOICE_V3_VOICES: &[ModelVoicePreset] = &[ModelVoicePreset {
    key: "en-newest",
    label: "English — Newest",
    language: "en",
    is_default: true,
}];

pub const OPENVOICE_V3_ONNX_FP16: ModelAssetManifest = ModelAssetManifest {
    id: ModelAssetId::OpenVoiceV3OnnxFp16,
    label: "OpenVoice v3 (English, ONNX FP16)",
    capability: ModelCapability::Tts,
    level: ModelLevel::Normal,
    provider: "openvoice",
    languages: &["en"],
    voice_presets: OPENVOICE_V3_VOICES,
    hardware: super::super::MANAGED_LOCAL_MODEL_HARDWARE,
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
        remote_directory: "",
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

const OPENVOICE_V2_ARCHIVE_ENTRIES: &[ModelArchiveEntry] = &[
    ModelArchiveEntry {
        relative_path: "model_config.json",
        archive_path: "{09F5E010-5D94-413C-8852-ABC34464DDF8}/nvigi.model.config.json",
    },
    ModelArchiveEntry {
        relative_path: "models/bert.onnx",
        archive_path: "{09F5E010-5D94-413C-8852-ABC34464DDF8}/Bert-Base-Uncased-QINT8.onnx",
    },
    ModelArchiveEntry {
        relative_path: "models/melo.onnx",
        archive_path: "{09F5E010-5D94-413C-8852-ABC34464DDF8}/SynthesizerTrnBase_onnx16_v2_float16.onnx",
    },
    ModelArchiveEntry {
        relative_path: "models/converter.onnx",
        archive_path: "{09F5E010-5D94-413C-8852-ABC34464DDF8}/SynthesizerTrnConverter_onnx16_v2_float16.onnx",
    },
    ModelArchiveEntry {
        relative_path: "frontend/cmudict.json",
        archive_path: "{09F5E010-5D94-413C-8852-ABC34464DDF8}/cmudict.json",
    },
    ModelArchiveEntry {
        relative_path: "frontend/bert_vocab.txt",
        archive_path: "{09F5E010-5D94-413C-8852-ABC34464DDF8}/vocab_tokenizer.txt",
    },
    ModelArchiveEntry {
        relative_path: "voices/en_us.bin",
        archive_path: "{09F5E010-5D94-413C-8852-ABC34464DDF8}/spectrograms_base/speaker0_se.bin",
    },
    ModelArchiveEntry {
        relative_path: "voices/en_british.bin",
        archive_path: "{09F5E010-5D94-413C-8852-ABC34464DDF8}/spectrograms_base/speaker1_se.bin",
    },
    ModelArchiveEntry {
        relative_path: "voices/en_india.bin",
        archive_path: "{09F5E010-5D94-413C-8852-ABC34464DDF8}/spectrograms_base/speaker2_se.bin",
    },
    ModelArchiveEntry {
        relative_path: "voices/en_au.bin",
        archive_path: "{09F5E010-5D94-413C-8852-ABC34464DDF8}/spectrograms_base/speaker3_se.bin",
    },
    ModelArchiveEntry {
        relative_path: "voices/en_default.bin",
        archive_path: "{09F5E010-5D94-413C-8852-ABC34464DDF8}/spectrograms_base/speaker4_se.bin",
    },
    ModelArchiveEntry {
        relative_path: "licenses/openvoice-melotts.txt",
        archive_path: "{09F5E010-5D94-413C-8852-ABC34464DDF8}/licences/openVoice_melloTTS/LICENCE.txt",
    },
    ModelArchiveEntry {
        relative_path: "licenses/bert-apache-2.0.txt",
        archive_path: "{09F5E010-5D94-413C-8852-ABC34464DDF8}/licences/bert_base_uncased/LICENCE.txt",
    },
    ModelArchiveEntry {
        relative_path: "licenses/cmudict.txt",
        archive_path: "{09F5E010-5D94-413C-8852-ABC34464DDF8}/licences/CMUDict/ReadME.txt",
    },
];

const OPENVOICE_V2_REQUIRED_FILES: &[RequiredModelFile] = &[
    RequiredModelFile {
        role: ModelFileRole::ModelConfig,
        relative_path: "model_config.json",
        purpose: "OpenVoice v2 acoustic model configuration",
        bytes: 5_221,
        sha256: "6b04f64591b436f0aa5b70f67669ab2e3f6daeb5bda1047ee3f78426a4c33e0a",
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
        purpose: "MeloTTS English v2 FP16 multi-accent base voice graph",
        bytes: 86_187_824,
        sha256: "88007ebc449e77f9be964be73ea0e96bf8aa29e879652a1005a590e52805dadc",
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
        relative_path: "voices/en_us.bin",
        purpose: "MeloTTS English v2 American source speaker embedding",
        bytes: 1_024,
        sha256: "6c710b5d75bdaff359cc3b81b646e5b87376c9dcfe478f215d9b1e68ccd67cd4",
    },
    RequiredModelFile {
        role: ModelFileRole::SpeakerEmbedding,
        relative_path: "voices/en_british.bin",
        purpose: "MeloTTS English v2 British source speaker embedding",
        bytes: 1_024,
        sha256: "f85ace7f50c2179bb74f96491c566a329fe2feab4107a8b82780e795927fab03",
    },
    RequiredModelFile {
        role: ModelFileRole::SpeakerEmbedding,
        relative_path: "voices/en_india.bin",
        purpose: "MeloTTS English v2 Indian source speaker embedding",
        bytes: 1_024,
        sha256: "caf0a7018db11607f35a597be5ddf5e5bf7da5c738a29a0f3d37cfab49575603",
    },
    RequiredModelFile {
        role: ModelFileRole::SpeakerEmbedding,
        relative_path: "voices/en_au.bin",
        purpose: "MeloTTS English v2 Australian source speaker embedding",
        bytes: 1_024,
        sha256: "1074ec1809c39f0ffff4e1327323ab39d876689144610d2559a845ed32c2b9e8",
    },
    RequiredModelFile {
        role: ModelFileRole::SpeakerEmbedding,
        relative_path: "voices/en_default.bin",
        purpose: "MeloTTS English v2 default source speaker embedding",
        bytes: 1_024,
        sha256: "1b1a57fa0159dffb761901966baa2c0cba08bfe04f75080c1827bf4c7b6e4180",
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

const OPENVOICE_V2_VOICES: &[ModelVoicePreset] = &[
    ModelVoicePreset {
        key: "en-us",
        label: "English — American",
        language: "en",
        is_default: true,
    },
    ModelVoicePreset {
        key: "en-british",
        label: "English — British",
        language: "en",
        is_default: false,
    },
    ModelVoicePreset {
        key: "en-india",
        label: "English — Indian",
        language: "en",
        is_default: false,
    },
    ModelVoicePreset {
        key: "en-au",
        label: "English — Australian",
        language: "en",
        is_default: false,
    },
    ModelVoicePreset {
        key: "en-default",
        label: "English — Default",
        language: "en",
        is_default: false,
    },
];

pub const OPENVOICE_V2_ONNX_FP16: ModelAssetManifest = ModelAssetManifest {
    id: ModelAssetId::OpenVoiceV2OnnxFp16,
    label: "OpenVoice v2 (English multi-accent, ONNX FP16)",
    capability: ModelCapability::Tts,
    level: ModelLevel::Normal,
    provider: "openvoice",
    languages: &["en"],
    voice_presets: OPENVOICE_V2_VOICES,
    hardware: super::super::MANAGED_LOCAL_MODEL_HARDWARE,
    audio_output: Some(ModelAudioOutput {
        sample_rate_hz: 22_050,
        channels: 1,
        sample_format: ModelAudioSampleFormat::PcmI16Le,
    }),
    relative_directory: "OpenVoice-v2-ONNX-FP16",
    required_files: OPENVOICE_V2_REQUIRED_FILES,
    source: ModelSource {
        repository: "nvidia/nvigisdk/openvoice",
        revision: "OpenVoice v2",
        remote_directory: "",
        include_patterns: &["*.zip"],
        file_overrides: OPENVOICE_FILE_OVERRIDES,
        archive: Some(ModelArchiveSource {
            filename: "openvoice-v2-ngc.zip",
            url: "https://api.ngc.nvidia.com/v2/models/nvidia/nvigisdk/openvoice/versions/OpenVoice%20v2/files/%7B09F5E010-5D94-413C-8852-ABC34464DDF8%7D.zip",
            bytes: 204_579_050,
            sha256: "266dc4662965858e07a1c8cb086f17e1c30f0fdc3202e8934103dc7927314811",
            entries: OPENVOICE_V2_ARCHIVE_ENTRIES,
        }),
    },
};

const OPENVOICE_V2_ZH_REQUIRED_FILES: &[RequiredModelFile] = &[
    RequiredModelFile {
        role: ModelFileRole::RuntimeManifest,
        relative_path: "package-manifest.json",
        purpose: "Reproducible conversion provenance and graph contract",
        bytes: 7_646,
        sha256: "72bc0596b9cfa9f337181865d5516903059620fc8f73e497b92c4c810c5df1e8",
    },
    RequiredModelFile {
        role: ModelFileRole::ModelConfig,
        relative_path: "model_config.json",
        purpose: "MeloTTS Chinese acoustic model configuration",
        bytes: 2_296,
        sha256: "d58b5acdab89ad2bbd65325affab309ae3cb964834b02f9a60587474e81c8bb9",
    },
    RequiredModelFile {
        role: ModelFileRole::BaseTtsGraph,
        relative_path: "models/melo.onnx",
        purpose: "MeloTTS Chinese mixed-English FP32-text/FP16-acoustic base voice graph",
        bytes: 100_934_380,
        sha256: "b5ad831b584c5a4c289e4a39c3821cefde0df794614dbc28ee56c9770154e8e6",
    },
    RequiredModelFile {
        role: ModelFileRole::BertGraph,
        relative_path: "models/bert.onnx",
        purpose: "Multilingual BERT FP16 text embedding graph",
        bytes: 305_406_373,
        sha256: "ef99a7db948fa82616c5407808ac9c04691a53afc106cc284f84c22a9f6ed353",
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
        role: ModelFileRole::Vocabulary,
        relative_path: "frontend/bert_vocab.txt",
        purpose: "Multilingual BERT WordPiece vocabulary",
        bytes: 871_891,
        sha256: "87b44292b452f6c05afa49b2e488e7eedf79ea4f4c39db6f2f4b37764228ef3f",
    },
    RequiredModelFile {
        role: ModelFileRole::PhonemeMap,
        relative_path: "frontend/opencpop-strict.txt",
        purpose: "OpenCPOP Chinese pinyin-to-phone mapping",
        bytes: 4_513,
        sha256: "86c4b30928e3a4305c9148058c9e2e56b04ce741363fedff382421f4a1e3709d",
    },
    RequiredModelFile {
        role: ModelFileRole::LanguageLexicon,
        relative_path: "frontend/chinese_lexicon.json",
        purpose: "Pinned Chinese phrase and pinyin lexicon",
        bytes: 1_985_652,
        sha256: "5c1ba81aa69c6d79ef413256d3e274b1b832434df373ee10929b2ffc059f5752",
    },
    RequiredModelFile {
        role: ModelFileRole::PronunciationDictionary,
        relative_path: "frontend/cmudict.json",
        purpose: "English CMU pronunciation dictionary for code-switching",
        bytes: 4_498_493,
        sha256: "b0609f32b65f4d04466897a37fb55d3d5e877b65f3df46c145c3c2217b1ff55d",
    },
    RequiredModelFile {
        role: ModelFileRole::SpeakerEmbedding,
        relative_path: "voices/zh.bin",
        purpose: "MeloTTS Chinese source speaker embedding",
        bytes: 1_024,
        sha256: "1e185b041cde958f51990c736e4f5fa878f3be5ae088fd6441b6ff5a2cd82932",
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
        relative_path: "licenses/apache-2.0.txt",
        purpose: "BERT Apache 2.0 license",
        bytes: 11_502,
        sha256: "0c18145191a225d94bb6d36a1dbc993c73b56ffc480af12b000b7b1f493e7789",
    },
    RequiredModelFile {
        role: ModelFileRole::License,
        relative_path: "licenses/cmudict.txt",
        purpose: "CMU pronunciation dictionary license notice",
        bytes: 1_754,
        sha256: "bd4ce8e44170a5f9f481310ca85c51de3c4f851a65e679b40e603b143bd3542a",
    },
    RequiredModelFile {
        role: ModelFileRole::License,
        relative_path: "licenses/pypinyin.txt",
        purpose: "pypinyin MIT license",
        bytes: 1_105,
        sha256: "1e6c90014b4912815c296ee64bb6f6280af47e6d4c5d80e86232dfc5defe764c",
    },
    RequiredModelFile {
        role: ModelFileRole::License,
        relative_path: "licenses/chinese-frontend-notice.txt",
        purpose: "Chinese frontend generated-data provenance notice",
        bytes: 178,
        sha256: "998ef5c85f099aeb285102935b1ac21345e37727ac16987164dd69015082b034",
    },
];

const OPENVOICE_V2_ZH_VOICES: &[ModelVoicePreset] = &[ModelVoicePreset {
    key: "zh-default",
    label: "Chinese — Default",
    language: "zh",
    is_default: true,
}];

pub const OPENVOICE_V2_ZH_ONNX_FP16: ModelAssetManifest = ModelAssetManifest {
    id: ModelAssetId::OpenVoiceV2ZhOnnxFp16,
    label: "OpenVoice v2 (Chinese, ONNX FP16)",
    capability: ModelCapability::Tts,
    level: ModelLevel::Normal,
    provider: "openvoice",
    languages: &["zh"],
    voice_presets: OPENVOICE_V2_ZH_VOICES,
    hardware: super::super::MANAGED_LOCAL_MODEL_HARDWARE,
    audio_output: Some(ModelAudioOutput {
        // The Chinese Melo base graph emits 44.1 kHz internally; OpenVoice's
        // converter contract returns the provider-standard 22.05 kHz PCM.
        sample_rate_hz: 22_050,
        channels: 1,
        sample_format: ModelAudioSampleFormat::PcmI16Le,
    }),
    relative_directory: "OpenVoice-v2-ZH-ONNX-FP16",
    required_files: OPENVOICE_V2_ZH_REQUIRED_FILES,
    source: ModelSource {
        repository: "NowLoadY/XRTranslate-OpenVoice-ONNX",
        revision: "8a5782785c7f728692057eab37e9a3645b5747f8",
        remote_directory: "packages/zh/v1",
        include_patterns: &["packages/zh/v1/**"],
        file_overrides: &[],
        archive: None,
    },
};
