/// Converts a diarization token into the compact label used by host and plugin
/// presentation surfaces.
pub(crate) fn compact_speaker_label(speaker_id: &str) -> Option<String> {
    let value = speaker_id.trim();
    if value.is_empty() {
        return None;
    }
    let suffix = value.strip_prefix("speaker-").unwrap_or(value);
    if suffix.eq_ignore_ascii_case("unknown") {
        return Some("S?".into());
    }
    let sequence = suffix.trim_start_matches('0');
    Some(format!(
        "S{}",
        if sequence.is_empty() { "0" } else { sequence }
    ))
}
