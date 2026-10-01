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

pub(super) fn format_timestamp(ms: i64) -> String {
    if ms <= 0 {
        return "-".to_string();
    }
    let secs = ms / 1000;
    let days = (secs / 86400) as i64;
    let daytime = (secs % 86400) as i64;
    let hours = daytime / 3600;
    let minutes = (daytime % 3600) / 60;

    let z = days + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!("{y:04}-{m:02}-{d:02} {hours:02}:{minutes:02}")
}
