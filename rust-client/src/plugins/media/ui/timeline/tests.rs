use super::*;
use crate::plugins::media::{
    backend::MediaSource,
    task::{MediaTask, MediaTaskStore, SubtitleMode},
};

#[test]
fn dragging_edges_zooms_without_drift_commits_on_release_and_escape_cancels() {
    let storage = tempfile::tempdir().unwrap();
    let mut controller = MediaController::default();
    controller.backend = None;
    controller.storage_dir = storage.path().into();
    controller.store.tasks.clear();
    let mut task = MediaTask::new(
        "Timing".into(),
        MediaSource::LocalFile("timing.srt".into()),
        "en".into(),
        "zh".into(),
        SubtitleMode::None,
        Default::default(),
    );
    let cues: Vec<_> = (0..20).map(|i| serde_json::json!({"id":i.to_string(),"start_ms":i*5000,"end_ms":i*5000+3000,"original_text":"Hello.","translated_text":"你好。","speaker_name":null})).collect();
    task.subtitles =
        serde_json::from_value(serde_json::json!({"enabled":true,"cues":cues})).unwrap();
    controller.active_task_id = Some(task.id.clone());
    controller.subtitles = task.subtitles.clone();
    controller.store.add_or_update(task);
    controller.save_active();
    let ctx = egui::Context::default();
    let mut clock = 0.0;
    let mut frame = |controller: &mut MediaController, events| {
        clock += 1.0 / 60.0;
        let mut view = None;
        let mut action = None;
        let output = ctx.run_ui(
            egui::RawInput {
                time: Some(clock),
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(640.0, 200.0),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                action = overview(controller, &controller.subtitles, true, ui);
                view = ui.data(|data| {
                    data.get_temp::<View>(
                        ui.make_persistent_id(("subtitle_window", &controller.active_task_id)),
                    )
                });
            },
        );
        let rect = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Rect(shape)
                    if shape.rect.height() == 24.0 && shape.rect.width() > 300.0 =>
                {
                    Some(shape.rect)
                }
                _ => None,
            })
            .unwrap();
        match action {
            Some(Action::Trim {
                cue,
                edge,
                time,
                finished,
            }) => controller.trim_subtitle(&cue, edge, time, finished),
            Some(Action::CancelTrim) => controller.cancel_subtitle_trim(),
            _ => {}
        }
        (view.unwrap(), rect)
    };
    let button = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame(&mut controller, vec![]);
    let (initial, rect) = frame(&mut controller, vec![]);
    let press = egui::pos2(
        rect.left() + initial.fraction(23_000.0) as f32 * rect.width() - 2.0,
        rect.center().y,
    );
    frame(
        &mut controller,
        vec![egui::Event::PointerMoved(press), button(press, true)],
    );
    assert!(
        controller.subtitle_edit.is_some(),
        "grabbing an edge starts immediately"
    );
    let pointer = press + egui::vec2(10.0, 0.0);
    frame(&mut controller, vec![egui::Event::PointerMoved(pointer)]);
    let edit = controller
        .subtitle_edit
        .as_ref()
        .expect("edge drag starts a timing draft");
    assert!(edit.dragging_time);
    assert_eq!(edit.cue.id, "4");
    let end = edit.cue.end_ms;
    assert!((23_001..=25_000).contains(&end));
    assert_eq!(
        controller.subtitles.cues()[4].end_ms,
        23_000,
        "drag preview must not publish"
    );
    let mut magnified = initial;
    for _ in 0..30 {
        magnified = frame(&mut controller, vec![]).0;
        assert_eq!(
            controller.subtitle_edit.as_ref().unwrap().cue.end_ms,
            end,
            "zooming under a still mouse must not change timing"
        );
    }
    assert!(magnified.end - magnified.start < (initial.end - initial.start) * 0.4);
    assert!(
        (rect.left() + magnified.fraction(end as f64) as f32 * rect.width() - (pointer.x + 2.0))
            .abs()
            < 1.0
    );
    frame(&mut controller, vec![button(pointer, false)]);
    assert!(controller.subtitle_edit.is_none());
    assert_eq!(controller.subtitles.cues()[4].end_ms, end);
    assert_eq!(
        MediaTaskStore::load_from_dir(storage.path()).tasks[0]
            .subtitles
            .cues()[4]
            .end_ms,
        end
    );
    let saved = controller.subtitles.export_srt();
    let mut restored = magnified;
    for _ in 0..40 {
        restored = frame(&mut controller, vec![]).0;
    }
    assert!(restored.end - restored.start > (initial.end - initial.start) * 0.95);
    let start = egui::pos2(
        rect.left() + restored.fraction(20_000.0) as f32 * rect.width(),
        rect.center().y,
    );
    frame(
        &mut controller,
        vec![egui::Event::PointerMoved(start), button(start, true)],
    );
    frame(
        &mut controller,
        vec![egui::Event::PointerMoved(start + egui::vec2(10.0, 0.0))],
    );
    assert!(controller.subtitle_edit.is_some());
    frame(
        &mut controller,
        vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    assert!(controller.subtitle_edit.is_none());
    assert_eq!(controller.subtitles.export_srt(), saved);
    assert_eq!(
        MediaTaskStore::load_from_dir(storage.path()).tasks[0]
            .subtitles
            .export_srt(),
        saved
    );
    controller.trim_subtitle("4", SubtitleField::End, 24_000, false);
    controller.hide_video();
    assert!(
        controller.subtitle_edit.is_none(),
        "leaving the page cancels the drag"
    );
    assert_eq!(controller.subtitles.export_srt(), saved);
    controller.trim_subtitle("4", SubtitleField::End, 100_000, true);
    assert_eq!(controller.subtitles.cues()[4].end_ms, 25_000);
    controller.trim_subtitle("4", SubtitleField::Start, 100_000, true);
    assert_eq!(controller.subtitles.cues()[4].start_ms, 24_999);
}
