//! Provider-owned MeloTTS text frontends.

mod chinese;
mod english;

use std::path::Path;

use half::f16;
use ort::session::Session;

use crate::InferenceError;

use chinese::ChineseFrontend;
use english::EnglishFrontend;

pub(super) struct MeloInputs {
    pub(super) phone_ids: Vec<i32>,
    pub(super) tones: Vec<i32>,
    pub(super) language_ids: Vec<i32>,
    pub(super) bert: Vec<f16>,
}

pub(super) enum MeloFrontend {
    English(EnglishFrontend),
    Chinese(ChineseFrontend),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MeloFrontendKind {
    English,
    ChineseMixedEnglish,
}

impl MeloFrontendKind {
    pub(super) const fn required_files(self) -> &'static [&'static str] {
        match self {
            Self::English => &["frontend/cmudict.json", "frontend/bert_vocab.txt"],
            Self::ChineseMixedEnglish => &[
                "frontend/cmudict.json",
                "frontend/bert_vocab.txt",
                "frontend/chinese_lexicon.json",
                "frontend/opencpop-strict.txt",
            ],
        }
    }
}

impl MeloFrontend {
    pub(super) fn load(
        model_dir: &Path,
        symbols: &[String],
        kind: MeloFrontendKind,
    ) -> Result<Self, InferenceError> {
        match kind {
            MeloFrontendKind::English => {
                EnglishFrontend::load(model_dir, symbols).map(Self::English)
            }
            MeloFrontendKind::ChineseMixedEnglish => {
                ChineseFrontend::load(model_dir, symbols).map(Self::Chinese)
            }
        }
    }

    pub(super) fn encode(
        &self,
        bert_session: &mut Session,
        text: &str,
    ) -> Result<MeloInputs, InferenceError> {
        match self {
            Self::English(frontend) => frontend.encode(bert_session, text),
            Self::Chinese(frontend) => frontend.encode(bert_session, text),
        }
    }
}

pub(super) fn distribute(items: usize, slots: usize) -> Vec<usize> {
    if slots == 0 {
        return Vec::new();
    }
    let mut result = vec![items / slots; slots];
    for value in result.iter_mut().take(items % slots) {
        *value += 1;
    }
    result
}

pub(super) fn intersperse(values: Vec<i32>, blank: i32) -> Vec<i32> {
    let mut result = Vec::with_capacity(values.len() * 2 + 1);
    result.push(blank);
    for value in values {
        result.push(value);
        result.push(blank);
    }
    result
}

pub(super) fn transpose_phone_features(features: &[f16]) -> Vec<f16> {
    let phones = features.len() / 768;
    let mut transposed = Vec::with_capacity(features.len());
    for channel in 0..768 {
        for phone in 0..phones {
            transposed.push(features[phone * 768 + channel]);
        }
    }
    transposed
}
