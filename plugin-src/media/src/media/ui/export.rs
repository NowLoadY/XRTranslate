use crate::{
    i18n::UiLanguage,
    media::{controller::MediaController, i18n::tr, video_export::ExportSettings},
    ui::components,
};
use eframe::egui;

pub(super) fn settings(
    controller: &mut MediaController,
    language: UiLanguage,
    ctx: &egui::Context,
) {
    let Some(id) = controller.pending_video_export.clone() else {
        return;
    };
    let Some(task) = controller.store.get_mut(&id) else {
        controller.pending_video_export = None;
        return;
    };
    let mut settings = task
        .video_export_settings
        .unwrap_or_else(|| ExportSettings::for_task(task));
    let busy = controller.subtitle_edit.is_some()
        || controller.speech_edit.is_some()
        || controller
            .speech_job
            .as_ref()
            .is_some_and(|job| job.task_id == id);
    let tracks = controller
        .backend
        .as_ref()
        .map(|backend| backend.audio_tracks())
        .unwrap_or_default();
    let mut mix_changed = false;
    let mut export = false;
    let mut cancel = false;
    let response = egui::Modal::new(egui::Id::unique("media_video_export")).show(ctx, |ui| {
        ui.set_width(340.0);
        ui.heading(tr(language, "Export video"));
        ui.add(egui::Label::new(egui::RichText::new(&task.title).weak()).truncate());
        ui.add_space(12.0);
        ui.add_enabled_ui(!busy, |ui| {
            mix_changed = super::detail::audio_tracks::controls(task, tracks, language, ui, false);
        });
        ui.add_space(8.0);
        ui.add_enabled_ui(
            task.subtitles.count() > 0 || settings.burn_subtitles,
            |ui| {
                components::checkbox(
                    ui,
                    &mut settings.burn_subtitles,
                    tr(language, "Burn subtitles into video"),
                );
            },
        );
        let problem = if busy {
            Some("Finish the current task or edit first.")
        } else {
            settings.problem(task)
        };
        if let Some(problem) = problem {
            ui.label(tr(language, problem));
        }
        ui.add_space(12.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            export =
                components::primary_button_enabled(ui, tr(language, "Export"), problem.is_none())
                    .clicked();
            cancel = components::secondary_button(ui, tr(language, "Cancel")).clicked();
        });
    });
    // Keep the same choices when returning to this task, just like its languages/style.
    task.video_export_settings = Some(settings);
    if mix_changed {
        controller.apply_audio_selection(false);
    }
    if export || cancel || response.should_close() {
        controller.pending_video_export = None;
        controller.save_active();
    }
    if export {
        controller.start_video_export(&id, settings, ctx);
    }
}
