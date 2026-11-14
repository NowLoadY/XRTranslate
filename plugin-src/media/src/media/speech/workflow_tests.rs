use super::*;
use crate::media::{
    backend::MediaSource,
    controller::SubtitleField,
    subtitles::{SubtitleCue, SubtitleTimeline},
    task::{MediaTaskStore, SubtitleMode},
};
use base64::{Engine, engine::general_purpose::STANDARD};

fn fixture(path: &Path) -> MediaController {
    let mut task = MediaTask::new(
        "Dubbing".into(),
        MediaSource::LocalFile("video.mp4".into()),
        "en".into(),
        "zh".into(),
        SubtitleMode::TranslateExisting,
        Default::default(),
    );
    task.duration_ms = 10000;
    task.subtitles = SubtitleTimeline::parse(
        "1\n00:00:01,000 --> 00:00:03,000\nHello.\n\n2\n00:00:06,000 --> 00:00:09,000\nGoodbye.",
    )
    .unwrap();
    for (id, text) in [("srt_0", "你好"), ("srt_1", "再见")] {
        task.subtitles.set_translation(id, text.into(), "en", "zh");
    }
    let mut controller = MediaController::open(path.into(), None);
    controller.backend = None;
    controller.active_task_id = Some(task.id.clone());
    controller.subtitles = task.subtitles.clone();
    controller.store.tasks = vec![task];
    controller
}

fn wav() -> Vec<u8> {
    let mut data = Cursor::new(Vec::new());
    let mut writer = hound::WavWriter::new(&mut data, SPEC).unwrap();
    for _ in 0..SAMPLE_RATE / 2 {
        writer.write_sample(100_i16).unwrap();
    }
    writer.finalize().unwrap();
    data.into_inner()
}

fn clip(path: &Path, cue: &SubtitleCue) -> SpeechClip {
    SpeechClip {
        id: uuid::Uuid::new_v4().to_string(),
        path: path.into(),
        start_ms: cue.start_ms,
        duration_ms: 500,
        text: cue.translated_text.clone().unwrap(),
        origin: Some(SpeechOrigin {
            cue_id: cue.id.clone(),
            start_ms: cue.start_ms,
            end_ms: cue.end_ms,
            language: "zh".into(),
            voice_id: None,
        }),
        ..Default::default()
    }
}

fn publish(controller: &mut MediaController, clip: SpeechClip) -> Result<(), String> {
    let temporary = tempfile::NamedTempFile::new_in(&controller.storage_dir)
        .unwrap()
        .into_temp_path();
    std::fs::write(&temporary, wav()).unwrap();
    controller.save_speech_clip(clip, temporary)
}

#[test]
fn project_voices_survive_reopening_and_do_not_need_selected_audio_or_source_stems() {
    let directory = tempfile::tempdir().unwrap();
    let mut controller = fixture(directory.path());
    let task_id = controller.active_task_id.clone().unwrap();
    let reference = reference::Prepared {
        id: "speaker:Alice".into(),
        audio: STANDARD.encode(wav()),
        range: [1000, 6000],
    };
    controller
        .save_project_voice(&task_id, reference.clone())
        .unwrap();
    controller
        .save_project_voice(&task_id, reference.clone())
        .unwrap();
    controller.rename_project_voice(&reference.id, "Alice".into());
    let restored = MediaTaskStore::load_from_dir(directory.path());
    let mut task = restored.tasks[0].clone();
    assert_eq!(task.speech_voices.len(), 1);
    assert_eq!(task.speech_voices[0].name, "Alice");
    assert!(task.speech_voices[0].reference.is_file());
    let mut cue = task.subtitles.cues()[0].clone();
    cue.speaker_name = Some("Alice".into());
    task.subtitles.edit_cue(cue).unwrap();
    assert!(task.separated_audio.is_none());
    let reused = reference::References::default().choose(&task, 0).unwrap();
    assert_eq!(reused.audio, reference.audio);
    assert_eq!(reused.range, reference.range);
    controller.remove_project_voice(&reference.id);
    let restored = MediaTaskStore::load_from_dir(directory.path());
    assert!(restored.tasks[0].speech_voices.is_empty());
    assert!(restored.tasks[0].speech_voices_migrated);
}

#[test]
fn edits_keep_the_old_audio_until_commit_and_allow_reverting_a_take() {
    let directory = tempfile::tempdir().unwrap();
    let mut controller = fixture(directory.path());
    let original = clip(
        &directory.path().join("old.wav"),
        &controller.subtitles.cues()[0],
    );
    controller.start_cue_speech("srt_0".into());
    publish(&mut controller, original.clone()).unwrap();
    controller.speech_job = None;
    let task = &controller.store.tasks[0];
    assert!(
        task.speech_track
            .as_ref()
            .unwrap()
            .current(task, &task.subtitles.cues()[0])
    );
    controller.begin_subtitle_edit("srt_0", SubtitleField::Translation);
    controller
        .subtitle_edit
        .as_mut()
        .unwrap()
        .cue
        .translated_text = Some("你好呀".into());
    controller.save_subtitle_edit();
    let task = &controller.store.tasks[0];
    assert!(
        !task
            .speech_track
            .as_ref()
            .unwrap()
            .current(task, &task.subtitles.cues()[0])
    );
    assert!(original.path.is_file());
    controller.start_cue_speech("srt_0".into());
    // Editing another cue must not invalidate the completed result for this cue.
    let mut other = controller.subtitles.cues()[1].clone();
    other.translated_text = Some("回头见".into());
    controller.subtitles.edit_cue(other).unwrap();
    let replacement = clip(
        &directory.path().join("new.wav"),
        &controller.subtitles.cues()[0],
    );
    publish(&mut controller, replacement.clone()).unwrap();
    controller.speech_job = None;
    let saved = &controller.store.tasks[0]
        .speech_track
        .as_ref()
        .unwrap()
        .clips[0];
    assert_eq!(saved.previous.as_deref(), Some(&original));
    controller.restore_speech(&replacement.id);
    let task = &controller.store.tasks[0];
    let track = task.speech_track.as_ref().unwrap();
    assert_eq!(track.clips[0].id, original.id);
    assert_eq!(track.clips[0].previous.as_ref().unwrap().id, replacement.id);
    assert!(
        !track.current(task, &task.subtitles.cues()[0]),
        "restoring old speech must not claim it matches edited text"
    );
    controller.restore_speech(&original.id);
    assert_eq!(
        controller.store.tasks[0]
            .speech_track
            .as_ref()
            .unwrap()
            .clips[0]
            .id,
        replacement.id
    );
    let restored = MediaTaskStore::load_from_dir(directory.path());
    assert_eq!(
        restored.tasks[0].speech_track.as_ref().unwrap().clips[0]
            .previous
            .as_ref()
            .unwrap()
            .id,
        original.id
    );
}

#[test]
fn a_late_result_cannot_replace_an_edited_cue_and_queue_targets_are_explicit() {
    let directory = tempfile::tempdir().unwrap();
    let mut controller = fixture(directory.path());
    let old = clip(
        &directory.path().join("late.wav"),
        &controller.subtitles.cues()[0],
    );
    controller.start_cue_with_voice("srt_0".into(), Some("one".into()));
    controller.start_cue_with_voice("srt_1".into(), Some("one".into()));
    controller.start_cue_with_voice("srt_1".into(), Some("two".into()));
    assert_eq!(controller.speech_queue.len(), 1);
    let queued = controller.speech_queue.front().unwrap();
    assert_eq!(Some(&queued.task_id), controller.active_task_id.as_ref());
    assert_eq!(queued.voice_id.as_deref(), Some("two"));
    let mut edited = controller.subtitles.cues()[0].clone();
    edited.translated_text = Some("新的字幕".into());
    controller.subtitles.edit_cue(edited).unwrap();
    publish(&mut controller, old.clone()).unwrap();
    assert!(!old.path.exists());
    assert!(controller.store.tasks[0].speech_track.is_none());
    controller.speech_job = None;
    controller.active_task_id = None;
    controller.start_queued_speech();
    let job = controller.speech_job.as_ref().unwrap();
    assert_eq!(job.task_id, controller.store.tasks[0].id);
    assert_eq!(job.selection.cue_id.as_deref(), Some("srt_1"));
    assert_eq!(job.selection.voice_id.as_deref(), Some("two"));
    controller.active_task_id = Some(job.task_id.clone());
    controller.cancel_project_speech();
    assert!(controller.speech_queue.is_empty());
}

#[test]
fn deleting_a_subtitle_removes_its_dubbing_and_previous_take() {
    let directory = tempfile::tempdir().unwrap();
    let mut controller = fixture(directory.path());
    controller.start_cue_speech("srt_0".into());
    let generated = clip(
        &directory.path().join("take.wav"),
        &controller.subtitles.cues()[0],
    );
    publish(&mut controller, generated).unwrap();
    controller.speech_job = None;
    assert!(controller.delete_subtitle("srt_0"));
    assert!(
        controller.store.tasks[0]
            .speech_track
            .as_ref()
            .unwrap()
            .clips
            .is_empty()
    );
    let restored = MediaTaskStore::load_from_dir(directory.path());
    assert_eq!(restored.tasks[0].subtitles.count(), 1);
    assert!(
        restored.tasks[0]
            .speech_track
            .as_ref()
            .unwrap()
            .clips
            .is_empty()
    );
}

#[test]
fn manual_timeline_offsets_and_staleness_are_independent() {
    let directory = tempfile::tempdir().unwrap();
    let mut controller = fixture(directory.path());
    let mut moved = clip(
        &directory.path().join("moved.wav"),
        &controller.subtitles.cues()[0],
    );
    std::fs::write(&moved.path, wav()).unwrap();
    moved.start_ms += 250;
    controller.store.tasks[0].speech_track = Some(SpeechTrack {
        path: directory.path().join("mix.wav"),
        language: "zh".into(),
        clips: vec![moved.clone()],
        rendered: false,
    });
    let task = &controller.store.tasks[0];
    assert!(
        task.speech_track
            .as_ref()
            .unwrap()
            .current(task, &task.subtitles.cues()[0])
    );
    controller.set_vocal_reference(Some([0, 5000]));
    controller.start_cue_speech("srt_0".into());
    let job = controller.speech_job.as_ref().unwrap();
    let snapshot = job.pending.as_ref().unwrap();
    assert!(snapshot.speech_reference_range.is_none());
    assert_eq!(snapshot.speech_track.as_ref().unwrap().clips[0], moved);
}

#[test]
fn batch_scopes_match_their_preview_and_report_only_their_own_cues() {
    let directory = tempfile::tempdir().unwrap();
    let mut controller = fixture(directory.path());
    controller.start_cue_speech("srt_0".into());
    let original = clip(
        &directory.path().join("take.wav"),
        &controller.subtitles.cues()[0],
    );
    publish(&mut controller, original).unwrap();
    controller.speech_job = None;
    controller.begin_subtitle_edit("srt_0", SubtitleField::Translation);
    controller
        .subtitle_edit
        .as_mut()
        .unwrap()
        .cue
        .translated_text = Some("你好呀".into());
    controller.save_subtitle_edit();
    for (scope, expected) in [
        (Scope::Missing, vec!["srt_1"]),
        (Scope::Changed, vec!["srt_0"]),
        (Scope::All, vec!["srt_0", "srt_1"]),
    ] {
        let task = &controller.store.tasks[0];
        let preview: Vec<_> = task
            .subtitles
            .cues()
            .iter()
            .filter(|cue| scope.contains(task, cue))
            .map(|cue| cue.id.as_str())
            .collect();
        assert_eq!(preview, expected);
        controller.start_speech_scope(scope, false);
        let job = controller.speech_job.as_ref().unwrap();
        assert_eq!(job.cues, expected);
        assert_eq!(
            controller.cue_speech(expected[0]),
            Some(CueSpeech::Generating)
        );
        if expected.len() > 1 {
            assert_eq!(controller.cue_speech(expected[1]), Some(CueSpeech::Queued));
            job.progress.completed.store(1, Ordering::Relaxed);
            assert_eq!(controller.cue_speech(expected[0]), None);
            assert_eq!(
                controller.cue_speech(expected[1]),
                Some(CueSpeech::Generating)
            );
        }
        controller.speech_job = None;
    }
}

#[test]
fn stopping_one_project_does_not_cancel_another_projects_queue() {
    let directory = tempfile::tempdir().unwrap();
    let mut controller = fixture(directory.path());
    let first = controller.active_task_id.clone().unwrap();
    let mut second = controller.store.tasks[0].clone();
    second.id = "second-project".into();
    controller.store.tasks.push(second);
    controller.start_cue_speech("srt_0".into());
    controller.start_cue_speech("srt_1".into());
    controller.active_task_id = Some("second-project".into());
    controller.start_cue_speech("srt_0".into());
    controller.active_task_id = Some(first);
    controller.cancel_project_speech();
    assert!(
        controller
            .speech_job
            .as_ref()
            .unwrap()
            .cancelled
            .load(Ordering::Relaxed)
    );
    assert_eq!(controller.cue_speech("srt_0"), None);
    assert_eq!(controller.speech_queue.len(), 1);
    assert_eq!(controller.speech_queue[0].task_id, "second-project");
    controller.speech_job = None;
    controller.start_queued_speech();
    assert_eq!(
        controller.speech_job.as_ref().unwrap().task_id,
        "second-project"
    );
}
