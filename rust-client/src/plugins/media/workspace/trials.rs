//! Finite experiments reuse imported-audio sessions; candidates never write live subtitles.
use super::super::{
    MediaPlugin,
    subtitles::{SubtitleCue, SubtitleTimeline},
};
use crate::{
    client_settings::RecognitionSettings,
    session_coordinator::{
        PluginSessionBinding, PluginSessionOwner, SessionOutputPolicy, TranslationEvent,
        TranslationOutcome, TranslationSessionOwner, TranslationTask,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use ui_director::{Tool, ToolCall, ToolResult, arguments};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Trial {
    pub(super) id: String,
    pub(super) status: String,
    start_ms: i64,
    end_ms: i64,
    source: Vec<SubtitleCue>,
    pub(super) recognition: RecognitionSettings,
    #[serde(default)]
    pub(super) audio_track: Option<super::super::task::AudioTrackSelection>,
    pub(super) timeline: SubtitleTimeline,
    error: Option<String>,
}

impl Trial {
    pub(super) fn recover(&mut self) {
        if self.status == "running" {
            self.status = "failed".into();
            self.error = Some("Interrupted by application restart".into());
        }
    }
}

pub(crate) struct PendingTrial {
    workspace: String,
    id: String,
    request: Option<TranslationTask>,
    started: std::time::Instant,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Start {
    workspace: String,
    revision: u64,
    first_cue: Option<String>,
    last_cue: Option<String>,
    start_ms: Option<i64>,
    end_ms: Option<i64>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Operation {
    workspace: String,
    operation: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Apply {
    workspace: String,
    revision: u64,
    operation: String,
}

pub(super) fn tools() -> Vec<Tool> {
    let operation = json!({"workspace":{"type":"string"},"operation":{"type":"string"}});
    let mut trial = Tool::new(
        "media.trial",
        "Re-recognize and translate up to 60 seconds to investigate recognition problems; use media.edit for translation-only corrections. Prefer first_cue/last_cue for replacement. start_ms/end_ms also permits gaps, but a range cutting through an existing cue is inspection-only and cannot be adopted. Uses the selected track and current recognition options; media.settings does not change the track. Current subtitles stay untouched. Read the candidate with media.read before judging or adopting it.",
        json!({"workspace":{"type":"string"},"revision":{"type":"integer"},"first_cue":{"type":"string"},"last_cue":{"type":"string"},"start_ms":{"type":"integer","minimum":0},"end_ms":{"type":"integer","minimum":1}}),
        &["workspace", "revision"],
    );
    trial.input_schema["oneOf"] = json!([
        {"required":["first_cue","last_cue"]},
        {"required":["start_ms","end_ms"]}
    ]);
    vec![
        trial,
        Tool::new(
            "media.operation",
            "Read a trial's status, parameters and document ID. Running operations include a wait receipt. Inspect completed text with media.read, using the document ID and time range to compare with draft or base.",
            operation.clone(),
            &["workspace", "operation"],
        ).read_only(),
        Tool::new(
            "media.cancel",
            "Cancel only the specified trial; retain subtitles and other tasks.",
            operation,
            &["workspace", "operation"],
        ),
        Tool::new(
            "media.apply_trial",
            "Adopt a completed trial over its time interval, including inserting recovered subtitles into a gap. Existing cues must be unchanged and fully covered by the interval. Inspect the entire candidate before adopting; source and audio settings remain in the record.",
            json!({"workspace":{"type":"string"},"revision":{"type":"integer"},"operation":{"type":"string"}}),
            &["workspace", "revision", "operation"],
        ),
    ]
}

fn pending(workspace: &str, id: &str) -> Value {
    json!({"status":"running","operation":id,
        "pending":{"name":"media.operation","arguments":{"workspace":workspace,"operation":id}},
        "cancel":{"name":"media.cancel","arguments":{"workspace":workspace,"operation":id}}})
}

fn cue_interval(timeline: &SubtitleTimeline, start_ms: i64, end_ms: i64) -> std::ops::Range<usize> {
    let end = timeline.cues().partition_point(|cue| cue.start_ms < end_ms);
    let start = (0..end)
        .find(|&index| timeline.effective_end_at(index) > start_ms)
        .unwrap_or(end);
    start..end
}

impl MediaPlugin {
    pub(crate) fn trial_call(&mut self, call: &ToolCall) -> ToolResult {
        match call.name.as_str() {
            "media.trial" => {
                if self.workspace_trial.is_some() {
                    return Err("An audio trial is already running".into());
                }
                let args: Start = arguments(call)?;
                let task = self
                    .workspace_task(&args.workspace)
                    .ok_or("Workspace not found")?;
                if task.is_task_running {
                    return Err("Wait for the media task to finish".into());
                }
                let workspace = task.optimization.as_ref().unwrap();
                workspace.check_revision(args.revision)?;
                let cues = workspace.draft.cues();
                let (start_ms, end_ms) = match (
                    args.first_cue.as_deref(),
                    args.last_cue.as_deref(),
                    args.start_ms,
                    args.end_ms,
                ) {
                    (Some(first), Some(last), None, None) => {
                        let first = cues
                            .iter()
                            .position(|cue| cue.id == first)
                            .ok_or("First cue not found")?;
                        let last = cues
                            .iter()
                            .position(|cue| cue.id == last)
                            .ok_or("Last cue not found")?;
                        if last < first {
                            return Err("Invalid cue interval".into());
                        }
                        (cues[first].start_ms, workspace.draft.effective_end_at(last))
                    }
                    (None, None, Some(start), Some(end)) => (start, end),
                    _ => return Err("Supply either first_cue/last_cue or start_ms/end_ms".into()),
                };
                if start_ms < 0 || end_ms <= start_ms || end_ms - start_ms > 60_000 {
                    return Err("Trial interval must be at most 60 seconds".into());
                }
                let range = cue_interval(&workspace.draft, start_ms, end_ms);
                if workspace.trials.len() >= 8 {
                    return Err("This workspace has reached its eight-trial budget".into());
                }
                let mut recognition = task.recognition.clone();
                recognition.continuous_recognition = false;
                let preprocessing = recognition
                    .preprocessing
                    .unwrap_or(self.default_preprocessing);
                if !self.available_preprocessing.contains(&preprocessing) {
                    return Err(
                        "Audio processing model is not installed. Select an available option."
                            .into(),
                    );
                }
                recognition.preprocessing = Some(preprocessing);
                if task.media_type == super::super::task::MediaType::Subtitles {
                    return Err("This task has no finite audio source".into());
                }
                let input = task.translation_request().import_input(
                    self.default_preprocessing,
                    Some((start_ms as u64, end_ms as u64)),
                )?;
                let languages = xrtranslate_engine::language::LanguageSelection::parse(
                    &task.source_language,
                    &task.target_language,
                )?;
                let id = uuid::Uuid::new_v4().to_string();
                let request = TranslationTask {
                    input,
                    languages,
                    plugin: Some(PluginSessionBinding::text(
                        trial_owner(&id),
                        SessionOutputPolicy::PluginOnly,
                    )),
                    context: cues
                        .get(range.start)
                        .map(|_| super::super::subtitle_job::subtitle_context(cues, range.start))
                        .unwrap_or_default(),
                    text_workload: None,
                    profiles: Vec::new(),
                };
                let trial = Trial {
                    id: id.clone(),
                    status: "running".into(),
                    start_ms,
                    end_ms,
                    source: cues[range].to_vec(),
                    recognition,
                    audio_track: Some(task.audio_track),
                    timeline: SubtitleTimeline::new(),
                    error: None,
                };
                super::feedback::status(&task.id, "", ui_director::feedback::Effect::Pending);
                self.workspace_mut(&args.workspace)?.trials.push(trial);
                self.workspace_trial = Some(PendingTrial {
                    workspace: args.workspace.clone(),
                    id: id.clone(),
                    request: Some(request),
                    started: std::time::Instant::now(),
                });
                self.controller.dirty = true;
                Ok(pending(&args.workspace, &id))
            }
            "media.operation" | "media.cancel" => {
                let args: Operation = arguments(call)?;
                if call.name == "media.cancel"
                    && self.workspace_trial.as_ref().is_some_and(|running| {
                        running.id == args.operation && running.workspace == args.workspace
                    })
                {
                    self.cancel_workspace_trial("Cancelled");
                }
                let workspace = self.workspace_mut(&args.workspace)?;
                let trial = workspace
                    .trials
                    .iter()
                    .find(|trial| trial.id == args.operation)
                    .ok_or("Operation not found")?;
                if trial.status == "running" {
                    Ok(pending(&args.workspace, &args.operation))
                } else {
                    Ok(
                        json!({"id":trial.id,"status":trial.status,"error":trial.error,
                        "document":trial.id,"start_ms":trial.start_ms,"end_ms":trial.end_ms,
                        "cue_count":trial.timeline.count(),"source_cue_count":trial.source.len(),
                        "recognition":trial.recognition,"audio_track":trial.audio_track}),
                    )
                }
            }
            "media.apply_trial" => {
                if self.workspace_trial.is_some() {
                    return Err("Wait for the current audio trial".into());
                }
                let args: Apply = arguments(call)?;
                let task = self
                    .workspace_task(&args.workspace)
                    .ok_or("Workspace not found")?;
                let task_id = task.id.clone();
                if task.is_task_running {
                    return Err("Wait for the media task to finish".into());
                }
                let workspace = self.workspace_mut(&args.workspace)?;
                workspace.check_revision(args.revision)?;
                let trial = workspace
                    .trials
                    .iter()
                    .find(|trial| trial.id == args.operation)
                    .ok_or("Operation not found")?;
                if trial.status != "completed" || trial.timeline.count() == 0 {
                    return Err("Trial has no completed subtitles".into());
                }
                let range = cue_interval(&workspace.draft, trial.start_ms, trial.end_ms);
                if workspace.draft.cues()[range.clone()] != trial.source {
                    return Err("Source cues have changed; run a new trial".into());
                }
                if !range.is_empty()
                    && (workspace.draft.cues()[range.start].start_ms < trial.start_ms
                        || workspace.draft.effective_end_at(range.end - 1) > trial.end_ms)
                {
                    return Err("Trial cuts through an existing cue; expand the interval or edit only the supported text correction".into());
                }
                let before =
                    super::feedback::items(&task_id, &workspace.draft.cues()[range.clone()]);
                let after = super::feedback::items(&task_id, trial.timeline.cues());
                workspace.draft.replace_range_from(range, &trial.timeline);
                workspace.revision += 1;
                let result = json!({"revision":workspace.revision,"adopted":args.operation});
                let recognition = trial.recognition.clone();
                workspace.record(&call.name, result.clone());
                self.controller
                    .set_task_recognition(&task_id, recognition)?;
                super::feedback::changed(&task_id, before, after);
                self.controller.dirty = true;
                Ok(result)
            }
            _ => Err("Unknown trial operation".into()),
        }
    }

    pub(crate) fn next_workspace_task(&mut self) -> Option<TranslationTask> {
        if self
            .workspace_trial
            .as_ref()
            .is_some_and(|pending| pending.started.elapsed() > std::time::Duration::from_secs(300))
        {
            self.cancel_workspace_trial("Audio trial timed out");
        }
        self.workspace_trial.as_mut()?.request.take()
    }

    pub(crate) fn cancel_workspace_trial(&mut self, reason: &str) {
        if let Some(pending) = self.workspace_trial.take() {
            self.workspace_cancelled
                .push(TranslationSessionOwner::Plugin(trial_owner(&pending.id)));
            if let Ok(workspace) = self.workspace_mut(&pending.workspace) {
                if let Some(trial) = workspace
                    .trials
                    .iter_mut()
                    .find(|trial| trial.id == pending.id)
                {
                    trial.status = "cancelled".into();
                    trial.error = Some(reason.into());
                }
                workspace.record(
                    "media.cancel",
                    json!({"operation":pending.id,"reason":reason}),
                );
            }
            self.controller.dirty = true;
        }
    }

    pub(crate) fn is_workspace_operation(&self, operation: &str) -> bool {
        self.workspace_trial
            .as_ref()
            .is_some_and(|trial| trial.id == operation)
    }

    pub(crate) fn accept_workspace_event(
        &mut self,
        operation: &str,
        event: &TranslationEvent,
    ) -> bool {
        let Some(pending) = self
            .workspace_trial
            .as_ref()
            .filter(|pending| pending.id == operation)
        else {
            return false;
        };
        let id = pending.workspace.clone();
        let Ok(workspace) = self.workspace_mut(&id) else {
            self.workspace_trial = None;
            return true;
        };
        let Some(trial) = workspace
            .trials
            .iter_mut()
            .find(|trial| trial.id == operation)
        else {
            return true;
        };
        match event {
            TranslationEvent::Segment(segment) if !segment.revisable && !segment.live => {
                if let Some((cue, metadata)) =
                    super::super::events::translation_cue(segment.clone(), &trial.timeline)
                {
                    trial.timeline.add_cue_with_metadata(cue, metadata);
                }
            }
            TranslationEvent::ReplaceSegments(segments)
                if !segments
                    .iter()
                    .any(|segment| segment.revisable || segment.live) =>
            {
                super::super::events::apply_recognition_snapshot(
                    &mut trial.timeline,
                    segments.clone(),
                )
            }
            TranslationEvent::Finished { outcome, .. } => {
                match outcome {
                    TranslationOutcome::Completed => {
                        let mut cues = trial.timeline.cues().to_vec();
                        for cue in &mut cues {
                            cue.start_ms =
                                (cue.start_ms + trial.start_ms).clamp(trial.start_ms, trial.end_ms);
                            cue.end_ms =
                                (cue.end_ms + trial.start_ms).clamp(cue.start_ms, trial.end_ms);
                        }
                        trial
                            .timeline
                            .replace_refined(0..trial.timeline.count(), cues);
                        trial.status = "completed".into();
                    }
                    TranslationOutcome::Cancelled => {
                        trial.status = "cancelled".into();
                        trial.error = Some("Cancelled".into());
                    }
                    TranslationOutcome::Failed(error) => {
                        trial.status = "failed".into();
                        trial.error = Some(error.clone());
                    }
                }
                log::info!(
                    "Media audio trial finished: operation={} range_ms={}..{} audio_track={:?} preprocessing={:?} source_cues={} result_cues={} status={} error={:?}",
                    trial.id,
                    trial.start_ms,
                    trial.end_ms,
                    trial.audio_track,
                    trial.recognition.preprocessing,
                    trial.source.len(),
                    trial.timeline.count(),
                    trial.status,
                    trial.error
                );
                let result = json!({"operation":trial.id,"status":trial.status,"error":trial.error,
                    "start_ms":trial.start_ms,"end_ms":trial.end_ms,"audio_track":trial.audio_track,
                    "recognition":trial.recognition,"source_cue_count":trial.source.len(),
                    "cue_count":trial.timeline.count()});
                workspace.record("media.trial", result);
                self.workspace_trial = None;
            }
            _ => {}
        }
        self.controller.dirty = true;
        true
    }
}

fn trial_owner(id: &str) -> PluginSessionOwner {
    PluginSessionOwner::new(
        super::super::super::PluginId::MEDIA.as_str(),
        id,
        "Media",
        "Media",
        "Audio trial",
    )
}
