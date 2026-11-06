//! Shared client controls and session contracts, reused by the shell and native plugins.
#[cfg(target_os = "android")]
pub mod android;
pub mod audio_playback;
pub mod audio_processing;
pub mod bookmarks;
pub mod companion;
pub mod export;
pub mod feature_access;
pub mod file_dialog;
pub mod i18n;
pub mod media_import;
pub mod osc;
pub mod playback;
pub mod recognition;
pub mod recording;
pub mod services;
pub mod session_coordinator;
pub mod streaming;
pub mod text_output;
pub mod ui;
pub mod usage_guidelines;
pub mod voice_interaction;
use i18n::UiLanguage;
use unicode_segmentation::UnicodeSegmentation;
pub fn language_label(ui_language: UiLanguage, code: &str) -> &'static str {
    if code == "auto" {
        return i18n::tr(ui_language, "Auto (bidirectional)");
    }
    xrtranslate_engine::language::SupportedLanguage::parse(code)
        .map(|language| i18n::tr(ui_language, language.name()))
        .unwrap_or_else(|| i18n::tr(ui_language, "Unknown language"))
}

/// Returns true if the two language codes are mutually exclusive and should not
/// both be selectable at the same time (e.g. zh and zh-TW are the same base
/// language and would produce a no-op or circular translation route).
pub fn languages_conflict(a: &str, b: &str) -> bool {
    use xrtranslate_engine::language::SupportedLanguage;
    match (SupportedLanguage::parse(a), SupportedLanguage::parse(b)) {
        (Some(a), Some(b)) => a.base_code() == b.base_code(),
        _ => a.eq_ignore_ascii_case(b),
    }
}

pub fn can_lookup(source: &str) -> bool {
    let source = source.trim();
    !source.is_empty()
        && !source.contains(['\n', '\r'])
        && source.chars().count() <= 80
        && source.unicode_words().count() <= 12
        && xrtranslate_engine::sentence_end_offsets(source).is_empty()
}

/// Display diarization numbers without changing stored speaker identities.
pub fn compact_speaker_label(speaker_id: &str) -> Option<String> {
    let value = speaker_id.trim();
    if value.is_empty() {
        return None;
    }
    let lower = value.to_ascii_lowercase();
    let suffix = lower
        .strip_prefix("speaker-")
        .or_else(|| lower.strip_prefix("speaker_"))
        .or_else(|| lower.strip_prefix("speaker "))
        .unwrap_or(&lower);
    if suffix == "unknown" || suffix == "s?" {
        return None;
    }
    let digits = suffix.strip_prefix('s').unwrap_or(suffix);
    if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
        let number = digits.trim_start_matches('0');
        Some(if number.is_empty() { "0" } else { number }.to_owned())
    } else {
        Some(value.to_owned())
    }
}
