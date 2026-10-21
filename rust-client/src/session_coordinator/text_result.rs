//! Collect one finite text request through the normalized result contract.
use super::{TranslationEvent, TranslationOutcome, TranslationSegment};
use std::collections::BTreeMap;

#[derive(Default)]
pub(crate) struct TextTranslationCollector {
    segments: BTreeMap<(String, u32), TranslationSegment>,
}

pub(crate) struct TextTranslationResult {
    pub text: String,
    pub source_language: String,
    pub target_language: String,
}

impl TextTranslationCollector {
    pub fn accept(
        &mut self,
        event: &TranslationEvent,
    ) -> Option<Result<TextTranslationResult, String>> {
        match event {
            TranslationEvent::Segment(segment) if segment.translated.is_some() => {
                self.segments.insert(
                    (segment.turn_id.clone(), segment.segment_index),
                    segment.clone(),
                );
            }
            TranslationEvent::ReplaceSegments(segments) => {
                if let Some(first) = segments
                    .first()
                    .filter(|segment| segment.translated.is_some())
                {
                    self.segments.retain(|(turn, _), _| turn != &first.turn_id);
                    for segment in segments
                        .iter()
                        .filter(|segment| segment.translated.is_some())
                    {
                        self.segments.insert(
                            (segment.turn_id.clone(), segment.segment_index),
                            segment.clone(),
                        );
                    }
                }
            }
            TranslationEvent::Finished { outcome, .. } => {
                return Some(match outcome {
                    TranslationOutcome::Completed => {
                        let mut text = String::new();
                        for segment in self.segments.values() {
                            crate::streaming::append_segment(
                                &mut text,
                                segment.translated.as_deref().unwrap_or_default(),
                            );
                        }
                        if text.trim().is_empty() {
                            Err("No translation was returned. Retry this task.".into())
                        } else {
                            let first = self.segments.values().next().unwrap();
                            let (source_language, target_language) =
                                if self.segments.values().all(|segment| {
                                    segment.source_lang == first.source_lang
                                        && segment.target_lang == first.target_lang
                                }) {
                                    (first.source_lang.clone(), first.target_lang.clone())
                                } else {
                                    (String::new(), String::new())
                                };
                            Ok(TextTranslationResult {
                                text,
                                source_language,
                                target_language,
                            })
                        }
                    }
                    TranslationOutcome::Failed(error) => Err(error.clone()),
                    TranslationOutcome::Cancelled => Err("Cancelled".into()),
                });
            }
            _ => {}
        }
        None
    }
}
