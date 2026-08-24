//! Recognition and translation history models plus deterministic stream merging.

mod merge;
mod model;

pub(crate) use merge::{
    collect_authoritative_recognition_snapshot, collect_authoritative_translation_snapshot,
    collect_recognition_window, merge_authoritative_recognition_snapshot,
    merge_authoritative_translation_snapshot, merge_stream_recognition, merge_stream_translation,
    upsert_completed_translation,
};
pub(crate) use model::{
    PendingAuthoritativeRecognition, PendingAuthoritativeTranslation, PendingFinalAsr,
    PendingRecognitionWindow, RecognitionHistoryEntry, TranslationHistoryEntry,
};
