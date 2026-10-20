/// Display diarization numbers without changing stored speaker identities.
pub(crate) fn compact_speaker_label(speaker_id: &str) -> Option<String> {
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
