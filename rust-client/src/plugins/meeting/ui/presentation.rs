use crate::plugins::meeting::{
    MeetingAudioSource,
    i18n::tr,
    store::{Meeting, MeetingSourceKind},
};
use eframe::egui;

pub(super) fn page_header(
    ui: &mut egui::Ui,
    title: &str,
    language: crate::i18n::UiLanguage,
    right: impl FnOnce(&mut egui::Ui),
) {
    crate::ui::components::page_header(ui, tr(language, title), right);
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

pub(super) fn status_indicator(
    ui: &mut egui::Ui,
    language: crate::i18n::UiLanguage,
    status: crate::plugins::meeting::store::MeetingStatus,
    waiting_for_microphone: bool,
) {
    use crate::plugins::meeting::{controller::meeting_status_label, store::MeetingStatus};
    use crate::ui::components::{StatusIcon, status_icon};
    let icon = match status {
        MeetingStatus::Live if waiting_for_microphone => StatusIcon::Working,
        MeetingStatus::Live => StatusIcon::Recording,
        MeetingStatus::Paused => StatusIcon::Paused,
        MeetingStatus::Ended => StatusIcon::Complete,
        MeetingStatus::Imported => StatusIcon::Imported,
        MeetingStatus::Processing => StatusIcon::Working,
        MeetingStatus::Failed | MeetingStatus::Interrupted => StatusIcon::Warning,
        MeetingStatus::Draft => StatusIcon::Idle,
    };
    let label = if waiting_for_microphone && status == MeetingStatus::Live {
        crate::i18n::tr(language, "Waiting for microphone")
    } else {
        tr(language, meeting_status_label(status))
    };
    status_icon(ui, icon, label);
}
