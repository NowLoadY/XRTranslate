use super::model::*;

const STREAM_TEXT_LIMIT: usize = 4_096;

pub(crate) fn collect_recognition_window(
    pending: &mut Vec<PendingRecognitionWindow>,
    stream_id: u64,
    continuous: bool,
    segment_index: u32,
    segment_count: u32,
    entry: RecognitionHistoryEntry,
) -> Option<RecognitionHistoryEntry> {
    if segment_count == 0 || segment_index == 0 || segment_index > segment_count {
        return None;
    }
    let turn_id = entry.turn_id.clone();
    let index = pending
        .iter()
        .position(|window| window.stream_id == stream_id && window.turn_id == turn_id)
        .unwrap_or_else(|| {
            if pending.len() >= 32 {
                pending.remove(0);
            }
            pending.push(PendingRecognitionWindow {
                stream_id,
                continuous,
                turn_id,
                segment_count: segment_count.max(1),
                segments: Vec::new(),
            });
            pending.len() - 1
        });
    let window = &mut pending[index];
    if window.segment_count != segment_count {
        pending.remove(index);
        return None;
    }
    if let Some((_, existing)) = window
        .segments
        .iter_mut()
        .find(|(index, _)| *index == segment_index)
    {
        *existing = entry;
    } else {
        window.segments.push((segment_index, entry));
    }
    if window.segments.len() != window.segment_count as usize
        || !(1..=window.segment_count)
            .all(|expected| window.segments.iter().any(|(index, _)| *index == expected))
    {
        return None;
    }

    let mut window = pending.remove(index);
    window.segments.sort_by_key(|(index, _)| *index);
    let (_, first) = window.segments.first()?.clone();
    let mut combined = RecognitionHistoryEntry {
        stream_id: Some(window.stream_id),
        live: window.continuous,
        text: String::new(),
        source_start_ms: first.source_start_ms,
        source_end_ms: first.source_end_ms,
        activation_matches: Vec::new(),
        context_matches: Vec::new(),
        revision: None,
        ..first
    };
    for (_, segment) in window.segments {
        let position = crate::streaming::append_segment(&mut combined.text, &segment.text);
        crate::streaming::append_term_matches(
            &mut combined.activation_matches,
            &segment.activation_matches,
            position,
        );
        crate::streaming::append_term_matches(
            &mut combined.context_matches,
            &segment.context_matches,
            position,
        );
        if !segment.speaker_id.is_empty() {
            combined.speaker_id = segment.speaker_id;
        }
        combined.source_start_ms = combined.source_start_ms.min(segment.source_start_ms);
        combined.source_end_ms = combined.source_end_ms.max(segment.source_end_ms);
    }
    Some(combined)
}

pub(crate) fn collect_authoritative_recognition_snapshot(
    pending: &mut Vec<PendingAuthoritativeRecognition>,
    stream_id: u64,
    revision_id: u64,
    segment_index: u32,
    segment_count: u32,
    entry: RecognitionHistoryEntry,
) -> Option<Vec<RecognitionHistoryEntry>> {
    if segment_count == 0 || segment_index == 0 || segment_index > segment_count {
        return None;
    }
    let index = pending
        .iter()
        .position(|snapshot| {
            snapshot.stream_id == stream_id
                && snapshot.turn_id == entry.turn_id
                && snapshot.revision_id == revision_id
        })
        .unwrap_or_else(|| {
            if pending.len() >= 32 {
                pending.remove(0);
            }
            pending.push(PendingAuthoritativeRecognition {
                stream_id,
                turn_id: entry.turn_id.clone(),
                revision_id,
                segment_count,
                segments: Vec::new(),
            });
            pending.len() - 1
        });
    let snapshot = &mut pending[index];
    if snapshot.segment_count != segment_count {
        pending.remove(index);
        return None;
    }
    if let Some((_, existing)) = snapshot
        .segments
        .iter_mut()
        .find(|(index, _)| *index == segment_index)
    {
        *existing = entry;
    } else {
        snapshot.segments.push((segment_index, entry));
    }
    if snapshot.segments.len() != snapshot.segment_count as usize
        || !(1..=snapshot.segment_count).all(|expected| {
            snapshot
                .segments
                .iter()
                .any(|(index, _)| *index == expected)
        })
    {
        return None;
    }
    let mut snapshot = pending.remove(index);
    snapshot.segments.sort_by_key(|(index, _)| *index);
    Some(
        snapshot
            .segments
            .into_iter()
            .map(|(_, entry)| entry)
            .collect(),
    )
}

pub(crate) fn merge_authoritative_recognition_snapshot(
    history: &mut Vec<RecognitionHistoryEntry>,
    stream_id: u64,
    mut entries: Vec<RecognitionHistoryEntry>,
) -> bool {
    if entries.is_empty() {
        return false;
    }
    // A snapshot replaces one logical turn. Other completed turns from the
    // same audio stream must remain in history.
    let turn_id = entries[0].turn_id.clone();
    let revision = entries[0].revision_id;
    if entries
        .iter()
        .any(|entry| entry.turn_id != turn_id || entry.revision_id != revision)
    {
        return false;
    }
    let same_turn = |entry: &RecognitionHistoryEntry| {
        entry.stream_id == Some(stream_id) && entry.turn_id == turn_id
    };
    let previous_revision = history
        .iter()
        .filter(|entry| same_turn(entry) && entry.authoritative_snapshot)
        .map(|entry| entry.revision_id)
        .max();
    let finalizes_preview = !entries.iter().any(|entry| entry.revisable)
        && history.iter().any(|entry| {
            same_turn(entry)
                && entry.authoritative_snapshot
                && entry.revision_id == revision
                && entry.revisable
        });
    if previous_revision
        .is_some_and(|previous| previous > revision || (previous == revision && !finalizes_preview))
    {
        return false;
    }
    let insertion_index = history
        .iter()
        .position(|entry| same_turn(entry) && (entry.authoritative_snapshot || entry.live))
        .unwrap_or(history.len());
    history.retain(|entry| !(same_turn(entry) && (entry.authoritative_snapshot || entry.live)));
    for entry in &mut entries {
        entry.stream_id = Some(stream_id);
        // Draining a buffered sentence finalizes the same recognition revision;
        // it must retire the live row instead of requiring another ASR call.
        entry.live = entry.revisable;
        entry.revision = None;
    }
    history.splice(insertion_index..insertion_index, entries);
    true
}

pub(crate) fn merge_stream_recognition(
    history: &mut Vec<RecognitionHistoryEntry>,
    stream_id: u64,
    mut fragment: RecognitionHistoryEntry,
) {
    retain_recognition_tail(&mut fragment);
    if fragment.authoritative_snapshot {
        merge_authoritative_recognition_snapshot(history, stream_id, vec![fragment]);
        return;
    }
    let Some(current) = history
        .iter_mut()
        .rfind(|entry| entry.stream_id == Some(stream_id) && entry.live)
    else {
        initialize_recognition_revision(&mut fragment);
        history.push(fragment);
        return;
    };

    let stable = current
        .revision
        .as_ref()
        .map(crate::streaming::RevisableText::stable_text)
        .filter(|text| !text.is_empty())
        .unwrap_or(&current.text);
    if crate::streaming::should_roll_caption(
        stable,
        current.source_start_ms,
        fragment.source_end_ms,
    ) {
        let handoff = current.revision.as_ref().map_or_else(
            || {
                crate::streaming::handoff_text(
                    &current.text,
                    &fragment.text,
                    fragment.overlap_ratio,
                )
            },
            |revision| revision.handoff(&fragment.text, fragment.overlap_ratio),
        );
        if !handoff.text.trim().is_empty() {
            current.live = false;
            fragment.text = handoff.text;
            fragment.activation_matches =
                trimmed_term_matches(&fragment.activation_matches, handoff.source_start);
            fragment.context_matches =
                trimmed_term_matches(&fragment.context_matches, handoff.source_start);
            initialize_recognition_revision(&mut fragment);
            history.push(fragment);
            return;
        }
    }

    if fragment.revisable {
        let update = current
            .revision
            .get_or_insert_with(|| crate::streaming::RevisableText::new(&current.text))
            .update(&fragment.text, fragment.overlap_ratio);
        merge_revision_matches(
            &mut current.activation_matches,
            &fragment.activation_matches,
            update.hypothesis_start,
        );
        merge_revision_matches(
            &mut current.context_matches,
            &fragment.context_matches,
            update.hypothesis_start,
        );
        current.text = update.text;
    } else {
        let position = crate::streaming::append_text(&mut current.text, &fragment.text);
        crate::streaming::append_term_matches(
            &mut current.activation_matches,
            &fragment.activation_matches,
            position,
        );
        crate::streaming::append_term_matches(
            &mut current.context_matches,
            &fragment.context_matches,
            position,
        );
    }
    if !fragment.speaker_id.is_empty() {
        current.speaker_id = fragment.speaker_id;
    }
    current.timing = xrtranslate_protocol::SegmentTiming::MergedWindows;
    current.boundary = fragment.boundary;
    current.source_start_ms = current.source_start_ms.min(fragment.source_start_ms);
    current.source_end_ms = current.source_end_ms.max(fragment.source_end_ms);
    retain_recognition_tail(current);
}

fn initialize_recognition_revision(entry: &mut RecognitionHistoryEntry) {
    if entry.revisable {
        entry.revision = Some(crate::streaming::RevisableText::new(&entry.text));
    }
}

pub(crate) fn merge_stream_translation(
    history: &mut Vec<TranslationHistoryEntry>,
    stream_id: u64,
    mut fragment: TranslationHistoryEntry,
) -> StreamMerge {
    crate::streaming::retain_tail(&mut fragment.source, None, STREAM_TEXT_LIMIT);
    crate::streaming::retain_tail(
        &mut fragment.translated,
        Some(&mut fragment.term_matches),
        STREAM_TEXT_LIMIT,
    );
    if fragment.authoritative_snapshot {
        if let Some(newest) = history
            .iter()
            .filter(|entry| entry.stream_id == Some(stream_id) && entry.authoritative_snapshot)
            .max_by_key(|entry| entry.revision_id)
            && newest.revision_id >= fragment.revision_id
        {
            return StreamMerge {
                entry: newest.clone(),
                rolled_over: false,
                changed: false,
            };
        }
        if let Some(current) = history
            .iter_mut()
            .rfind(|entry| entry.stream_id == Some(stream_id) && entry.live)
        {
            let source_start_ms = current.source_start_ms.min(fragment.source_start_ms);
            let changed = current.source != fragment.source
                || current.translated != fragment.translated
                || current.additional_translations != fragment.additional_translations
                || current.asr_only != fragment.asr_only
                || current.term_matches != fragment.term_matches;
            *current = fragment;
            current.source_start_ms = source_start_ms;
            current.timing = xrtranslate_protocol::SegmentTiming::MergedWindows;
            return StreamMerge {
                entry: current.clone(),
                rolled_over: false,
                changed,
            };
        }
        history.push(fragment.clone());
        return StreamMerge {
            entry: fragment,
            rolled_over: false,
            changed: true,
        };
    }
    let Some(current) = history
        .iter_mut()
        .rfind(|entry| entry.stream_id == Some(stream_id) && entry.live)
    else {
        initialize_revision(&mut fragment);
        history.push(fragment.clone());
        return StreamMerge {
            entry: fragment,
            rolled_over: false,
            changed: true,
        };
    };

    if current.asr_only != fragment.asr_only
        || current
            .additional_translations
            .iter()
            .map(|entry| &entry.target_lang)
            .ne(fragment
                .additional_translations
                .iter()
                .map(|entry| &entry.target_lang))
    {
        current.live = false;
        initialize_revision(&mut fragment);
        history.push(fragment.clone());
        return StreamMerge {
            entry: fragment,
            rolled_over: true,
            changed: true,
        };
    }

    let stable_source = current
        .source_revision
        .as_ref()
        .map(crate::streaming::RevisableText::stable_text)
        .filter(|text| !text.is_empty())
        .unwrap_or(&current.source);
    if crate::streaming::should_roll_caption(
        stable_source,
        current.source_start_ms,
        fragment.source_end_ms,
    ) {
        let source = current.source_revision.as_ref().map_or_else(
            || {
                crate::streaming::handoff_text(
                    &current.source,
                    &fragment.source,
                    fragment.overlap_ratio,
                )
            },
            |revision| revision.handoff(&fragment.source, fragment.overlap_ratio),
        );
        if !source.text.trim().is_empty() {
            let translated = current.translated_revision.as_ref().map_or_else(
                || {
                    crate::streaming::handoff_text(
                        &current.translated,
                        &fragment.translated,
                        fragment.overlap_ratio,
                    )
                },
                |revision| revision.handoff(&fragment.translated, fragment.overlap_ratio),
            );
            current.live = false;
            fragment.source = source.text;
            fragment.translated = translated.text;
            fragment.term_matches =
                trimmed_term_matches(&fragment.term_matches, translated.source_start);
            for extra in &mut fragment.additional_translations {
                if let Some(previous) = current
                    .additional_translations
                    .iter()
                    .find(|old| old.target_lang == extra.target_lang)
                {
                    let handoff = crate::streaming::handoff_text(
                        &previous.translated_text,
                        &extra.translated_text,
                        fragment.overlap_ratio,
                    );
                    extra.translated_text = handoff.text;
                    extra.term_matches =
                        trimmed_term_matches(&extra.term_matches, handoff.source_start);
                }
            }
            initialize_revision(&mut fragment);
            history.push(fragment.clone());
            return StreamMerge {
                entry: fragment,
                rolled_over: true,
                changed: true,
            };
        }
    }

    let (source_changed, translated_changed) = if fragment.revisable {
        let old_source = current.source.clone();
        let old_translated = current.translated.clone();
        let source = current
            .source_revision
            .get_or_insert_with(|| crate::streaming::RevisableText::new(&current.source))
            .update(&fragment.source, fragment.overlap_ratio);
        let translated = current
            .translated_revision
            .get_or_insert_with(|| crate::streaming::RevisableText::new(&current.translated))
            .update(&fragment.translated, fragment.overlap_ratio);
        current.source = source.text;
        current.translated = translated.text;
        merge_revision_matches(
            &mut current.term_matches,
            &fragment.term_matches,
            translated.hypothesis_start,
        );
        (
            current.source != old_source,
            current.translated != old_translated,
        )
    } else {
        let source_changed =
            crate::streaming::append_text(&mut current.source, &fragment.source).is_some();
        let translated_offset =
            crate::streaming::append_text(&mut current.translated, &fragment.translated);
        let translated_changed = translated_offset.is_some();
        crate::streaming::append_term_matches(
            &mut current.term_matches,
            &fragment.term_matches,
            translated_offset,
        );
        (source_changed, translated_changed)
    };
    let additional_changed = merge_additional_translations(current, &fragment);
    crate::streaming::retain_tail(&mut current.source, None, STREAM_TEXT_LIMIT);
    crate::streaming::retain_tail(
        &mut current.translated,
        Some(&mut current.term_matches),
        STREAM_TEXT_LIMIT,
    );
    if !fragment.speaker_id.is_empty() {
        current.speaker_id = fragment.speaker_id;
    }
    current.timing = xrtranslate_protocol::SegmentTiming::MergedWindows;
    current.boundary = fragment.boundary;
    current.source_start_ms = current.source_start_ms.min(fragment.source_start_ms);
    current.source_end_ms = current.source_end_ms.max(fragment.source_end_ms);
    StreamMerge {
        entry: current.clone(),
        rolled_over: false,
        changed: source_changed || translated_changed || additional_changed,
    }
}

fn merge_additional_translations(
    current: &mut TranslationHistoryEntry,
    fragment: &TranslationHistoryEntry,
) -> bool {
    let mut changed = false;
    for incoming in &fragment.additional_translations {
        let Some(existing) = current
            .additional_translations
            .iter_mut()
            .find(|old| old.target_lang == incoming.target_lang)
        else {
            continue;
        };
        let old = existing.clone();
        if fragment.revisable {
            let revision = current
                .additional_revisions
                .entry(incoming.target_lang.clone())
                .or_insert_with(|| crate::streaming::RevisableText::new(&existing.translated_text))
                .update(&incoming.translated_text, fragment.overlap_ratio);
            existing.translated_text = revision.text;
            merge_revision_matches(
                &mut existing.term_matches,
                &incoming.term_matches,
                revision.hypothesis_start,
            );
        } else {
            let offset = crate::streaming::append_text(
                &mut existing.translated_text,
                &incoming.translated_text,
            );
            crate::streaming::append_term_matches(
                &mut existing.term_matches,
                &incoming.term_matches,
                offset,
            );
        }
        crate::streaming::retain_tail(
            &mut existing.translated_text,
            Some(&mut existing.term_matches),
            STREAM_TEXT_LIMIT,
        );
        changed |= *existing != old;
    }
    changed
}

pub(crate) fn collect_authoritative_translation_snapshot(
    pending: &mut Vec<PendingAuthoritativeTranslation>,
    stream_id: u64,
    revision_id: u64,
    segment_index: u32,
    segment_count: u32,
    entry: TranslationHistoryEntry,
) -> Option<Vec<TranslationHistoryEntry>> {
    if segment_count == 0 || segment_index == 0 || segment_index > segment_count {
        return None;
    }
    let index = pending
        .iter()
        .position(|snapshot| snapshot.stream_id == stream_id && snapshot.revision_id == revision_id)
        .unwrap_or_else(|| {
            if pending.len() >= 32 {
                pending.remove(0);
            }
            pending.push(PendingAuthoritativeTranslation {
                stream_id,
                revision_id,
                segment_count,
                segments: Vec::new(),
            });
            pending.len() - 1
        });
    let snapshot = &mut pending[index];
    if snapshot.segment_count != segment_count {
        pending.remove(index);
        return None;
    }
    if let Some((_, existing)) = snapshot
        .segments
        .iter_mut()
        .find(|(index, _)| *index == segment_index)
    {
        *existing = entry;
    } else {
        snapshot.segments.push((segment_index, entry));
    }
    if snapshot.segments.len() != snapshot.segment_count as usize
        || !(1..=snapshot.segment_count).all(|expected| {
            snapshot
                .segments
                .iter()
                .any(|(index, _)| *index == expected)
        })
    {
        return None;
    }
    let mut snapshot = pending.remove(index);
    snapshot.segments.sort_by_key(|(index, _)| *index);
    Some(
        snapshot
            .segments
            .into_iter()
            .map(|(_, entry)| entry)
            .collect(),
    )
}

pub(crate) fn merge_authoritative_translation_snapshot(
    history: &mut Vec<TranslationHistoryEntry>,
    stream_id: u64,
    mut entries: Vec<TranslationHistoryEntry>,
) -> AuthoritativeTranslationMerge {
    if entries.is_empty() {
        return AuthoritativeTranslationMerge {
            accepted: false,
            stabilized: Vec::new(),
            live: None,
            changed: false,
        };
    }
    entries.sort_by_key(|entry| entry.segment_index);
    let revision = entries[0].revision_id;
    if let Some(newest) = history
        .iter()
        .filter(|entry| entry.stream_id == Some(stream_id) && entry.authoritative_snapshot)
        .max_by_key(|entry| entry.revision_id)
        && newest.revision_id >= revision
    {
        return AuthoritativeTranslationMerge {
            accepted: false,
            live: history
                .iter()
                .rfind(|entry| entry.stream_id == Some(stream_id) && entry.live)
                .cloned(),
            stabilized: Vec::new(),
            changed: false,
        };
    }

    let old_entries = history
        .iter()
        .filter(|entry| {
            entry.stream_id == Some(stream_id) && (entry.authoritative_snapshot || entry.live)
        })
        .cloned()
        .collect::<Vec<_>>();
    let old_by_index = |index: u32| {
        old_entries
            .iter()
            .find(|entry| entry.segment_index == index)
    };
    let mut stabilized = Vec::new();
    let mut changed = old_entries.len() != entries.len();
    let entry_count = entries.len();
    for (index, entry) in entries.iter_mut().enumerate() {
        entry.stream_id = Some(stream_id);
        entry.live = index + 1 == entry_count;
        let old = old_by_index(entry.segment_index);
        let content_changed = old.is_none_or(|old| {
            let mut old = old.clone();
            old.live = entry.live;
            // Revision IDs identify snapshots, but are not visible caption
            // content. Ignore them here so an unchanged tail does not cause
            // an OSC Replace on every ASR revision.
            old.revision_id = entry.revision_id;
            old != *entry
        });
        changed |= content_changed;
        if !entry.live && (old.is_none() || old.is_some_and(|old| old.live)) {
            stabilized.push(entry.clone());
        }
    }
    history.retain(|entry| {
        !(entry.stream_id == Some(stream_id) && (entry.authoritative_snapshot || entry.live))
    });
    history.extend(entries.iter().cloned());
    let live = entries.last().cloned();
    AuthoritativeTranslationMerge {
        accepted: true,
        stabilized,
        live,
        changed,
    }
}

/// Inserts a completed, non-streaming translation without conflating separate
/// segments from the same backend turn. Stable backend identity wins over the
/// legacy text/time fallback used by older event payloads without a turn ID.
pub(crate) fn upsert_completed_translation(
    history: &mut Vec<TranslationHistoryEntry>,
    fragment: TranslationHistoryEntry,
) {
    if !fragment.turn_id.is_empty() {
        if let Some(existing) = history.iter_mut().rfind(|entry| {
            entry.stream_id == fragment.stream_id
                && entry.audio_source == fragment.audio_source
                && entry.turn_id == fragment.turn_id
                && entry.segment_index == fragment.segment_index
        }) {
            *existing = fragment;
        } else {
            history.push(fragment);
        }
        return;
    }

    if let Some(last) = history.last_mut()
        && last.stream_id == fragment.stream_id
        && last.turn_id.is_empty()
        && last.source == fragment.source
        && (last.source_start_ms - fragment.source_start_ms).abs() <= 2500.0
    {
        *last = fragment;
    } else {
        history.push(fragment);
    }
}

fn initialize_revision(entry: &mut TranslationHistoryEntry) {
    if entry.revisable {
        entry.source_revision = Some(crate::streaming::RevisableText::new(&entry.source));
        entry.translated_revision = Some(crate::streaming::RevisableText::new(&entry.translated));
        entry.additional_revisions = entry
            .additional_translations
            .iter()
            .map(|extra| {
                (
                    extra.target_lang.clone(),
                    crate::streaming::RevisableText::new(&extra.translated_text),
                )
            })
            .collect();
    }
}

fn shifted_term_matches(
    matches: &[xrtranslate_protocol::CorpusTermMatch],
    offset: usize,
) -> Vec<xrtranslate_protocol::CorpusTermMatch> {
    let Ok(offset) = u32::try_from(offset) else {
        return Vec::new();
    };
    matches
        .iter()
        .cloned()
        .filter_map(|mut term| {
            term.start_byte = term.start_byte.checked_add(offset)?;
            term.end_byte = term.end_byte.checked_add(offset)?;
            Some(term)
        })
        .collect()
}

fn trimmed_term_matches(
    matches: &[xrtranslate_protocol::CorpusTermMatch],
    source_start: usize,
) -> Vec<xrtranslate_protocol::CorpusTermMatch> {
    let Ok(source_start) = u32::try_from(source_start) else {
        return Vec::new();
    };
    matches
        .iter()
        .cloned()
        .filter_map(|mut term| {
            if term.start_byte < source_start {
                return None;
            }
            term.start_byte = term.start_byte.checked_sub(source_start)?;
            term.end_byte = term.end_byte.checked_sub(source_start)?;
            Some(term)
        })
        .collect()
}

fn merge_revision_matches(
    current: &mut Vec<xrtranslate_protocol::CorpusTermMatch>,
    incoming: &[xrtranslate_protocol::CorpusTermMatch],
    hypothesis_start: usize,
) {
    let Ok(stable_end) = u32::try_from(hypothesis_start) else {
        current.clear();
        return;
    };
    current.retain(|term| term.end_byte <= stable_end);
    current.extend(shifted_term_matches(incoming, hypothesis_start));
}

fn retain_recognition_tail(entry: &mut RecognitionHistoryEntry) {
    let original_len = entry.text.len();
    crate::streaming::retain_tail(
        &mut entry.text,
        Some(&mut entry.activation_matches),
        STREAM_TEXT_LIMIT,
    );
    let removed = original_len.saturating_sub(entry.text.len());
    if removed > 0 {
        entry.context_matches = trimmed_term_matches(&entry.context_matches, removed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client_settings::CaptureSource;

    fn fragment(stream_id: u64, source: &str, translated: &str) -> TranslationHistoryEntry {
        TranslationHistoryEntry {
            turn_id: String::new(),
            segment_index: 0,
            stream_id: Some(stream_id),
            audio_source: CaptureSource::Microphone,
            live: true,
            source: source.into(),
            translated: translated.into(),
            additional_translations: Vec::new(),
            asr_only: false,
            speaker_id: String::new(),
            source_start_ms: 0.0,
            source_end_ms: 1.0,
            timing: xrtranslate_protocol::SegmentTiming::UtteranceWindow,
            boundary: xrtranslate_protocol::SegmentBoundary::Silence,
            term_matches: Vec::new(),
            revisable: false,
            overlap_ratio: 0.0,
            authoritative_snapshot: false,
            revision_id: 0,
            source_revision: None,
            translated_revision: None,
            additional_revisions: Default::default(),
        }
    }

    fn snapshot(stream_id: u64, source: &str, translated: &str) -> TranslationHistoryEntry {
        TranslationHistoryEntry {
            revisable: true,
            overlap_ratio: 0.34,
            ..fragment(stream_id, source, translated)
        }
    }

    fn recognition_snapshot(stream_id: u64, turn_id: &str, text: &str) -> RecognitionHistoryEntry {
        RecognitionHistoryEntry {
            stream_id: Some(stream_id),
            live: true,
            text: text.into(),
            turn_id: turn_id.into(),
            speaker_id: String::new(),
            source_start_ms: 0.0,
            source_end_ms: 1_000.0,
            timing: xrtranslate_protocol::SegmentTiming::UtteranceWindow,
            boundary: xrtranslate_protocol::SegmentBoundary::DurationLimit,
            activation_matches: Vec::new(),
            context_matches: Vec::new(),
            revisable: true,
            overlap_ratio: 0.34,
            authoritative_snapshot: false,
            revision_id: 0,
            revision: None,
        }
    }

    fn authoritative_source(
        stream_id: u64,
        turn_id: &str,
        text: &str,
        revision: u64,
        revisable: bool,
    ) -> RecognitionHistoryEntry {
        RecognitionHistoryEntry {
            authoritative_snapshot: true,
            revision_id: revision,
            revisable,
            ..recognition_snapshot(stream_id, turn_id, text)
        }
    }

    #[test]
    fn authoritative_source_finalizes_same_revision_and_keeps_other_turns() {
        let mut history = Vec::new();
        let preview = authoritative_source(7, "turn-a", "The contract", 9, true);
        assert!(merge_authoritative_recognition_snapshot(
            &mut history,
            7,
            vec![preview.clone()],
        ));
        assert!(history[0].live);

        // A drain finalizes the retained text without a new recognition pass.
        let final_source = authoritative_source(7, "turn-a", "The contract", 9, false);
        assert!(merge_authoritative_recognition_snapshot(
            &mut history,
            7,
            vec![final_source.clone()],
        ));
        assert_eq!(history.len(), 1);
        assert!(!history[0].live);
        assert!(!history[0].revisable);
        assert!(!merge_authoritative_recognition_snapshot(
            &mut history,
            7,
            vec![final_source],
        ));
        assert!(!merge_authoritative_recognition_snapshot(
            &mut history,
            7,
            vec![preview],
        ));

        // Revisions belong to a turn, so another turn may begin at a smaller
        // revision while retaining the prior turn's completed source.
        assert!(merge_authoritative_recognition_snapshot(
            &mut history,
            7,
            vec![authoritative_source(7, "turn-b", "Next speaker", 1, true)],
        ));
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].text, "The contract");
        assert_eq!(history[1].text, "Next speaker");
        assert!(!history[0].live);
        assert!(history[1].live);
        assert!(!merge_authoritative_recognition_snapshot(
            &mut history,
            7,
            vec![authoritative_source(7, "turn-a", "Stale source", 8, false)],
        ));
        assert_eq!(history[0].text, "The contract");
    }

    #[test]
    fn authoritative_source_replaces_only_its_turn_and_preserves_sentence_stability() {
        let mut history = vec![recognition_snapshot(8, "other-stream", "Other audio")];
        assert!(merge_authoritative_recognition_snapshot(
            &mut history,
            7,
            vec![
                authoritative_source(7, "turn-a", "Ready.", 1, false),
                authoritative_source(7, "turn-a", "The contract", 1, true),
            ],
        ));
        assert!(!history[1].live);
        assert!(history[2].live);
        assert!(merge_authoritative_recognition_snapshot(
            &mut history,
            7,
            vec![authoritative_source(7, "turn-b", "A later turn.", 2, false)],
        ));
        assert!(merge_authoritative_recognition_snapshot(
            &mut history,
            7,
            vec![
                authoritative_source(7, "turn-a", "Ready.", 2, false),
                authoritative_source(7, "turn-a", "The contract is ready.", 2, false),
            ],
        ));
        assert_eq!(history.len(), 4);
        assert_eq!(history[0].text, "Other audio");
        assert_eq!(history[1].text, "Ready.");
        assert_eq!(history[2].text, "The contract is ready.");
        assert_eq!(history[3].text, "A later turn.");
        assert!(
            history[1..]
                .iter()
                .all(|entry| !entry.live && !entry.revisable)
        );
    }

    #[test]
    fn authoritative_source_collector_keeps_interleaved_turns_separate() {
        let mut pending = Vec::new();
        for (turn, text) in [("turn-a", "A first."), ("turn-b", "B first.")] {
            assert!(
                collect_authoritative_recognition_snapshot(
                    &mut pending,
                    7,
                    3,
                    1,
                    2,
                    authoritative_source(7, turn, text, 3, false),
                )
                .is_none()
            );
        }
        let a = collect_authoritative_recognition_snapshot(
            &mut pending,
            7,
            3,
            2,
            2,
            authoritative_source(7, "turn-a", "A last.", 3, false),
        )
        .unwrap();
        let b = collect_authoritative_recognition_snapshot(
            &mut pending,
            7,
            3,
            2,
            2,
            authoritative_source(7, "turn-b", "B last.", 3, false),
        )
        .unwrap();
        assert_eq!(
            a.iter()
                .map(|entry| entry.text.as_str())
                .collect::<Vec<_>>(),
            ["A first.", "A last."]
        );
        assert_eq!(
            b.iter()
                .map(|entry| entry.text.as_str())
                .collect::<Vec<_>>(),
            ["B first.", "B last."]
        );
        assert!(pending.is_empty());
    }

    #[test]
    fn streaming_translation_updates_each_audio_source_in_place() {
        let mut history = Vec::new();
        merge_stream_translation(&mut history, 1, fragment(1, "Hello", "你好"));
        merge_stream_translation(&mut history, 2, fragment(2, "Music", "音乐"));
        let microphone =
            merge_stream_translation(&mut history, 1, fragment(1, "world", "你好世界"));

        assert_eq!(history.len(), 2);
        assert_eq!(microphone.entry.source, "Hello world");
        assert_eq!(microphone.entry.translated, "你好世界");
        assert_eq!(history[1].source, "Music");
    }

    #[test]
    fn completed_translation_keeps_every_segment_in_one_turn() {
        let mut history = Vec::new();
        let mut first = fragment(7, "First sentence.", "第一句。");
        first.stream_id = None;
        first.live = false;
        first.turn_id = "turn-7".into();
        first.segment_index = 1;
        let mut second = fragment(7, "Second sentence.", "第二句。");
        second.stream_id = None;
        second.live = false;
        second.turn_id = "turn-7".into();
        second.segment_index = 2;

        upsert_completed_translation(&mut history, first);
        upsert_completed_translation(&mut history, second);

        assert_eq!(history.len(), 2);
        assert_eq!(history[0].source, "First sentence.");
        assert_eq!(history[1].source, "Second sentence.");
    }

    #[test]
    fn completed_translation_revises_only_its_matching_segment() {
        let mut history = Vec::new();
        let mut first = fragment(7, "First", "第一");
        first.stream_id = None;
        first.live = false;
        first.turn_id = "turn-7".into();
        first.segment_index = 1;
        let mut second = fragment(7, "Second", "第二");
        second.stream_id = None;
        second.live = false;
        second.turn_id = "turn-7".into();
        second.segment_index = 2;
        upsert_completed_translation(&mut history, first.clone());
        upsert_completed_translation(&mut history, second);

        first.source = "First revised".into();
        upsert_completed_translation(&mut history, first);

        assert_eq!(history.len(), 2);
        assert_eq!(history[0].source, "First revised");
        assert_eq!(history[1].source, "Second");
    }

    #[test]
    fn completed_translations_with_matching_turn_ids_stay_in_their_own_sessions() {
        let mut history = Vec::new();
        let mut a = fragment(1, "Meeting", "会议");
        let mut b = fragment(2, "Video", "视频");
        a.turn_id = "native-1".into();
        b.turn_id = "native-1".into();
        a.live = false;
        b.live = false;
        upsert_completed_translation(&mut history, a.clone());
        upsert_completed_translation(&mut history, b);
        assert_eq!(history.len(), 2);
        a.translated = "更正".into();
        upsert_completed_translation(&mut history, a);
        assert_eq!(history.len(), 2);
        assert_eq!(history[1].source, "Video");
    }

    #[test]
    fn authoritative_snapshot_waits_for_all_segments_and_removes_old_tail() {
        let mut pending = Vec::new();
        let mut first = snapshot(3, "First sentence.", "第一句。");
        first.authoritative_snapshot = true;
        first.revision_id = 1;
        first.segment_index = 1;
        assert!(
            collect_authoritative_translation_snapshot(&mut pending, 3, 1, 1, 2, first,).is_none()
        );

        let mut second = snapshot(3, "Live tail", "实时尾句");
        second.authoritative_snapshot = true;
        second.revision_id = 1;
        second.segment_index = 2;
        let entries =
            collect_authoritative_translation_snapshot(&mut pending, 3, 1, 2, 2, second).unwrap();
        let mut history = Vec::new();
        let merged = merge_authoritative_translation_snapshot(&mut history, 3, entries);
        assert!(merged.changed);
        assert_eq!(merged.stabilized.len(), 1);
        assert_eq!(history.len(), 2);
        assert!(!history[0].live);
        assert!(history[1].live);

        let mut corrected = snapshot(3, "Revised complete sentence.", "修订后的完整句。");
        corrected.authoritative_snapshot = true;
        corrected.revision_id = 2;
        corrected.segment_index = 1;
        let entries =
            collect_authoritative_translation_snapshot(&mut pending, 3, 2, 1, 1, corrected)
                .unwrap();
        let merged = merge_authoritative_translation_snapshot(&mut history, 3, entries);
        assert!(merged.changed);
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].source, "Revised complete sentence.");
        assert!(history[0].live);
    }

    #[test]
    fn unchanged_authoritative_content_does_not_repeat_live_caption_replace() {
        let mut first = snapshot(3, "Stable sentence.", "稳定句。");
        first.authoritative_snapshot = true;
        first.revision_id = 1;
        first.segment_index = 1;
        let mut history = Vec::new();
        let merged = merge_authoritative_translation_snapshot(&mut history, 3, vec![first]);
        assert!(merged.changed);

        let mut repeat = snapshot(3, "Stable sentence.", "稳定句。");
        repeat.authoritative_snapshot = true;
        repeat.revision_id = 2;
        repeat.segment_index = 1;
        let merged = merge_authoritative_translation_snapshot(&mut history, 3, vec![repeat]);
        assert!(merged.accepted);
        assert!(!merged.changed);
        assert!(merged.stabilized.is_empty());
    }

    #[test]
    fn streaming_translation_rolls_a_finished_caption_into_history() {
        let mut history = Vec::new();
        let mut first = fragment(
            1,
            "This is a complete first sentence with enough stable words to roll cleanly.",
            "这是完整的第一句。",
        );
        first.source_end_ms = 4_000.0;
        merge_stream_translation(&mut history, 1, first);
        let mut next = fragment(1, "Next", "下一句");
        next.source_start_ms = 4_000.0;
        next.source_end_ms = 5_000.0;
        let update = merge_stream_translation(&mut history, 1, next);

        assert!(update.rolled_over);
        assert_eq!(history.len(), 2);
        assert!(!history[0].live);
        assert!(history[1].live);
    }

    #[test]
    fn revisable_windows_replace_the_unstable_tail() {
        let mut history = Vec::new();
        merge_stream_translation(
            &mut history,
            1,
            snapshot(1, "we walk across the central street", "我们走过中央大街"),
        );
        let update = merge_stream_translation(
            &mut history,
            1,
            snapshot(1, "the central station and turn left", "中央车站然后左转"),
        );

        assert_eq!(
            update.entry.source,
            "we walk across the central station and turn left"
        );
        assert_eq!(update.entry.translated, "我们走过中央车站然后左转");
        assert_eq!(history.len(), 1);
    }

    #[test]
    fn provisional_window_punctuation_does_not_roll_a_live_caption() {
        let mut history = Vec::new();
        let mut first = snapshot(1, "A short provisional sentence.", "一个临时短句。");
        first.source_end_ms = 2_000.0;
        merge_stream_translation(&mut history, 1, first);

        let mut next = snapshot(1, "provisional sentence continues here", "临时短句仍在继续");
        next.source_start_ms = 1_000.0;
        next.source_end_ms = 5_000.0;
        let update = merge_stream_translation(&mut history, 1, next);

        assert!(!update.rolled_over);
        assert_eq!(history.len(), 1);
    }

    #[test]
    fn caption_rollover_consumes_the_shared_window_prefix() {
        let mut history = Vec::new();
        let mut first = snapshot(
            1,
            "I frown, but you use it to end the sentence with a period.",
            "I frown, but you use it to end the sentence with a period.",
        );
        first.source_end_ms = 4_000.0;
        merge_stream_translation(&mut history, 1, first);

        let mut next = snapshot(
            1,
            "use it to end the sentence with a period. I can only say I admit it.",
            "use it to end the sentence with a period. I can only say I admit it.",
        );
        next.source_start_ms = 2_500.0;
        next.source_end_ms = 5_500.0;
        let update = merge_stream_translation(&mut history, 1, next);

        assert!(update.rolled_over);
        assert_eq!(history.len(), 2);
        assert_eq!(history[1].source, "I can only say I admit it.");
        assert_eq!(history[1].translated, "I can only say I admit it.");
    }

    #[test]
    fn recognition_window_combines_all_segments_before_display() {
        let mut pending = Vec::<PendingRecognitionWindow>::new();
        let second = recognition_snapshot(7, "turn-1", "我的台词全念一遍。");
        let first = recognition_snapshot(7, "turn-1", "准备好的台词。");

        assert!(collect_recognition_window(&mut pending, 7, true, 2, 2, second).is_none());
        let combined = collect_recognition_window(&mut pending, 7, true, 1, 2, first).unwrap();

        assert_eq!(combined.text, "准备好的台词。我的台词全念一遍。");
        assert!(pending.is_empty());
    }

    #[test]
    fn recognition_window_rejects_invalid_or_conflicting_segment_plans() {
        let mut pending = Vec::<PendingRecognitionWindow>::new();
        assert!(
            collect_recognition_window(
                &mut pending,
                7,
                true,
                0,
                2,
                recognition_snapshot(7, "turn-invalid", "bad"),
            )
            .is_none()
        );
        assert!(pending.is_empty());

        assert!(
            collect_recognition_window(
                &mut pending,
                7,
                true,
                1,
                2,
                recognition_snapshot(7, "turn-conflict", "first"),
            )
            .is_none()
        );
        assert!(
            collect_recognition_window(
                &mut pending,
                7,
                true,
                2,
                3,
                recognition_snapshot(7, "turn-conflict", "second"),
            )
            .is_none()
        );
        assert!(pending.is_empty());
    }

    #[test]
    fn recognition_history_revises_the_shared_audio_window_in_place() {
        let mut history = Vec::new();
        merge_stream_recognition(
            &mut history,
            7,
            recognition_snapshot(7, "turn-1", "你停在了这条我们熟悉的街。"),
        );
        let mut next = recognition_snapshot(7, "turn-2", "熟悉的街。然后继续向前走。");
        next.source_start_ms = 1_000.0;
        next.source_end_ms = 3_000.0;
        merge_stream_recognition(&mut history, 7, next);

        assert_eq!(history.len(), 1);
        assert_eq!(
            history[0].text,
            "你停在了这条我们熟悉的街。然后继续向前走。"
        );
    }

    #[test]
    fn authoritative_translation_replaces_semantic_rewrites_atomically() {
        let mut history = Vec::new();
        let mut first = snapshot(
            7,
            "证明的过程中，AI只是利用强悍的数分高带技巧。",
            "During the proof, AI uses numerical tricks.",
        );
        first.authoritative_snapshot = true;
        first.revision_id = 4;
        merge_stream_translation(&mut history, 7, first);

        let mut corrected = snapshot(
            7,
            "证明的过程中，AI只是利用强悍和无比强大的算力。",
            "During the proof, AI simply relies on immense computing power.",
        );
        corrected.authoritative_snapshot = true;
        corrected.revision_id = 5;
        let update = merge_stream_translation(&mut history, 7, corrected);

        assert_eq!(history.len(), 1);
        assert_eq!(
            update.entry.translated,
            "During the proof, AI simply relies on immense computing power."
        );
    }

    #[test]
    fn stale_authoritative_translation_cannot_overwrite_a_newer_revision() {
        let mut history = Vec::new();
        let mut newest = snapshot(7, "correct source", "correct translation");
        newest.authoritative_snapshot = true;
        newest.revision_id = 9;
        merge_stream_translation(&mut history, 7, newest);

        let mut stale = snapshot(7, "stale source", "stale translation");
        stale.authoritative_snapshot = true;
        stale.revision_id = 8;
        let update = merge_stream_translation(&mut history, 7, stale);

        assert!(!update.changed);
        assert_eq!(history[0].source, "correct source");
        assert_eq!(history[0].translated, "correct translation");
    }

    #[test]
    fn additional_only_revisions_refresh_the_existing_caption_and_reject_stale_output() {
        let extra = |text: &str| xrtranslate_protocol::AdditionalTranslation {
            target_lang: "ja".into(),
            translated_text: text.into(),
            term_matches: Vec::new(),
        };
        let mut first = snapshot(7, "source", "main");
        first.authoritative_snapshot = true;
        first.revision_id = 1;
        first.additional_translations = vec![extra("first")];
        let mut history = Vec::new();
        merge_stream_translation(&mut history, 7, first.clone());
        let mut next = first.clone();
        next.revision_id = 2;
        next.additional_translations = vec![extra("corrected")];
        assert!(merge_stream_translation(&mut history, 7, next).changed);
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].output_text(), "main\nja · corrected");
        assert!(!merge_stream_translation(&mut history, 7, first).changed);
        assert_eq!(
            history[0].additional_translations[0].translated_text,
            "corrected"
        );
    }

    #[test]
    fn suppressed_additional_output_is_removed_from_captions_and_can_resume() {
        for authoritative in [false, true] {
            let mut history = Vec::new();
            let mut entry = if authoritative {
                snapshot(7, "source", "main")
            } else {
                fragment(7, "source", "main")
            };
            entry.authoritative_snapshot = authoritative;
            entry.revision_id = 1;
            entry.additional_translations = vec![xrtranslate_protocol::AdditionalTranslation {
                target_lang: "zh".into(),
                translated_text: "extra before overlap".into(),
                term_matches: Vec::new(),
            }];
            merge_stream_translation(&mut history, 7, entry.clone());

            entry.revision_id = 2;
            entry.translated = "main during overlap".into();
            entry.additional_translations.clear();
            let suppressed = merge_stream_translation(&mut history, 7, entry.clone());
            assert!(suppressed.changed);
            assert!(suppressed.entry.additional_translations.is_empty());
            assert_eq!(suppressed.entry.output_text(), "main during overlap");

            entry.revision_id = 3;
            entry.translated = "main after overlap".into();
            entry.additional_translations = vec![xrtranslate_protocol::AdditionalTranslation {
                target_lang: "zh".into(),
                translated_text: "extra after overlap".into(),
                term_matches: Vec::new(),
            }];
            let resumed = merge_stream_translation(&mut history, 7, entry);
            assert!(resumed.changed);
            assert_eq!(resumed.entry.additional_translations.len(), 1);
            assert_eq!(
                resumed.entry.output_text(),
                "main after overlap\nzh · extra after overlap"
            );
        }
    }

    #[test]
    fn language_modes_do_not_merge_asr_only_into_a_translated_caption() {
        let mut history = Vec::new();
        merge_stream_translation(&mut history, 7, fragment(7, "source", "main"));
        let mut next = fragment(7, "raw recognition", "corrected recognition");
        next.asr_only = true;
        let merged = merge_stream_translation(&mut history, 7, next);
        assert!(merged.rolled_over);
        assert!(merged.entry.asr_only);
        assert_eq!(merged.entry.translated, "corrected recognition");
        assert_eq!(history.len(), 2);
    }
}
