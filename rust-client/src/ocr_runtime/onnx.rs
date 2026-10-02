use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

use image::RgbImage;
use oar_ocr::{
    core::config::OrtSessionConfig,
    domain::tasks::{TextDetectionConfig, TextRecognitionConfig},
    oarocr::{OAROCR, OAROCRBuilder},
    processors::LimitType,
};
use serde::Deserialize;

pub(super) struct OnnxOcr(OAROCR);

impl OnnxOcr {
    pub(super) fn load(
        runtime: &Path,
        detection: &Path,
        recognition: &Path,
        model_config: &Path,
    ) -> Result<Self, String> {
        initialize_runtime(runtime)?;
        let config: RecognitionConfig = serde_yaml_ng::from_slice(
            &std::fs::read(model_config).map_err(|error| error.to_string())?,
        )
        .map_err(|error| format!("Cannot read OCR character dictionary: {error}"))?;
        if config.post_process.character_dict.is_empty() {
            return Err("OCR character dictionary is empty.".into());
        }
        let cores = std::thread::available_parallelism().map_or(2, usize::from);
        // Leave CPU capacity for live audio, translation, and the interface.
        let threads = (cores / 2).clamp(1, 4);
        OAROCRBuilder::new(detection, recognition, model_config)
            .character_dict_content(config.post_process.character_dict.join("\n"))
            .ort_session(OrtSessionConfig::new().with_intra_threads(threads))
            .text_detection_config(TextDetectionConfig {
                score_threshold: 0.2,
                box_threshold: 0.45,
                unclip_ratio: 1.4,
                max_candidates: 3000,
                limit_side_len: Some(1536),
                limit_type: Some(LimitType::Max),
                max_side_len: Some(4096),
            })
            .text_recognition_config(TextRecognitionConfig {
                score_threshold: 0.5,
            })
            .build()
            .map(Self)
            .map_err(|error| format!("Cannot load OCR model: {error}"))
    }

    pub(super) fn recognize(&self, image: RgbImage) -> Result<String, String> {
        let results = self
            .0
            .predict(vec![image])
            .map_err(|error| error.to_string())?;
        Ok(results
            .into_iter()
            .flat_map(|result| result.text_regions)
            .filter_map(|region| region.text)
            .filter(|text| !text.trim().is_empty())
            .map(|text| text.trim().to_owned())
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

#[derive(Deserialize)]
struct RecognitionConfig {
    #[serde(rename = "PostProcess")]
    post_process: CharacterDictionary,
}

#[derive(Deserialize)]
struct CharacterDictionary {
    character_dict: Vec<String>,
}

fn initialize_runtime(path: &Path) -> Result<(), String> {
    if !path.is_file() {
        return Err("OCR runtime is missing. Complete the model setup first.".into());
    }
    static INITIALIZED: Mutex<Option<PathBuf>> = Mutex::new(None);
    let path = path.canonicalize().map_err(|error| error.to_string())?;
    let mut selected = INITIALIZED
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if let Some(selected) = selected.as_ref() {
        return if selected == &path {
            Ok(())
        } else {
            Err("Restart the application after changing the OCR runtime directory.".into())
        };
    }
    let builder = ort::init_from(&path).map_err(|error| error.to_string())?;
    if !builder.commit() {
        return Err("OCR runtime was initialized before its managed library was selected.".into());
    }
    // Only cache success, so repairing a missing/incompatible installation can
    // be retried. The packaged CPU library stays loaded for the process lifetime.
    *selected = Some(path);
    Ok(())
}
