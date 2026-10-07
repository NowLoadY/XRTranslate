use crate::plugins::meeting::{
    MeetingAudioSource,
    i18n::tr,
    store::{MarkerKind, Meeting, MeetingSourceKind},
};
use eframe::egui;

pub(super) fn page_header(
    ui: &mut egui::Ui,
    title: &str,
    language: crate::i18n::UiLanguage,
    right: impl FnOnce(&mut egui::Ui),
) {
    let title_text = tr(language, title);
    let title = egui::RichText::new(title_text)
        .size(22.0)
        .color(crate::ui::theme::text_strong())
        .strong();
    if ui.available_width() < 600.0 {
        ui.add(egui::Label::new(title).wrap());
        ui.horizontal_wrapped(right);
    } else {
        ui.horizontal(|ui| {
            ui.label(title);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), right);
        });
    }
    ui.add_space(14.0);
}

pub(super) fn marker_label(kind: MarkerKind, language: crate::i18n::UiLanguage) -> &'static str {
    match kind {
        MarkerKind::KeyDecision => tr(language, "Key decision"),
        MarkerKind::ActionItem => tr(language, "Action item"),
        MarkerKind::Note => tr(language, "Note"),
    }
}

pub(super) fn source_label(meeting: &Meeting, language: crate::i18n::UiLanguage) -> &'static str {
    match meeting.source_kind {
        MeetingSourceKind::LiveCapture => tr(language, "Live"),
        MeetingSourceKind::ImportedAudio => tr(language, "Imported audio"),
    }
}

pub(super) fn meeting_language_label(code: &str, language: crate::i18n::UiLanguage) -> String {
    if code == "auto" {
        return tr(language, "Auto (bidirectional)").to_string();
    }
    if code.contains(',') {
        let parts: Vec<_> = code
            .split(',')
            .map(|part| single_language_label(part.trim(), language))
            .collect();
        return parts.join(" + ");
    }
    single_language_label(code, language)
}

fn single_language_label(code: &str, language: crate::i18n::UiLanguage) -> String {
    let english_name = crate::LANGUAGE_OPTIONS
        .iter()
        .find_map(|(value, label)| (*value == code).then_some(*label));
    if let Some(name) = english_name {
        tr(language, name).to_string()
    } else {
        code.to_string()
    }
}

pub(super) fn capture_label(
    source: MeetingAudioSource,
    language: crate::i18n::UiLanguage,
) -> &'static str {
    match source {
        MeetingAudioSource::Microphone => tr(language, "Microphone"),
        MeetingAudioSource::SystemAudio => tr(language, "System audio"),
        MeetingAudioSource::Both => tr(language, "Microphone + system"),
    }
}

pub(super) fn format_duration(ms: i64) -> String {
    let seconds = ms.max(0) / 1000;
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

pub(super) use crate::ui::components::format_timestamp;
