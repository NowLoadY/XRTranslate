//! Media identities and text; comparison and presentation belong to UI Director.
use super::super::{subtitles::SubtitleCue, ui};
use ui_director::feedback::{Effect, Feedback, Item};

pub(super) fn items<'a>(task: &str, cues: impl IntoIterator<Item = &'a SubtitleCue>) -> Vec<Item> {
    cues.into_iter()
        .map(|cue| Item {
            target: ui::subtitle_target(task, &cue.id).value(),
            label: format!(
                "{:02}:{:02}",
                cue.start_ms / 60_000,
                cue.start_ms / 1_000 % 60
            ),
            text: match cue.translated_text.as_deref() {
                Some(translation) => format!("{}\n{translation}", cue.original_text),
                None => cue.original_text.clone(),
            },
        })
        .collect()
}

pub(super) fn changed(task: &str, before: Vec<Item>, after: Vec<Item>) {
    ui_director::egui::feedback::show(ui_director::feedback::changes(
        before,
        after,
        ui::subtitles_target(task).value(),
    ));
}

pub(super) fn status(task: &str, label: &str, effect: Effect) {
    ui_director::egui::feedback::show([Feedback::new(
        ui::subtitles_target(task).value(),
        label,
        effect,
    )]);
}

pub(super) fn settings(
    task: &str,
    before: &crate::client_settings::RecognitionSettings,
    after: &crate::client_settings::RecognitionSettings,
) {
    use xrtranslate_protocol::AudioPreprocessing;
    let processing = |value| {
        match value {
            None => "Default",
            Some(AudioPreprocessing::None) => "None",
            Some(AudioPreprocessing::Gtcrn) => "Speech denoising",
            Some(AudioPreprocessing::VocalSeparation) => "Vocal separation",
        }
        .to_owned()
    };
    let target = ui::settings_target(task).value();
    let changes = [
        (
            "Audio processing",
            processing(before.preprocessing),
            processing(after.preprocessing),
        ),
        (
            "Background noise",
            before.background_noise.to_string(),
            after.background_noise.to_string(),
        ),
        (
            "Pause tolerance",
            before.pause_tolerance.to_string(),
            after.pause_tolerance.to_string(),
        ),
    ]
    .into_iter()
    .filter_map(|(label, before, after)| Feedback::changed(target, label, &before, &after))
    .collect::<Vec<_>>();
    if changes.is_empty() {
        ui_director::egui::feedback::show([Feedback::new(target, "", Effect::Read)]);
    } else {
        ui_director::egui::feedback::show(changes);
    }
}
