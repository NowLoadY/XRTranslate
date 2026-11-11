//! A versioned working copy. The caller chooses actions; media owns validation and commit.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{MediaPlugin, subtitles::SubtitleTimeline};
use ui_director::{Tool, ToolCall, ToolProvider, ToolResult, arguments};
mod feedback;
mod inspection;
mod reading;
mod trials;
use inspection::issues;
pub(super) use trials::PendingTrial;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Workspace {
    pub id: String,
    pub revision: u64,
    pub status: String,
    #[serde(default)]
    pub context: Value,
    pub base: SubtitleTimeline,
    pub draft: SubtitleTimeline,
    pub committed: Option<SubtitleTimeline>,
    pub records: Vec<Value>,
    #[serde(default)]
    pub trials: Vec<trials::Trial>,
}

impl Workspace {
    pub(super) fn trials_remaining(&self) -> usize {
        trials::MAX_TRIALS.saturating_sub(self.trials.len())
    }

    pub(super) fn recover(&mut self) {
        for trial in &mut self.trials {
            trial.recover();
        }
    }
    pub fn record(&mut self, action: &str, result: Value) {
        if self.records.len() == 64 {
            self.records.remove(0);
        }
        self.records
            .push(json!({"action": action, "result": result}));
    }

    pub fn check_revision(&self, revision: u64) -> Result<(), String> {
        if self.revision != revision {
            return Err(format!(
                "Workspace changed: expected revision {revision}, current {}. Read it again.",
                self.revision
            ));
        }
        if self.status != "open" {
            return Err(format!("Workspace is {}", self.status));
        }
        Ok(())
    }

    fn check_base(
        &self,
        task: &super::task::MediaTask,
        current: &SubtitleTimeline,
    ) -> Result<(), String> {
        let context = task_context(task);
        let changed = context
            .as_object()
            .unwrap()
            .iter()
            .filter(|(key, value)| self.context.get(*key) != Some(*value))
            .map(|(key, _)| key.as_str())
            .collect::<Vec<_>>();
        let reason = if !changed.is_empty() {
            format!("task inputs changed ({})", changed.join(", "))
        } else if !same_timeline(current, &self.base) {
            "subtitles changed since this workspace was opened".into()
        } else {
            return Ok(());
        };
        Err(format!(
            "Conflict: {reason}. Draft {} is retained; compare draft/current with media.read before reopening with reset=true.",
            self.id
        ))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Open {
    task: String,
    #[serde(default)]
    reset: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Check {
    workspace: String,
    #[serde(default)]
    offset: usize,
    #[serde(default = "page_size")]
    limit: usize,
    kind: Option<String>,
}
fn page_size() -> usize {
    12
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Scope {
    workspace: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Revision {
    workspace: String,
    revision: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Edit {
    workspace: String,
    revision: u64,
    edits: Vec<CueEdit>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CueEdit {
    cue: String,
    original: Option<String>,
    translation: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Delete {
    workspace: String,
    revision: u64,
    cue: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Merge {
    workspace: String,
    revision: u64,
    source_cue: String,
    target_cue: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Split {
    workspace: String,
    revision: u64,
    cue: Option<String>,
    parts: Option<Vec<SplitPart>>,
    splits: Option<Vec<CueSplit>>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CueSplit {
    cue: String,
    parts: Vec<SplitPart>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SplitPart {
    original: String,
    translation: String,
}

pub(crate) fn tools() -> Vec<Tool> {
    let scope = json!({"workspace":{"type":"string"}});
    let revision = json!({"workspace":{"type":"string"},"revision":{"type":"integer","minimum":0}});
    let parts = json!({"type":"array","minItems":2,"maxItems":16,"items":{"type":"object","properties":{"original":{"type":"string","minLength":1,"maxLength":8000},"translation":{"type":"string","minLength":1,"maxLength":8000}},"required":["original","translation"],"additionalProperties":false}});
    let mut split = Tool::new(
        "media.split",
        "Atomically split one cue (cue/parts), or batch 1 to 8 distinct cues (splits) from a reviewed passage in one call. Supply each part's original text and nonempty translation in the task's target language. Originals must be exact consecutive source substrings, preserving every word and punctuation; use media.edit before or after splitting to correct recognition mistakes. Repartition a correct existing translation by meaning. Exact source preservation does not require literal repetition or unnatural phrasing in the translation. Local code assigns proportional timestamps, at least 1 second per part. Any invalid item leaves the entire draft unchanged. Use either cue/parts or splits, never both.",
        json!({"workspace":{"type":"string"},"revision":{"type":"integer"},"cue":{"type":"string"},"parts":parts,"splits":{"type":"array","minItems":1,"maxItems":8,"items":{"type":"object","properties":{"cue":{"type":"string"},"parts":parts},"required":["cue","parts"],"additionalProperties":false}}}),
        &["workspace", "revision"],
    );
    split.input_schema["oneOf"] = json!([
        {"required":["cue","parts"],"not":{"required":["splits"]}},
        {"required":["splits"],"not":{"anyOf":[{"required":["cue"]},{"required":["parts"]}]}}
    ]);
    let mut tools = vec![
        Tool::new(
            "media.tasks",
            "List media projects (tasks) with their IDs, titles, subtitle counts and running state.",
            json!({}),
            &[],
        ).read_only(),
        Tool::new(
            "media.open",
            "Open or resume a subtitle working copy for an idle task. Rejects stale drafts before editing and returns their ID for inspection with media.read. Does not navigate: use UI tools to open the corresponding media project when the user should see the work. reset=true discards a previous working copy; use it only to restart after a conflict.",
            json!({"task":{"type":"string"},"reset":{"type":"boolean"}}),
            &["task"],
        ),
        reading::tool(),
        Tool::new(
            "media.check",
            "Return counts and cue locations for excessive length, empty translations or invalid timing; this does not detect mistranslations or omitted phrases. hints flag short cues or repeated adjacent boundaries for optional review, not mandatory edits. Filter by kind; next_offset pages findings, not cues. Read the relevant ranges yourself.",
            json!({"workspace":{"type":"string"},"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":50},"kind":{"type":"string","enum":inspection::KINDS}}),
            &["workspace"],
        ).read_only(),
        Tool::new(
            "media.edit",
            "Atomically edit original (ASR) text and/or translation in the working copy using the subtitle editor. Omitted fields stay unchanged; an empty translation clears it. Original text must be nonempty. Check the translation whenever you correct source text.",
            json!({"workspace":{"type":"string"},"revision":{"type":"integer"},"edits":{"type":"array","minItems":1,"maxItems":50,"items":{"type":"object","properties":{"cue":{"type":"string"},"original":{"type":"string","minLength":1,"maxLength":8000},"translation":{"type":"string","maxLength":8000}},"required":["cue"],"anyOf":[{"required":["original"]},{"required":["translation"]}],"additionalProperties":false}}}),
            &["workspace", "revision", "edits"],
        ),
        Tool::new(
            "media.delete",
            "Delete one unwanted cue by ID from the working copy. Commit publishes the change; undo can restore the previous subtitles. Inspect the cue and its context before deleting it.",
            json!({"workspace":{"type":"string"},"revision":{"type":"integer","minimum":0},"cue":{"type":"string"}}),
            &["workspace", "revision", "cue"],
        ),
        Tool::new(
            "media.merge",
            "Merge two adjacent cues using the subtitle editor's merge operation. Use this to repair a phrase or sentence incorrectly divided across neighbors; if the combined cue is too long, use media.split to repartition it at better boundaries. Text is joined in timeline order, outer timestamps and the target cue's ID are retained. Review and align the joined translation. Changes stay in the working copy until commit.",
            json!({"workspace":{"type":"string"},"revision":{"type":"integer","minimum":0},"source_cue":{"type":"string"},"target_cue":{"type":"string"}}),
            &["workspace", "revision", "source_cue", "target_cue"],
        ),
        split,
        Tool::new(
            "media.commit",
            "Publish the working copy after checking it. Fails if the user changed the task's subtitles since opening. Unresolved issues are retained in the result.",
            revision.clone(),
            &["workspace", "revision"],
        ),
        Tool::new(
            "media.undo",
            "Undo this workspace's last commit only if its published subtitles have not changed.",
            scope.clone(),
            &["workspace"],
        ),
        Tool::new(
            "media.records",
            "Read the workspace's bounded operation and outcome record, including affected cue IDs and time ranges. Use these locations to reread changes after context compaction instead of scanning the whole document.",
            scope,
            &["workspace"],
        ).read_only(),
        Tool::new(
            "media.settings",
            "Read or change the task's recognition options, shared with the visible UI. Audio trials apply these options to the selected audio_track, which may already be a separated stem. Changing preprocessing does not select the original audio. Omit optional fields to read.",
            json!({"workspace":{"type":"string"},"preprocessing":{"type":"string","enum":["none","gtcrn","vocal_separation"]},"background_noise":{"type":"number","minimum":0.02,"maximum":0.95},"pause_tolerance":{"type":"number","minimum":0,"maximum":1}}),
            &["workspace"],
        ),
    ];
    {
        tools.extend(trials::tools());
    }
    tools
}

impl ToolProvider for MediaPlugin {
    fn tools(&self) -> Vec<Tool> {
        tools()
    }
    fn call(&mut self, call: &ToolCall) -> ToolResult {
        let task = call.arguments["workspace"]
            .as_str()
            .and_then(|id| self.workspace_task(id))
            .map(|task| task.id.clone());
        let result = self.workspace_call(call);
        if let (Some(task), Err(error)) = (task, &result) {
            feedback::status(
                &task,
                "",
                ui_director::feedback::Effect::Failed(error.clone()),
            );
        }
        result
    }
}

fn same_timeline(left: &SubtitleTimeline, right: &SubtitleTimeline) -> bool {
    // Includes authored timing and recognition provenance; serialized versions
    // survive application restarts, unlike an in-memory revision counter.
    serde_json::to_value(left).ok() == serde_json::to_value(right).ok()
}

fn cue_location(cue: &super::subtitles::SubtitleCue, end_ms: i64) -> Value {
    json!({"cue":cue.id,"start_ms":cue.start_ms,"end_ms":end_ms})
}

fn task_context(task: &super::task::MediaTask) -> Value {
    // Only subtitle inputs participate; generated audio and playback are independent.
    json!({"source":task.source,"source_language":task.source_language,
        "target_language":task.target_language,"character_limit":task.subtitle_char_limit,
        "audio_track":task.audio_track,"original_audio_track":task.original_audio_track,
        "recognition_channels":task.audio_channels.iter().filter(|channel| channel.recognition).map(|channel| channel.index).collect::<Vec<_>>()})
}

impl MediaPlugin {
    pub(crate) fn prepare_optimization(&mut self, task: &str) -> Result<(Value, String), String> {
        let reset = self.controller.store.get(task).is_some_and(|task| {
            task.optimization.as_ref().is_some_and(|workspace| {
                let current = if self.controller.active_task_id.as_deref() == Some(&task.id) {
                    &self.controller.subtitles
                } else {
                    &task.subtitles
                };
                workspace.status == "open" && workspace.check_base(task, current).is_err()
            })
        });
        if reset {
            log::info!(
                "Refreshing subtitle optimization workspace for task {task}: subtitles or task context changed"
            );
        }
        let opened = self.workspace_call(&ToolCall {
            name: "media.open".into(),
            arguments: json!({"task": task, "reset": reset}),
        })?;
        Ok((
            json!({"workspace": opened["workspace"]}),
            include_str!("optimization.md").trim().to_owned(),
        ))
    }

    pub(crate) fn workspace_task(&self, id: &str) -> Option<&super::task::MediaTask> {
        self.controller.store.tasks.iter().find(|task| {
            task.optimization
                .as_ref()
                .is_some_and(|workspace| workspace.id == id)
        })
    }

    pub(crate) fn workspace_mut(&mut self, id: &str) -> Result<&mut Workspace, String> {
        self.controller
            .store
            .tasks
            .iter_mut()
            .find_map(|task| {
                task.optimization
                    .as_mut()
                    .filter(|workspace| workspace.id == id)
            })
            .ok_or_else(|| "Workspace not found".into())
    }

    pub(crate) fn workspace_call(&mut self, call: &ToolCall) -> ToolResult {
        if matches!(
            call.name.as_str(),
            "media.trial" | "media.operation" | "media.apply_trial" | "media.cancel"
        ) {
            return self.trial_call(call);
        }
        if !matches!(
            call.name.as_str(),
            "media.read" | "media.check" | "media.records" | "media.tasks"
        ) && self.workspace_trial.is_some()
        {
            return Err("Wait for the current audio trial or cancel it".into());
        }
        match call.name.as_str() {
            "media.settings" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Settings {
                    workspace: String,
                    preprocessing: Option<xrtranslate_protocol::AudioPreprocessing>,
                    background_noise: Option<f32>,
                    pause_tolerance: Option<f32>,
                }
                let args: Settings = arguments(call)?;
                let task = self
                    .workspace_task(&args.workspace)
                    .ok_or("Workspace not found")?;
                let id = task.id.clone();
                let audio_track = task.audio_track;
                let previous = task.recognition.clone();
                let mut settings = task.recognition.clone();
                if let Some(choice) = args.preprocessing {
                    if !self.available_preprocessing.contains(&choice) {
                        return Err("Audio processing model is not installed".into());
                    }
                    settings.preprocessing = Some(choice);
                }
                if let Some(value) = args.background_noise {
                    settings.background_noise = value;
                }
                if let Some(value) = args.pause_tolerance {
                    settings.pause_tolerance = value;
                }
                self.controller
                    .set_task_recognition(&id, settings.clone())?;
                self.controller.save_active();
                if self.controller.dirty {
                    return Err(self
                        .controller
                        .error
                        .clone()
                        .unwrap_or("Could not save settings".into()));
                }
                feedback::settings(&id, &previous, &settings);
                Ok(
                    json!({"recognition":settings,"audio_track":audio_track,"available_preprocessing":self.available_preprocessing}),
                )
            }
            "media.tasks" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Empty {}
                let _: Empty = arguments(call)?;
                Ok(json!(self.controller.store.tasks.iter().map(|task| json!({
                    "id":task.id,"title":task.title,"running":task.is_task_running,
                    "subtitle_count":if self.controller.active_task_id.as_deref()==Some(&task.id) { self.controller.subtitles.count() } else { task.subtitles.count() },
                    "workspace":task.optimization.as_ref().map(|workspace| &workspace.id),
                })).collect::<Vec<_>>()))
            }
            "media.open" => {
                let args: Open = arguments(call)?;
                let active = self.controller.active_task_id.as_deref() == Some(&args.task);
                if active && self.controller.subtitle_edit.is_some() {
                    return Err("Finish the current subtitle edit first".into());
                }
                let task = self
                    .controller
                    .store
                    .get_mut(&args.task)
                    .ok_or("Media task not found")?;
                if task.is_task_running {
                    return Err("Wait for recognition and translation to finish".into());
                }
                let timeline = if active {
                    &self.controller.subtitles
                } else {
                    &task.subtitles
                };
                if timeline.count() == 0 {
                    return Err("No subtitles to optimize".into());
                }
                if args.reset
                    || task
                        .optimization
                        .as_ref()
                        .is_none_or(|workspace| workspace.status != "open")
                {
                    task.optimization = Some(Workspace {
                        id: uuid::Uuid::new_v4().to_string(),
                        revision: 0,
                        status: "open".into(),
                        context: task_context(task),
                        base: timeline.clone(),
                        draft: timeline.clone(),
                        committed: None,
                        records: Vec::new(),
                        trials: Vec::new(),
                    });
                } else {
                    task.optimization
                        .as_ref()
                        .unwrap()
                        .check_base(task, timeline)?;
                }
                let workspace = task.optimization.as_ref().unwrap();
                let result = json!({"workspace":workspace.id,"revision":workspace.revision,"task":task.id,
                    "trials_remaining":workspace.trials_remaining()});
                self.controller.dirty = true;
                Ok(result)
            }
            "media.read" => self.read_workspace(arguments(call)?),
            "media.check" => {
                let args: Check = arguments(call)?;
                if !(1..=50).contains(&args.limit) {
                    return Err("limit must be in 1..=50".into());
                }
                if args
                    .kind
                    .as_deref()
                    .is_some_and(|kind| !inspection::KINDS.contains(&kind))
                {
                    return Err("Unknown issue kind; use one listed by media.check".into());
                }
                let task = self
                    .workspace_task(&args.workspace)
                    .ok_or("Workspace not found")?;
                let workspace = task.optimization.as_ref().unwrap();
                Ok(inspection::report(
                    &workspace.draft,
                    task.subtitle_char_limit,
                    workspace.revision,
                    args.offset,
                    args.limit,
                    args.kind.as_deref(),
                ))
            }
            "media.records" => {
                let args: Scope = arguments(call)?;
                Ok(json!(
                    self.workspace_task(&args.workspace)
                        .ok_or("Workspace not found")?
                        .optimization
                        .as_ref()
                        .unwrap()
                        .records
                ))
            }
            "media.edit" => {
                let args: Edit = arguments(call)?;
                let task = self
                    .workspace_task(&args.workspace)
                    .ok_or("Workspace not found")?;
                let target_language = task.target_language.clone();
                let task_id = task.id.clone();
                let workspace = self.workspace_mut(&args.workspace)?;
                workspace.check_revision(args.revision)?;
                if args.edits.is_empty() || args.edits.len() > 50 {
                    return Err("Supply 1 to 50 edits".into());
                }
                let mut draft = workspace.draft.clone();
                let mut affected = Vec::new();
                let mut seen = std::collections::HashSet::new();
                for edit in args.edits {
                    if edit.original.is_none() && edit.translation.is_none() {
                        return Err("Supply original text and/or translation for each edit".into());
                    }
                    if !seen.insert(edit.cue.clone()) {
                        return Err("Duplicate cue in edit".into());
                    }
                    let index = draft
                        .cues()
                        .iter()
                        .position(|cue| cue.id == edit.cue)
                        .ok_or("Cue not found")?;
                    let mut cue = draft.cues()[index].clone();
                    // Match the UI editor's displayed timing; edit_cue retains
                    // the stored timing and provenance when no time was edited.
                    cue.end_ms = draft.effective_end_at(index);
                    if let Some(original) = edit.original {
                        cue.original_text = original;
                    }
                    if let Some(translation) = edit.translation {
                        cue.translated_text = Some(translation);
                        cue.target_language = target_language.clone();
                    }
                    cue.normalize_text();
                    if cue.original_text.is_empty() {
                        return Err(
                            "Original text cannot be empty; use media.delete to remove a cue"
                                .into(),
                        );
                    }
                    if [
                        &cue.original_text,
                        cue.translated_text.as_deref().unwrap_or_default(),
                    ]
                    .into_iter()
                    .any(|text| text.chars().count() > 8_000)
                    {
                        return Err("Each text field must contain at most 8000 characters".into());
                    }
                    affected.push(cue_location(&cue, cue.end_ms));
                    draft.edit_cue(cue).ok_or("Cue not found")?;
                }
                let before = feedback::items(
                    &task_id,
                    workspace
                        .draft
                        .cues()
                        .iter()
                        .filter(|cue| seen.contains(&cue.id)),
                );
                let after = feedback::items(
                    &task_id,
                    draft.cues().iter().filter(|cue| seen.contains(&cue.id)),
                );
                let changed = !same_timeline(&workspace.draft, &draft);
                if changed {
                    workspace.draft = draft;
                    workspace.revision += 1;
                }
                feedback::changed(&task_id, before, after);
                let result =
                    json!({"revision":workspace.revision,"changed":changed,"affected":affected});
                workspace.record(&call.name, result.clone());
                self.controller.dirty = true;
                Ok(result)
            }
            "media.delete" => {
                let args: Delete = arguments(call)?;
                let task_id = self
                    .workspace_task(&args.workspace)
                    .ok_or("Workspace not found")?
                    .id
                    .clone();
                let workspace = self.workspace_mut(&args.workspace)?;
                workspace.check_revision(args.revision)?;
                let index = workspace
                    .draft
                    .cues()
                    .iter()
                    .position(|cue| cue.id == args.cue)
                    .ok_or("Cue not found")?;
                let affected = cue_location(
                    &workspace.draft.cues()[index],
                    workspace.draft.effective_end_at(index),
                );
                let before = feedback::items(&task_id, [&workspace.draft.cues()[index]]);
                workspace.draft.remove_cue(&args.cue);
                feedback::changed(&task_id, before, Vec::new());
                workspace.revision += 1;
                let next_cue = workspace
                    .draft
                    .cues()
                    .get(index.min(workspace.draft.count().saturating_sub(1)))
                    .map(|cue| &cue.id);
                let result = json!({"revision":workspace.revision,"deleted":args.cue,"next_cue":next_cue,"affected":[affected]});
                workspace.record(&call.name, result.clone());
                self.controller.dirty = true;
                Ok(result)
            }
            "media.merge" => {
                let args: Merge = arguments(call)?;
                let task_id = self
                    .workspace_task(&args.workspace)
                    .ok_or("Workspace not found")?
                    .id
                    .clone();
                let workspace = self.workspace_mut(&args.workspace)?;
                workspace.check_revision(args.revision)?;
                let before = feedback::items(
                    &task_id,
                    workspace
                        .draft
                        .cues()
                        .iter()
                        .filter(|cue| cue.id == args.source_cue || cue.id == args.target_cue),
                );
                if !workspace
                    .draft
                    .merge_adjacent(&args.source_cue, &args.target_cue)
                {
                    return Err("Choose two existing adjacent cues to merge".into());
                }
                workspace.revision += 1;
                let index = workspace
                    .draft
                    .cues()
                    .iter()
                    .position(|cue| cue.id == args.target_cue)
                    .unwrap();
                let affected = cue_location(
                    &workspace.draft.cues()[index],
                    workspace.draft.effective_end_at(index),
                );
                feedback::changed(
                    &task_id,
                    before,
                    feedback::items(&task_id, [&workspace.draft.cues()[index]]),
                );
                let result = json!({"revision":workspace.revision,"cue":args.target_cue,"affected":[affected]});
                workspace.record(&call.name, result.clone());
                self.controller.dirty = true;
                Ok(result)
            }
            "media.split" => {
                let args: Split = arguments(call)?;
                let splits = match (args.cue, args.parts, args.splits) {
                    (Some(cue), Some(parts), None) => vec![CueSplit { cue, parts }],
                    (None, None, Some(splits)) if (1..=8).contains(&splits.len()) => splits,
                    _ => return Err("Supply either cue/parts or 1 to 8 splits".into()),
                };
                let task = self
                    .workspace_task(&args.workspace)
                    .ok_or("Workspace not found")?;
                let task_id = task.id.clone();
                let target_language = task.target_language.clone();
                let workspace = self.workspace_mut(&args.workspace)?;
                workspace.check_revision(args.revision)?;
                let mut draft = workspace.draft.clone();
                let mut seen = std::collections::HashSet::new();
                let mut ids = Vec::new();
                let mut affected = Vec::new();
                let mut before = Vec::new();
                let mut after = Vec::new();
                for split in splits {
                    if !seen.insert(split.cue.clone()) {
                        return Err("Each cue may appear only once in a split batch".into());
                    }
                    let index = draft
                        .cues()
                        .iter()
                        .position(|cue| cue.id == split.cue)
                        .ok_or("Cue not found")?;
                    let originals = split
                        .parts
                        .iter()
                        .map(|part| part.original.as_str())
                        .collect::<Vec<_>>();
                    let mut parts = super::subtitle_split::validated_split(
                        &draft.cues()[index],
                        draft.effective_end_at(index),
                        &originals,
                    )
                    .ok_or(concat!(
                        "Invalid split: preserve every source word and punctuation, ",
                        "use word boundaries and at least one second per part"
                    ))?;
                    for (cue, part) in parts.iter_mut().zip(split.parts) {
                        cue.translated_text = Some(part.translation);
                        cue.target_language = target_language.clone();
                        cue.normalize_text();
                        let translation = cue
                            .translated_text
                            .as_deref()
                            .ok_or("Supply a nonempty translation for every split part")?;
                        if cue.original_text.chars().count() > 8_000
                            || translation.chars().count() > 8_000
                        {
                            return Err(
                                "Each text field must contain at most 8000 characters".into()
                            );
                        }
                    }
                    ids.extend(parts.iter().map(|cue| cue.id.clone()));
                    affected.extend(parts.iter().map(|cue| cue_location(cue, cue.end_ms)));
                    before.extend(feedback::items(&task_id, [&draft.cues()[index]]));
                    after.extend(feedback::items(&task_id, &parts));
                    draft.replace_split(index, parts);
                }
                workspace.draft = draft;
                feedback::changed(&task_id, before, after);
                workspace.revision += 1;
                let result = json!({"revision":workspace.revision,"cues":ids,"affected":affected});
                workspace.record(&call.name, result.clone());
                self.controller.dirty = true;
                Ok(result)
            }
            "media.commit" | "media.undo" => {
                let (id, revision) = if call.name == "media.commit" {
                    let args: Revision = arguments(call)?;
                    (args.workspace, Some(args.revision))
                } else {
                    let args: Scope = arguments(call)?;
                    (args.workspace, None)
                };
                let task_id = self
                    .workspace_task(&id)
                    .ok_or("Workspace not found")?
                    .id
                    .clone();
                let active = self.controller.active_task_id.as_deref() == Some(&task_id);
                if active && self.controller.subtitle_edit.is_some() {
                    return Err("Finish the current subtitle edit first".into());
                }
                let task = self.controller.store.get_mut(&task_id).unwrap();
                if task.is_task_running {
                    return Err("Task is running".into());
                }
                let current = if active {
                    &self.controller.subtitles
                } else {
                    &task.subtitles
                };
                if let Some(revision) = revision {
                    let workspace = task.optimization.as_ref().unwrap();
                    workspace.check_revision(revision)?;
                    workspace.check_base(task, current)?;
                }
                let previous = task.clone();
                let previous_current = current.clone();
                let workspace = task.optimization.as_mut().unwrap();
                let replacement = if revision.is_some() {
                    if issues(&workspace.draft, task.subtitle_char_limit)
                        .iter()
                        .any(|issue| {
                            issue["problems"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .any(|p| p == "invalid_timing")
                        })
                    {
                        return Err("Fix invalid timing before committing".into());
                    }
                    workspace.committed = Some(workspace.draft.clone());
                    workspace.status = "committed".into();
                    workspace.draft.clone()
                } else {
                    let committed = workspace.committed.as_ref().ok_or("No commit to undo")?;
                    if !same_timeline(current, committed) {
                        return Err("Conflict: published subtitles have changed".into());
                    }
                    workspace.status = "reverted".into();
                    workspace.committed = None;
                    workspace.base.clone()
                };
                workspace.revision += 1;
                let result = json!({"revision":workspace.revision,"status":workspace.status,"remaining_issues":issues(&replacement,task.subtitle_char_limit).len()});
                workspace.record(&call.name, result.clone());
                task.subtitles = replacement.clone();
                if active {
                    self.controller.subtitles.replace(replacement);
                }
                self.controller.dirty = true;
                self.controller.save_active();
                if self.controller.dirty {
                    if active {
                        self.controller.subtitles.replace(previous_current);
                    }
                    *self.controller.store.get_mut(&task_id).unwrap() = previous;
                    return Err(self
                        .controller
                        .error
                        .clone()
                        .unwrap_or("Could not save subtitles".into()));
                }
                feedback::status(&task_id, "", ui_director::feedback::Effect::Invoke);
                Ok(result)
            }
            _ => Err(format!("Unknown media tool: {}", call.name)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        backend::MediaSource,
        task::{MediaTask, MediaTaskStore, SubtitleMode, default_video_recognition},
    };
    use super::*;

    fn fixture() -> (MediaPlugin, tempfile::TempDir, String) {
        fixture_from_file("review.srt")
    }

    fn fixture_from_file(file: &str) -> (MediaPlugin, tempfile::TempDir, String) {
        let storage = tempfile::tempdir().unwrap();
        let mut plugin = MediaPlugin::new(crate::media::controller::MediaController::open(
            storage.path().into(),
            None,
        ));
        plugin.controller.backend = None;
        plugin.controller.storage_dir = storage.path().into();
        plugin.controller.store.tasks.clear();
        let mut task = MediaTask::new(
            "Review".into(),
            MediaSource::LocalFile(file.into()),
            "auto".into(),
            "zh".into(),
            SubtitleMode::TranslateExisting,
            default_video_recognition(),
        );
        task.subtitles = SubtitleTimeline::parse(
            "1\n00:00:01,000 --> 00:00:09,000\nHello world. Goodbye world.\n",
        )
        .unwrap();
        let mut cue = task.subtitles.cues()[0].clone();
        cue.source_language = "en".into();
        task.subtitles.edit_cue(cue).unwrap();
        let id = task.id.clone();
        plugin.controller.store.add_or_update(task);
        plugin.controller.open_task(&id).unwrap();
        let workspace = call(&mut plugin, "media.open", json!({"task":id})).unwrap()["workspace"]
            .as_str()
            .unwrap()
            .to_owned();
        (plugin, storage, workspace)
    }

    fn call(plugin: &mut MediaPlugin, name: &str, arguments: Value) -> ToolResult {
        ToolProvider::call(
            plugin,
            &ToolCall {
                name: name.into(),
                arguments,
            },
        )
    }

    #[test]
    fn workspace_split_edit_commit_undo_preserves_source_and_updates_ui_and_disk() {
        let (mut plugin, storage, workspace) = fixture();
        let original = plugin.controller.subtitles.export_srt();
        let cue = plugin.controller.subtitles.cues()[0].id.clone();
        let before = plugin.workspace_mut(&workspace).unwrap().draft.clone();
        // A later invalid part must not publish a partial split or clear translations.
        for last in [
            json!({"original":"Goodbye world.","translation":"\n \r\n"}),
            json!({"original":"Goodbye world."}),
            json!({"original":"Goodbye.","translation":"再见。"}),
        ] {
            assert!(
                call(
                    &mut plugin,
                    "media.split",
                    json!({
                        "workspace":workspace,"revision":0,"cue":cue,
                        "parts":[{"original":"Hello world.","translation":"你好，世界。"},last]
                    })
                )
                .is_err()
            );
            let unchanged = plugin.workspace_mut(&workspace).unwrap();
            assert_eq!(unchanged.revision, 0);
            assert!(same_timeline(&unchanged.draft, &before));
        }
        let split = call(
            &mut plugin,
            "media.split",
            json!({
                "workspace":workspace,"revision":0,"cue":cue,"parts":[
                    {"original":"Hello world.","translation":" 你好，世界。\r\n "},
                    {"original":"Goodbye world.","translation":"再见，世界。"}
                ]
            }),
        )
        .unwrap();
        let ids = split["cues"].as_array().unwrap();
        assert_eq!(split["affected"][0]["start_ms"], 1000);
        assert_eq!(split["affected"][1]["end_ms"], 9000);
        let records = call(&mut plugin, "media.records", json!({"workspace":workspace})).unwrap();
        assert_eq!(records[0]["result"]["affected"], split["affected"]);
        let preview = &plugin.workspace_mut(&workspace).unwrap().draft;
        assert_eq!(
            preview
                .cues()
                .iter()
                .map(|cue| cue.translated_text.as_deref())
                .collect::<Vec<_>>(),
            [Some("你好，世界。"), Some("再见，世界。")]
        );
        assert!(
            preview
                .cues()
                .iter()
                .all(|cue| cue.source_language == "en" && cue.target_language == "zh")
        );
        assert!(issues(preview, 200).is_empty());
        // One missing cue must reject the whole edit, including its valid first item.
        assert!(
            call(
                &mut plugin,
                "media.edit",
                json!({"workspace":workspace,"revision":1,"edits":[
            {"cue":ids[0],"translation":"你好。"},{"cue":"missing","translation":"再见。"}]})
            )
            .is_err()
        );
        assert_eq!(
            plugin.workspace_mut(&workspace).unwrap().draft.cues()[0]
                .translated_text
                .as_deref(),
            Some("你好，世界。")
        );
        call(&mut plugin, "media.edit", json!({"workspace":workspace,"revision":1,"edits":[
            {"cue":ids[0],"translation":"\n你好。\n\n世界！"},{"cue":ids[1],"translation":"再见，世界。"}]})).unwrap();
        assert_eq!(plugin.controller.subtitles.export_srt(), original);
        call(
            &mut plugin,
            "media.settings",
            json!({"workspace":workspace,"preprocessing":"none","pause_tolerance":0.75}),
        )
        .unwrap();
        assert_eq!(
            plugin.controller.store.tasks[0].recognition.pause_tolerance,
            0.75
        );
        call(
            &mut plugin,
            "media.commit",
            json!({"workspace":workspace,"revision":2}),
        )
        .unwrap();
        let cues = plugin.controller.subtitles.cues();
        assert_eq!(cues.len(), 2);
        assert_eq!(
            cues.iter()
                .map(|cue| cue.original_text.as_str())
                .collect::<Vec<_>>(),
            ["Hello world.", "Goodbye world."]
        );
        assert_eq!(cues[0].end_ms, cues[1].start_ms);
        assert_eq!((cues[0].start_ms, cues[1].end_ms), (1000, 9000));
        assert_eq!(cues[0].translated_text.as_deref(), Some("你好。\n世界！"));
        assert!(cues.iter().all(|cue| cue.source_language == "en"));
        assert_eq!(
            plugin.controller.subtitles.metadata_for(&cues[0].id).timing,
            xrtranslate_protocol::SegmentTiming::EstimatedTextPartition
        );
        let saved = MediaTaskStore::load_from_dir(storage.path());
        assert_eq!(
            saved.tasks[0].subtitles.export_srt(),
            plugin.controller.subtitles.export_srt()
        );
        call(&mut plugin, "media.undo", json!({"workspace":workspace})).unwrap();
        assert_eq!(plugin.controller.subtitles.export_srt(), original);
    }

    #[test]
    fn split_batch_is_atomic_and_preserves_independent_timing() {
        let (mut plugin, _storage, workspace) = fixture();
        let timeline = SubtitleTimeline::parse(concat!(
            "1\n00:00:01,000 --> 00:00:09,000\nHello world. Goodbye world.\n\n",
            "2\n00:00:12,000 --> 00:00:20,000\nHello world. Goodbye world.\n"
        ))
        .unwrap();
        plugin.workspace_mut(&workspace).unwrap().draft = timeline.clone();
        let split = |index: usize| {
            json!({"cue":timeline.cues()[index].id,"parts":[
                {"original":"Hello world.","translation":"你好，世界。"},
                {"original":"Goodbye world.","translation":"再见，世界。"}
            ]})
        };
        let mut invalid = split(1);
        invalid["parts"][1]["translation"] = json!(" ");
        for splits in [
            json!([split(0), invalid]),
            json!([split(0), split(0)]),
            json!([]),
            json!(vec![split(0); 9]),
        ] {
            assert!(
                call(
                    &mut plugin,
                    "media.split",
                    json!({
                        "workspace":workspace,"revision":0,"splits":splits
                    })
                )
                .is_err()
            );
            let unchanged = plugin.workspace_mut(&workspace).unwrap();
            assert!(same_timeline(&unchanged.draft, &timeline));
            assert_eq!(unchanged.revision, 0);
            assert!(unchanged.records.is_empty());
        }
        assert!(
            call(
                &mut plugin,
                "media.split",
                json!({
                    "workspace":workspace,"revision":0,"splits":[split(0)],
                    "cue":timeline.cues()[0].id,"parts":split(0)["parts"]
                })
            )
            .is_err()
        );
        let result = call(
            &mut plugin,
            "media.split",
            json!({
                "workspace":workspace,"revision":0,"splits":[split(1),split(0)]
            }),
        )
        .unwrap();
        assert_eq!(result["revision"], 1);
        assert_eq!(result["cues"].as_array().unwrap().len(), 4);
        let draft = &plugin.workspace_mut(&workspace).unwrap().draft;
        let cues = draft.cues();
        for (parts, original) in cues.chunks(2).zip(timeline.cues()) {
            assert_eq!(parts[0].start_ms, original.start_ms);
            assert_eq!(parts[0].end_ms, parts[1].start_ms);
            assert_eq!(parts[1].end_ms, original.end_ms);
            assert_eq!(
                format!("{} {}", parts[0].original_text, parts[1].original_text),
                original.original_text
            );
            assert!(
                parts
                    .iter()
                    .all(|cue| cue.translated_text.is_some() && cue.target_language == "zh")
            );
        }
        assert_eq!(
            plugin.controller.subtitles.count(),
            1,
            "draft is not published until commit"
        );
    }

    #[test]
    fn merge_rejects_invalid_pairs_and_publishes_only_on_commit() {
        let (mut plugin, storage, workspace) = fixture();
        let original = plugin.controller.subtitles.clone();
        let split = call(
            &mut plugin,
            "media.split",
            json!({
                "workspace":workspace,"revision":0,"cue":original.cues()[0].id,
                "parts":[
                    {"original":"Hello","translation":"你好"},
                    {"original":"world.","translation":"世界。"},
                    {"original":"Goodbye world.","translation":"再见，世界。"}
                ]
            }),
        )
        .unwrap();
        let ids = split["cues"].as_array().unwrap();
        let before = plugin.workspace_mut(&workspace).unwrap().draft.clone();
        for (revision, source, target) in [
            (0, ids[0].clone(), ids[1].clone()),
            (1, ids[0].clone(), ids[2].clone()),
            (1, ids[0].clone(), ids[0].clone()),
            (1, json!("missing"), ids[1].clone()),
        ] {
            assert!(call(&mut plugin, "media.merge", json!({
                "workspace":workspace,"revision":revision,"source_cue":source,"target_cue":target
            })).is_err());
            let unchanged = plugin.workspace_mut(&workspace).unwrap();
            assert_eq!(unchanged.revision, 1);
            assert!(same_timeline(&unchanged.draft, &before));
        }
        let result = call(
            &mut plugin,
            "media.merge",
            json!({
                "workspace":workspace,"revision":1,"source_cue":ids[0],"target_cue":ids[1]
            }),
        )
        .unwrap();
        assert_eq!(result["cue"], ids[1]);
        assert_eq!(result["revision"], 2);
        let draft = &plugin.workspace_mut(&workspace).unwrap().draft;
        assert_eq!(draft.count(), 2);
        assert_eq!(draft.cues()[0].original_text, "Hello world.");
        assert_eq!(draft.cues()[1], before.cues()[2]);
        assert_eq!(draft.cues()[0].start_ms, before.cues()[0].start_ms);
        assert_eq!(draft.cues()[0].end_ms, before.cues()[1].end_ms);
        assert!(same_timeline(&plugin.controller.subtitles, &original));
        call(
            &mut plugin,
            "media.commit",
            json!({"workspace":workspace,"revision":2}),
        )
        .unwrap();
        let saved = MediaTaskStore::load_from_dir(storage.path());
        assert_eq!(plugin.controller.subtitles.count(), 2);
        assert!(same_timeline(
            &saved.tasks[0].subtitles,
            &plugin.controller.subtitles
        ));
        call(&mut plugin, "media.undo", json!({"workspace":workspace})).unwrap();
        assert!(same_timeline(&plugin.controller.subtitles, &original));
    }

    #[test]
    fn reads_original_draft_and_trial_by_text_or_time_without_dumping_the_trial() {
        let (mut plugin, _storage, workspace) = fixture();
        let candidate = SubtitleTimeline::parse("1\n00:00:01,000 --> 00:00:04,000\nHello world.\n\n2\n00:00:04,000 --> 00:00:09,000\nGoodbye world.\n").unwrap();
        let original = plugin.controller.subtitles.clone();
        let mut captured_recognition = default_video_recognition();
        captured_recognition.preprocessing = Some(xrtranslate_protocol::AudioPreprocessing::Gtcrn);
        plugin.workspace_mut(&workspace).unwrap().trials.push(serde_json::from_value(json!({
            "id":"candidate","status":"completed","start_ms":1000,"end_ms":9000,
            "source":original.cues(),"recognition":captured_recognition,"audio_track":"Original",
            "timeline":candidate,"error":null
        })).unwrap());
        let receipt = call(
            &mut plugin,
            "media.operation",
            json!({"workspace":workspace,"operation":"candidate"}),
        )
        .unwrap();
        assert_eq!(receipt["document"], "candidate");
        assert_eq!(receipt["trials_remaining"], 7);
        assert!(receipt.get("timeline").is_none() && receipt.get("source").is_none());
        let first = call(
            &mut plugin,
            "media.read",
            json!({"workspace":workspace,"document":"candidate","query":"WORLD","limit":1}),
        )
        .unwrap();
        assert_eq!(first["cues"].as_array().unwrap().len(), 1);
        assert_eq!(first["recognition"]["preprocessing"], "gtcrn");
        assert_eq!(first["audio_track"], "Original");
        assert_eq!(first["trials_remaining"], 7);
        assert_eq!(first["next_offset"], 1);
        let second = call(&mut plugin, "media.read", json!({"workspace":workspace,"document":"candidate","query":"world","offset":first["next_offset"],"limit":1})).unwrap();
        assert_eq!(second["cues"][0]["index"], 1);
        assert!(second["next_offset"].is_null());
        call(&mut plugin, "media.edit", json!({"workspace":workspace,"revision":0,"edits":[{"cue":original.cues()[0].id,"translation":"你好，再见。"}]})).unwrap();
        for (document, expected) in [
            ("base", "Hello world. Goodbye world."),
            ("draft", "Hello world. Goodbye world."),
            ("candidate", "Goodbye world."),
        ] {
            let page = call(
                &mut plugin,
                "media.read",
                json!({"workspace":workspace,"document":document,"start_ms":4000,"end_ms":5000}),
            )
            .unwrap();
            assert_eq!(page["cues"].as_array().unwrap().len(), 1);
            assert_eq!(page["cues"][0]["cue"]["original_text"], expected);
            assert_eq!(
                page["cues"][0]["cue"]["translated_text"].as_str(),
                (document == "draft").then_some("你好，再见。")
            );
        }
        let translated = call(
            &mut plugin,
            "media.read",
            json!({"workspace":workspace,"query":"你好"}),
        )
        .unwrap();
        assert_eq!(translated["cues"].as_array().unwrap().len(), 1);
        for extra in [
            json!({"document":"missing"}),
            json!({"start_ms":5000,"end_ms":4000}),
            json!({"limit":0}),
        ] {
            let mut args = extra;
            args["workspace"] = json!(workspace);
            assert!(call(&mut plugin, "media.read", args).is_err());
        }
        assert!(same_timeline(&original, &plugin.controller.subtitles));
    }

    #[test]
    fn time_range_trials_fill_gaps_without_overwriting_partial_or_changed_cues() {
        use super::super::subtitles::{SubtitleCue, SubtitleMetadata};
        use crate::session_coordinator::{TranslationEvent, TranslationInput, TranslationOutcome};

        for (start, end, changed, allowed) in [
            (9_000, 12_000, false, true),
            (2_000, 5_000, false, false),
            (1_000, 9_000, true, false),
            (1_000, 9_000, false, true),
        ] {
            let (mut plugin, _storage, workspace) = fixture_from_file("review.mp4");
            call(
                &mut plugin,
                "media.settings",
                json!({"workspace":workspace,"preprocessing":"none"}),
            )
            .unwrap();
            let original = plugin.controller.subtitles.clone();
            let range = if start == 1_000 && !changed {
                json!({"first_cue":original.cues()[0].id,"last_cue":original.cues()[0].id})
            } else {
                json!({"start_ms":start,"end_ms":end})
            };
            let mut args = range;
            args["workspace"] = json!(workspace);
            args["revision"] = json!(0);
            let receipt = call(&mut plugin, "media.trial", args).unwrap();
            let operation = receipt["operation"].as_str().unwrap();
            let request = plugin.next_workspace_task().unwrap();
            let TranslationInput::File { options, .. } = request.input else {
                panic!("A trial must reuse the finite file input");
            };
            assert_eq!(options.range_ms, Some((start as u64, end as u64)));

            // The normal completion path restores absolute timestamps before adoption.
            let mut candidate = SubtitleTimeline::new();
            candidate.add_cue_with_metadata(
                SubtitleCue {
                    id: "recovered".into(),
                    start_ms: 100,
                    end_ms: 1_100,
                    original_text: "Recovered speech.".into(),
                    translated_text: Some("补回的语音。".into()),
                    ..original.cues()[0].clone()
                },
                SubtitleMetadata::authored(),
            );
            plugin
                .workspace_mut(&workspace)
                .unwrap()
                .trials
                .last_mut()
                .unwrap()
                .timeline = candidate;
            assert!(plugin.accept_workspace_event(
                operation,
                &TranslationEvent::Finished {
                    stream_id: 99,
                    outcome: TranslationOutcome::Completed,
                }
            ));
            if changed {
                call(
                    &mut plugin,
                    "media.edit",
                    json!({"workspace":workspace,"revision":0,
                    "edits":[{"cue":original.cues()[0].id,"original":"User correction."}]}),
                )
                .unwrap();
            }
            let before = plugin.workspace_mut(&workspace).unwrap().draft.clone();
            let result = call(
                &mut plugin,
                "media.apply_trial",
                json!({"workspace":workspace,
                "revision":u64::from(changed),"operation":operation}),
            );
            assert_eq!(
                result.is_ok(),
                allowed,
                "{start}..{end}, changed={changed}: {result:?}"
            );
            let draft = &plugin.workspace_mut(&workspace).unwrap().draft;
            if allowed {
                let recovered = draft
                    .cues()
                    .iter()
                    .find(|cue| cue.id == "recovered")
                    .unwrap();
                assert_eq!(
                    (recovered.start_ms, recovered.end_ms),
                    (start + 100, start + 1_100)
                );
                if start == 9_000 {
                    assert_eq!(draft.count(), 2);
                    assert_eq!(draft.cues()[0], original.cues()[0]);
                }
            } else {
                assert!(same_timeline(draft, &before));
            }
            assert!(same_timeline(&original, &plugin.controller.subtitles));
        }
    }

    #[test]
    fn source_edits_and_deletion_commit_together_and_undo_restores_the_document() {
        let (mut plugin, storage, workspace) = fixture();
        let original = plugin.controller.subtitles.clone();
        let cue = original.cues()[0].id.clone();
        let split = call(
            &mut plugin,
            "media.split",
            json!({
                "workspace":workspace,"revision":0,"cue":cue,"parts":[
                    {"original":"Hello world.","translation":"你好，世界。"},
                    {"original":"Goodbye world.","translation":"再见，世界。"}
                ]
            }),
        )
        .unwrap();
        let first = split["cues"][0].as_str().unwrap();
        let second = split["cues"][1].as_str().unwrap();
        let before = plugin.workspace_mut(&workspace).unwrap().draft.clone();
        assert!(
            call(
                &mut plugin,
                "media.edit",
                json!({"workspace":workspace,"revision":1,"edits":[
                    {"cue":first,"original":"Changed."}, {"cue":second,"original":"\n  \r\n"}
                ]})
            )
            .is_err()
        );
        assert!(same_timeline(
            &plugin.workspace_mut(&workspace).unwrap().draft,
            &before
        ));
        assert!(
            call(
                &mut plugin,
                "media.edit",
                json!({"workspace":workspace,"revision":1,"edits":[{"cue":first}]})
            )
            .is_err()
        );
        call(
            &mut plugin,
            "media.edit",
            json!({"workspace":workspace,"revision":1,"edits":[
                {"cue":first,"original":"\r\nHello\n\neveryone. ","translation":"\n大家好。\n"}
            ]}),
        )
        .unwrap();
        call(
            &mut plugin,
            "media.edit",
            json!({"workspace":workspace,"revision":2,"edits":[
                {"cue":first,"original":"Hello friends."}
            ]}),
        )
        .unwrap();
        assert!(
            call(
                &mut plugin,
                "media.delete",
                json!({"workspace":workspace,"revision":2,"cue":second})
            )
            .is_err()
        );
        assert!(
            call(
                &mut plugin,
                "media.delete",
                json!({"workspace":workspace,"revision":3,"cue":"missing"})
            )
            .is_err()
        );
        call(
            &mut plugin,
            "media.delete",
            json!({"workspace":workspace,"revision":3,"cue":second}),
        )
        .unwrap();
        assert!(
            same_timeline(&plugin.controller.subtitles, &original),
            "draft changes must not leak before commit"
        );
        call(
            &mut plugin,
            "media.commit",
            json!({"workspace":workspace,"revision":4}),
        )
        .unwrap();
        let published = &plugin.controller.subtitles;
        assert_eq!(published.count(), 1);
        let cue = &published.cues()[0];
        assert_eq!(cue.original_text, "Hello friends.");
        assert_eq!(
            cue.translated_text.as_deref(),
            Some("大家好。"),
            "omitted translation stays unchanged"
        );
        assert_eq!(cue.source_language, "en");
        assert_eq!(cue.target_language, "zh");
        assert_eq!(
            (cue.start_ms, cue.end_ms),
            (before.cues()[0].start_ms, before.cues()[0].end_ms)
        );
        assert_eq!(published.metadata_for(first), before.metadata_for(first));
        let saved = MediaTaskStore::load_from_dir(storage.path());
        assert!(same_timeline(&saved.tasks[0].subtitles, published));
        assert!(
            saved.tasks[0]
                .subtitles
                .cues()
                .iter()
                .all(|cue| cue.id != second)
        );
        call(&mut plugin, "media.undo", json!({"workspace":workspace})).unwrap();
        assert!(same_timeline(&plugin.controller.subtitles, &original));
        assert!(same_timeline(
            &MediaTaskStore::load_from_dir(storage.path()).tasks[0].subtitles,
            &original
        ));
    }

    #[test]
    fn optimization_resumes_edits_but_refreshes_after_subtitles_or_context_change() {
        for active in [false, true] {
            for regenerate in [false, true] {
                let (mut plugin, storage, workspace) = fixture();
                let task = plugin.controller.active_task_id.clone().unwrap();
                let cue = plugin.controller.subtitles.cues()[0].id.clone();
                if !active {
                    plugin.controller.active_task_id = None;
                }
                call(
                    &mut plugin,
                    "media.edit",
                    json!({"workspace":workspace,"revision":0,
                    "edits":[{"cue":cue,"translation":"你好，再见。"}]}),
                )
                .unwrap();
                let stored = plugin.controller.store.get_mut(&task).unwrap();
                stored.speech_track = Some(super::super::task::SpeechTrack {
                    path: storage.path().join("speech.wav"),
                    language: "zh".into(),
                    clips: Vec::new(),
                    rendered: true,
                });
                stored.play_speech_track = true;
                assert_eq!(
                    call(&mut plugin, "media.open", json!({"task":task})).unwrap()["workspace"],
                    workspace
                );
                // Persisted snapshots may include playback fields; they are not inputs.
                let context = &mut plugin.workspace_mut(&workspace).unwrap().context;
                context["speech_track"] = Value::Null;
                context["play_speech_track"] = false.into();
                let (resumed, _) = plugin.prepare_optimization(&task).unwrap();
                assert_eq!(resumed["workspace"], workspace);
                assert_eq!(plugin.workspace_mut(&workspace).unwrap().revision, 1);

                if regenerate {
                    let current = SubtitleTimeline::parse(
                        "1\n00:00:01,000 --> 00:00:05,000\nListen again.\n",
                    )
                    .unwrap();
                    if active {
                        plugin.controller.subtitles = current;
                    } else {
                        plugin.controller.store.get_mut(&task).unwrap().subtitles = current;
                    }
                } else {
                    plugin
                        .controller
                        .store
                        .get_mut(&task)
                        .unwrap()
                        .target_language = "fr".into();
                }
                let current = if active {
                    plugin.controller.subtitles.clone()
                } else {
                    plugin
                        .controller
                        .store
                        .get(&task)
                        .unwrap()
                        .subtitles
                        .clone()
                };
                let error = call(&mut plugin, "media.open", json!({"task":task})).unwrap_err();
                assert!(error.contains(if regenerate {
                    "subtitles changed"
                } else {
                    "target_language"
                }));
                assert!(
                    error.contains(&workspace),
                    "the draft must remain inspectable"
                );
                assert_eq!(plugin.workspace_mut(&workspace).unwrap().revision, 1);
                let published = call(
                    &mut plugin,
                    "media.read",
                    json!({"workspace":workspace,"document":"current"}),
                )
                .unwrap();
                assert_eq!(published["cues"][0]["cue"], json!(current.cues()[0]));
                assert_eq!(
                    call(
                        &mut plugin,
                        "media.commit",
                        json!({"workspace":workspace,"revision":1}),
                    )
                    .unwrap_err(),
                    error,
                );
                let (scope, _) = plugin.prepare_optimization(&task).unwrap();
                let fresh = scope["workspace"].as_str().unwrap();
                assert_ne!(
                    fresh, workspace,
                    "a new optimization must use current subtitles and constraints"
                );
                let draft = plugin.workspace_mut(fresh).unwrap();
                assert_eq!(draft.revision, 0);
                assert!(same_timeline(&draft.draft, &current));
                call(
                    &mut plugin,
                    "media.commit",
                    json!({"workspace":fresh,"revision":0}),
                )
                .unwrap();
                assert!(same_timeline(
                    &MediaTaskStore::load_from_dir(storage.path()).tasks[0].subtitles,
                    &current
                ));
            }
        }
    }

    #[test]
    fn workspace_rejects_conflicts_and_rolls_back_failed_saves() {
        let (mut plugin, storage, workspace) = fixture();
        let cue = plugin.controller.subtitles.cues()[0].id.clone();
        call(&mut plugin,"media.edit",json!({"workspace":workspace,"revision":0,"edits":[{"cue":cue,"translation":"你好，再见。"}]})).unwrap();
        plugin
            .controller
            .subtitles
            .set_translation(&cue, "用户修改".into(), "en", "zh");
        assert!(
            call(
                &mut plugin,
                "media.commit",
                json!({"workspace":workspace,"revision":1})
            )
            .unwrap_err()
            .contains("Conflict")
        );
        assert_eq!(
            plugin.controller.subtitles.cues()[0]
                .translated_text
                .as_deref(),
            Some("用户修改")
        );
        plugin.controller.subtitles = plugin.workspace_mut(&workspace).unwrap().base.clone();
        let original = plugin.controller.subtitles.export_srt();
        let bad_directory = storage.path().join("a-file");
        std::fs::write(&bad_directory, b"not a directory").unwrap();
        plugin.controller.storage_dir = bad_directory;
        assert!(
            call(
                &mut plugin,
                "media.commit",
                json!({"workspace":workspace,"revision":1})
            )
            .is_err()
        );
        assert_eq!(plugin.controller.subtitles.export_srt(), original);
        assert_eq!(plugin.workspace_mut(&workspace).unwrap().status, "open");
    }
}
