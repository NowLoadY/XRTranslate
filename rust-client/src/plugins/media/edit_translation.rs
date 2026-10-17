//! A retranslation updates the edit draft; only the existing Save action commits it.
use super::{
    MediaPlugin,
    controller::SubtitleEdit,
    subtitle_job::{SubtitleTranslation, SubtitleTranslationResult},
};
use crate::session_coordinator::{
    TranslationEvent, TranslationOutcome, TranslationSessionOwner, TranslationTask,
};
use xrtranslate_engine::language::LanguageCapabilities;

pub(super) struct SubtitleRetranslation {
    task_id: String,
    original: String,
    translation: SubtitleTranslation,
    owner: TranslationSessionOwner,
}

impl MediaPlugin {
    pub(crate) fn retranslate_subtitle(
        &mut self,
        languages: LanguageCapabilities,
    ) -> Result<TranslationTask, String> {
        let draft = self
            .controller
            .subtitle_edit
            .as_ref()
            .ok_or("No subtitle is being edited")?;
        let task = self
            .controller
            .store
            .get(&draft.task_id)
            .filter(|task| {
                !task.is_task_running
                    && self.controller.active_task_id.as_deref() == Some(task.id.as_str())
            })
            .ok_or("Pause translation to edit.")?;
        if draft.retranslation.is_some() {
            return Err("This subtitle is already being translated".into());
        }
        if draft.cue.original_text.trim().is_empty() {
            return Err("Subtitle text cannot be empty".into());
        }
        let index = self
            .controller
            .subtitles
            .cues()
            .iter()
            .position(|cue| cue.id == draft.cue.id)
            .ok_or("Subtitle not found")?;
        let selection = languages
            .for_text()
            .select(&task.source_language, &task.target_language)?;
        let translation = SubtitleTranslation::new(&draft.cue.id);
        let request = translation.task(
            self.controller.subtitles.cues(),
            index,
            draft.cue.original_text.trim().to_owned(),
            selection,
        );
        let pending = SubtitleRetranslation {
            task_id: draft.task_id.clone(),
            original: draft.cue.original_text.clone(),
            owner: request.owner(),
            translation,
        };
        self.controller
            .subtitle_edit
            .as_mut()
            .unwrap()
            .retranslation = Some(pending.translation.operation.clone());
        self.subtitle_retranslation = Some(pending);
        Ok(request)
    }

    pub(crate) fn take_cancelled_retranslation(&mut self) -> Option<TranslationSessionOwner> {
        let pending = self.subtitle_retranslation.as_ref()?;
        if self.controller.subtitle_edit.as_ref().is_some_and(|draft| {
            self.controller.active_task_id.as_deref() == Some(pending.task_id.as_str())
                && draft.retranslation.as_deref() == Some(pending.translation.operation.as_str())
        }) {
            return None;
        }
        self.subtitle_retranslation
            .take()
            .map(|pending| pending.owner)
    }

    pub(super) fn accept_retranslation_event(
        &mut self,
        operation: &str,
        event: &TranslationEvent,
    ) -> bool {
        let Some(pending) = &mut self.subtitle_retranslation else {
            return false;
        };
        if pending.translation.operation != operation {
            return false;
        }
        if let Some(result) = pending.translation.accept(event) {
            let pending = self.subtitle_retranslation.take().unwrap();
            let error = pending.apply(
                self.controller.active_task_id.as_deref(),
                self.controller.subtitle_edit.as_mut(),
                result,
            );
            if let Some(error) = error
                && !matches!(
                    event,
                    TranslationEvent::Finished {
                        outcome: TranslationOutcome::Cancelled,
                        ..
                    }
                )
            {
                self.controller.error = Some(error);
            }
        }
        true
    }
}

impl SubtitleRetranslation {
    fn apply(
        &self,
        active_task: Option<&str>,
        draft: Option<&mut SubtitleEdit>,
        result: Result<SubtitleTranslationResult, String>,
    ) -> Option<String> {
        let draft = draft.filter(|draft| {
            active_task == Some(self.task_id.as_str())
                && draft.task_id == self.task_id
                && draft.cue.id == self.translation.cue
                && draft.retranslation.as_deref() == Some(self.translation.operation.as_str())
        })?;
        draft.retranslation = None;
        if draft.cue.original_text != self.original {
            return None;
        }
        match result {
            Ok(result) => {
                draft.cue.translated_text = Some(result.text);
                draft.cue.source_language = result.source_language;
                draft.cue.target_language = result.target_language;
                None
            }
            Err(error) => Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::media::subtitles::SubtitleTimeline;

    fn edit() -> (SubtitleTimeline, SubtitleEdit, SubtitleRetranslation) {
        let mut timeline = SubtitleTimeline::parse(
            "1\n00:00:00,000 --> 00:00:01,000\nThe entrance is on the right.",
        )
        .unwrap();
        timeline.set_translation("srt_0", "入口在右边。".into(), "en", "zh");
        let mut cue = timeline.cues()[0].clone();
        cue.original_text = "The entrance is on the left.".into();
        let translation = SubtitleTranslation::new(&cue.id);
        let draft = SubtitleEdit {
            task_id: "task".into(),
            cue,
            focus: None,
            retranslate_requested: false,
            retranslation: Some(translation.operation.clone()),
        };
        let pending = SubtitleRetranslation {
            task_id: draft.task_id.clone(),
            original: draft.cue.original_text.clone(),
            translation,
            owner: TranslationSessionOwner::Host {
                capture_source: crate::client_settings::CaptureSource::Microphone,
            },
        };
        (timeline, draft, pending)
    }

    fn result() -> Result<SubtitleTranslationResult, String> {
        Ok(SubtitleTranslationResult {
            text: "入口在左边。".into(),
            source_language: "en".into(),
            target_language: "zh".into(),
        })
    }

    #[test]
    fn retranslation_only_updates_the_matching_draft() {
        let (timeline, mut draft, pending) = edit();
        assert!(
            pending
                .apply(Some("task"), Some(&mut draft), result())
                .is_none()
        );
        assert_eq!(draft.cue.translated_text.as_deref(), Some("入口在左边。"));
        assert!(draft.retranslation.is_none());
        assert_eq!(
            timeline.cues()[0].translated_text.as_deref(),
            Some("入口在右边。")
        );
        assert_eq!(draft.cue.start_ms, timeline.cues()[0].start_ms);
    }

    #[test]
    fn cancelled_changed_or_replaced_edits_ignore_late_results() {
        let (_, mut draft, pending) = edit();
        assert!(pending.apply(Some("task"), None, result()).is_none());
        pending.apply(Some("other-task"), Some(&mut draft), result());
        assert_eq!(draft.cue.translated_text.as_deref(), Some("入口在右边。"));
        draft.retranslation = Some("new-operation".into());
        pending.apply(Some("task"), Some(&mut draft), result());
        assert_eq!(draft.retranslation.as_deref(), Some("new-operation"));
        draft.retranslation = Some(pending.translation.operation.clone());
        draft.cue.original_text = "A different sentence".into();
        pending.apply(Some("task"), Some(&mut draft), result());
        assert_eq!(draft.cue.translated_text.as_deref(), Some("入口在右边。"));
        assert!(draft.retranslation.is_none());
    }

    #[test]
    fn failure_preserves_edit_and_allows_retry() {
        let (_, mut draft, pending) = edit();
        assert_eq!(
            pending.apply(Some("task"), Some(&mut draft), Err("offline".into())),
            Some("offline".into())
        );
        assert_eq!(draft.cue.translated_text.as_deref(), Some("入口在右边。"));
        assert_eq!(draft.cue.original_text, "The entrance is on the left.");
        assert!(draft.retranslation.is_none());
    }
}
