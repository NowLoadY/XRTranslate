use crate::plugins::meeting::{
    MeetingAction, MeetingInputRequest, MeetingReprocessRequest, MeetingStartRequest,
    MeetingUiSnapshot,
    controller::{MeetingController, MeetingPane, MeetingRoute},
    store::{MarkerKind, SegmentMarker},
};

#[derive(Clone)]
pub(super) enum UiAction {
    None,
    NewLive,
    NewImport,
    CreateAndStart,
    Open(String),
    Back,
    Continue(String),
    Pause,
    End,
    NewTopic,
    AddMarker(String, MarkerKind),
    SaveMarker(SegmentMarker),
    QuickNote,
    SaveMinutes,
    JumpToEvidence(String),
    Export,
    ExportMeeting(String),
    Reprocess,
    AskDelete(String),
    Delete(String),
    CancelDelete,
    RenameSpeaker(String, String),
    MergeSpeaker(String, String),
}

pub(super) fn apply_action(
    controller: &mut MeetingController,
    action: UiAction,
    snapshot: &MeetingUiSnapshot,
) -> MeetingAction {
    match action {
        UiAction::None => {}
        UiAction::NewLive => {
            controller.reset_draft(false, snapshot);
            controller.route = MeetingRoute::Create;
        }
        UiAction::NewImport => {
            controller.reset_draft(true, snapshot);
            controller.route = MeetingRoute::Create;
        }
        UiAction::Back => {
            controller.refresh_library();
            controller.route = MeetingRoute::Library;
        }
        UiAction::Open(id) => controller.open_meeting(&id),
        UiAction::CreateAndStart => {
            if let Some(error) = draft_validation_error(controller, snapshot) {
                controller.error = Some(error);
                return MeetingAction::None;
            }
            return MeetingAction::CreateAndStart(start_request(controller));
        }
        UiAction::Continue(id) => {
            if controller
                .active_meeting_id()
                .is_some_and(|active_id| active_id != id)
            {
                controller.error =
                    Some("Finish the active meeting before continuing a different meeting".into());
                return MeetingAction::None;
            }
            return MeetingAction::Continue(id);
        }
        UiAction::Pause => return MeetingAction::Pause,
        UiAction::End => return MeetingAction::End,
        UiAction::NewTopic => controller.create_topic(),
        UiAction::AddMarker(id, kind) => controller.add_marker(&id, kind),
        UiAction::SaveMarker(marker) => controller.save_marker(&marker),
        UiAction::QuickNote => controller.add_quick_note(),
        UiAction::SaveMinutes => controller.save_minutes(),
        UiAction::JumpToEvidence(segment_id) => {
            controller.pane = MeetingPane::Timeline;
            controller.evidence_target = Some(segment_id);
        }
        UiAction::AskDelete(id) => controller.pending_delete = Some(id),
        UiAction::Delete(id) => controller.delete(&id),
        UiAction::CancelDelete => controller.pending_delete = None,
        UiAction::Export => {
            if let Some(meeting_id) = controller
                .bundle
                .as_ref()
                .map(|bundle| bundle.meeting.id.clone())
            {
                return MeetingAction::Export(meeting_id);
            }
        }
        UiAction::ExportMeeting(meeting_id) => return MeetingAction::Export(meeting_id),
        UiAction::Reprocess => {
            let meeting_id = controller
                .bundle
                .as_ref()
                .map(|bundle| bundle.meeting.id.clone());
            let path = controller.reprocessable_audio_path();
            if let (Some(meeting_id), Some(path)) = (meeting_id, path) {
                if controller.active_meeting_id().is_some() {
                    controller.error = Some(
                        "Wait for the active meeting to finish before reprocessing audio".into(),
                    );
                    return MeetingAction::None;
                }
                if !path.is_file() {
                    controller.error =
                        Some("The retained audio file is no longer available".into());
                    return MeetingAction::None;
                }
                return MeetingAction::Reprocess(MeetingReprocessRequest {
                    meeting_id,
                    audio_path: path,
                    topic_title: "Reprocessed audio".into(),
                });
            } else {
                controller.error = Some("The retained audio file is no longer available".into());
            }
        }
        UiAction::RenameSpeaker(id, name) => controller.rename_speaker(&id, &name),
        UiAction::MergeSpeaker(source, target) => controller.merge_speakers(&source, &target),
    }
    MeetingAction::None
}

fn start_request(controller: &MeetingController) -> MeetingStartRequest {
    let draft = &controller.draft;
    let input = if draft.import_audio {
        MeetingInputRequest::ImportedAudio {
            path: draft.import_path.trim().into(),
        }
    } else {
        MeetingInputRequest::Live {
            source: draft.capture_source,
            save_recording: draft.save_recording,
        }
    };
    MeetingStartRequest {
        name: draft.name.trim().into(),
        source_language: draft.source_language.clone(),
        target_language: draft.target_language.clone(),
        input,
    }
}

pub(super) fn draft_validation_error(
    controller: &MeetingController,
    snapshot: &MeetingUiSnapshot,
) -> Option<String> {
    if !controller.storage_available() {
        return Some("Meeting storage is unavailable; recording is disabled".into());
    }
    if controller.draft.name.trim().is_empty() {
        return Some("Enter a meeting name".into());
    }
    if controller.active_meeting_id().is_some() {
        return Some("Finish the active meeting before starting another one".into());
    }
    if snapshot.host_session_busy {
        return Some("Stop the current translation session before starting a meeting".into());
    }
    if let Err(error) = snapshot.languages.select(
        &controller.draft.source_language,
        &controller.draft.target_language,
    ) {
        return Some(error);
    }
    if controller.draft.import_audio {
        let value = controller.draft.import_path.trim();
        if value.is_empty() {
            return Some("Choose an audio file".into());
        }
        if !std::path::Path::new(value).is_file() {
            return Some("The selected audio file is not available".into());
        }
    }
    None
}
