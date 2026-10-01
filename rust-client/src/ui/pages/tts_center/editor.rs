use crate::{
    i18n::{UiLanguage, tr},
    ui::components,
};
use eframe::egui;
use std::path::PathBuf;
use xrtranslate_engine::language::LanguageSet;

#[derive(Clone)]
pub struct VoiceDraft {
    pub name: String,
    pub description: String,
    pub transcript: String,
    pub path: Option<PathBuf>,
    pub manual_transcript: bool,
    pub source_language: String,
}

impl Default for VoiceDraft {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            transcript: String::new(),
            path: None,
            manual_transcript: false,
            source_language: "auto".into(),
        }
    }
}

/// The form only collects reference source data; it never handles clone formats.
pub fn show(
    ui: &egui::Ui,
    draft: &mut VoiceDraft,
    language: UiLanguage,
    asr_languages: LanguageSet,
    busy: bool,
) -> (bool, bool) {
    let mut open = true;
    let mut save = false;
    let mut cancel = false;
    egui::Window::new(tr(language, "Add voice"))
        .id(egui::Id::new("new_voice_card"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(420.0)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .vscroll(true)
        .show(ui.ctx(), |ui| {
            ui.add_enabled_ui(!busy, |ui| {
                ui.label(tr(language, "Name"));
                ui.add(
                    egui::TextEdit::singleline(&mut draft.name)
                        .char_limit(100)
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(8.0);
                let choose_reference =
                    components::secondary_button(ui, tr(language, "Choose reference audio"))
                        .clicked();
                if let Some(path) = crate::file_dialog::FileDialog::new()
                    .add_filter("Audio", &["wav", "mp3", "flac", "ogg", "m4a", "aac"])
                    .pick_file(ui.ctx(), "tts_reference_audio", choose_reference)
                {
                    if draft.name.trim().is_empty() {
                        draft.name = path
                            .file_stem()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .chars()
                            .take(100)
                            .collect();
                    }
                    draft.path = Some(path);
                }
                ui.weak("0.5–60 s");
                if let Some(path) = &draft.path {
                    ui.label(path.file_name().unwrap_or_default().to_string_lossy());
                }
                ui.add_space(8.0);
                ui.label(tr(language, "Reference transcript"));
                ui.horizontal(|ui| {
                    ui.selectable_value(
                        &mut draft.manual_transcript,
                        false,
                        tr(language, "Automatic (ASR)"),
                    );
                    ui.selectable_value(
                        &mut draft.manual_transcript,
                        true,
                        tr(language, "Specify text"),
                    );
                });
                if draft.manual_transcript {
                    ui.add(
                        egui::TextEdit::multiline(&mut draft.transcript)
                            .hint_text(tr(language, "Exact words spoken in the audio"))
                            .desired_width(f32::INFINITY)
                            .desired_rows(5)
                            .char_limit(20_000),
                    );
                } else {
                    ui.horizontal(|ui| {
                        ui.label(tr(language, "Audio language"));
                        components::source_language_selector(
                            ui,
                            "voice_reference_language",
                            &mut draft.source_language,
                            asr_languages,
                            false,
                            language,
                        );
                    });
                }
                ui.add_space(8.0);
                ui.label(tr(language, "Description (optional)"));
                ui.add(
                    egui::TextEdit::singleline(&mut draft.description)
                        .desired_width(f32::INFINITY)
                        .char_limit(500),
                );
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    save = components::primary_button_enabled(
                        ui,
                        tr(language, "Add"),
                        draft.path.is_some()
                            && !draft.name.trim().is_empty()
                            && (!draft.manual_transcript || !draft.transcript.trim().is_empty()),
                    )
                    .clicked();
                    cancel = components::secondary_button(ui, tr(language, "Cancel")).clicked();
                });
            });
            if busy {
                ui.spinner();
            }
        });
    (open && !cancel, save)
}
