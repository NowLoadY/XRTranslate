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
