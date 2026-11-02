use crate::{
    i18n::UiLanguage,
    plugins::media::{
        backend::AudioTrack,
        i18n::tr,
        task::{AudioTrackSelection, MediaTask},
    },
};
use eframe::egui;

pub(super) fn selector(
    ui: &mut egui::Ui,
    task: &mut MediaTask,
    tracks: &[AudioTrack],
    language: UiLanguage,
) -> bool {
    let before = (task.audio_track, task.original_audio_track);
    let original = tracks
        .iter()
        .filter(|t| t.external.is_none())
        .collect::<Vec<_>>();
    let original_label = |id: Option<i64>| {
        original
            .iter()
            .find(|t| Some(t.id) == id)
            .map(|t| {
                if !t.title.is_empty() {
                    t.title.clone()
                } else if original.len() > 1 {
                    format!("{} {}", tr(language, "Original"), t.id)
                } else {
                    tr(language, "Original").into()
                }
            })
            .unwrap_or_else(|| tr(language, "Original").into())
    };
    let label = match task.audio_track {
        AudioTrackSelection::Original => original_label(task.original_audio_track),
        AudioTrackSelection::Vocals => tr(language, "Vocals").into(),
        AudioTrackSelection::Background => tr(language, "Background").into(),
    };
    ui.horizontal(|ui| {
        let label_width = ui
            .painter()
            .layout_no_wrap(
                tr(language, "Audio processing").into(),
                egui::TextStyle::Body.resolve(ui.style()),
                ui.visuals().text_color(),
            )
            .size()
            .x;
        let caption = ui.label(tr(language, "Audio track"));
        ui.add_space((label_width - caption.rect.width()).max(0.0));
        egui::ComboBox::from_id_salt("media_audio_track")
            .selected_text(label)
            .truncate()
            .width(160.0_f32.min(ui.available_width()))
            .show_ui(ui, |ui| {
                if original.is_empty() {
                    ui.selectable_value(
                        &mut task.audio_track,
                        AudioTrackSelection::Original,
                        tr(language, "Original"),
                    );
                }
                for track in original.iter() {
                    if ui
                        .selectable_label(
                            task.audio_track == AudioTrackSelection::Original
                                && task.original_audio_track == Some(track.id),
                            original_label(Some(track.id)),
                        )
                        .clicked()
                    {
                        task.audio_track = AudioTrackSelection::Original;
                        task.original_audio_track = Some(track.id);
                    }
                }
                if let Some(stems) = &task.separated_audio {
                    ui.separator();
                    for (value, label, path) in [
                        (AudioTrackSelection::Vocals, "Vocals", &stems.vocals),
                        (
                            AudioTrackSelection::Background,
                            "Background",
                            &stems.background,
                        ),
                    ] {
                        ui.add_enabled_ui(path.is_file(), |ui| {
                            ui.selectable_value(&mut task.audio_track, value, tr(language, label));
                        });
                    }
                }
            });
    });
    before != (task.audio_track, task.original_audio_track)
}
