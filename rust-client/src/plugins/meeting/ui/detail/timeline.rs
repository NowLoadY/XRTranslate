use super::{super::actions::UiAction, rows};
use crate::plugins::meeting::{
    controller::MeetingController,
    i18n::tr,
    store::{MeetingBundle, Segment, Speaker},
};
use crate::ui::{components, virtual_list};
use eframe::egui;
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    sync::Arc,
};

#[derive(Clone, Copy)]
enum Entry {
    Topic(usize),
    Waiting(usize),
    Segment(usize),
    Empty,
}

struct Note {
    index: usize,
    text: Option<Arc<egui::Galley>>,
}

struct Row {
    id: egui::Id,
    entry: Entry,
    original: Arc<egui::Galley>,
    translated: Option<Arc<egui::Galley>>,
    updating: Option<Arc<egui::Galley>>,
    speaker: String,
    notes: Vec<Note>,
    show_notes: bool,
    header_height: f32,
}

pub(super) fn render_timeline(
    controller: &mut MeetingController,
    language: crate::i18n::UiLanguage,
    action: &mut UiAction,
    ui: &mut egui::Ui,
) {
    // Keep the controls in place when the first committed sentence arrives.
    components::search_bar(
        ui,
        &mut controller.search,
        tr(language, "Search this meeting"),
    );
    ui.horizontal_wrapped(|ui| {
        ui.selectable_value(
            &mut controller.highlights_only,
            false,
            tr(language, "All messages"),
        );
        ui.selectable_value(
            &mut controller.highlights_only,
            true,
            tr(language, "Highlights"),
        );
    });
    ui.add_space(8.0);

    let live = super::active_capture_visible(controller);
    let can_seek = controller.active_meeting_id().is_none();
    let can_merge = controller.can_merge_segments();
    let Some(bundle) = controller.bundle.as_ref() else {
        return;
    };
    let query = controller.search.trim().to_lowercase();
    let mut by_topic: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut by_segment: HashMap<&str, Vec<usize>> = HashMap::new();
    let speakers = bundle
        .speakers
        .iter()
        .map(|speaker| (speaker.id.as_str(), speaker))
        .collect::<HashMap<_, _>>();
    for (index, segment) in bundle.segments.iter().enumerate() {
        by_topic.entry(&segment.topic_id).or_default().push(index);
    }
    for (index, marker) in bundle.markers.iter().enumerate() {
        by_segment
            .entry(&marker.segment_id)
            .or_default()
            .push(index);
    }
    let mut entries = Vec::new();
    for (index, topic) in bundle.topics.iter().enumerate() {
        let topic_matches = topic
            .title
            .as_deref()
            .unwrap_or_default()
            .to_lowercase()
            .contains(&query);
        let segments = by_topic
            .get(topic.id.as_str())
            .into_iter()
            .flatten()
            .copied()
            .filter(|index| {
                let segment = &bundle.segments[*index];
                let markers = by_segment.get(segment.id.as_str());
                (!controller.highlights_only || markers.is_some())
                    && (query.is_empty()
                        || topic_matches
                        || segment_matches(segment, &query)
                        || markers.into_iter().flatten().any(|index| {
                            bundle.markers[*index].text.to_lowercase().contains(&query)
                        }))
            })
            .collect::<Vec<_>>();
        if segments.is_empty() && (!query.is_empty() || controller.highlights_only) {
            continue;
        }
        if topic.title.is_some() || bundle.topics.len() > 1 {
            entries.push(Entry::Topic(index));
        }
        if segments.is_empty() {
            entries.push(Entry::Waiting(index));
        }
        entries.extend(segments.into_iter().map(Entry::Segment));
    }
    if entries.is_empty() {
        entries.push(Entry::Empty);
    }
    let evidence = controller.evidence_target.as_deref().and_then(|target| {
        entries.iter().position(
            |entry| matches!(entry, Entry::Segment(index) if bundle.segments[*index].id == target),
        )
    });
    let active_id = controller
        .playback
        .current_segment(&bundle.segments)
        .map(|segment| segment.id.clone());
    let note_drafts = controller.marker_drafts.clone();
    let mut drafts_hash = std::collections::hash_map::DefaultHasher::new();
    for draft in &note_drafts {
        draft.hash(&mut drafts_hash);
    }
    let revision = egui::Id::new((
        &bundle.meeting.id,
        controller.bundle_revision(),
        &query,
        controller.highlights_only,
        drafts_hash.finish(),
    ));
    let measurements_id = egui::Id::new("meeting_timeline_measurements");
    let mut measurements = ui
        .data_mut(|data| data.remove_temp::<virtual_list::Measurements<Entry>>(measurements_id))
        .unwrap_or_default();
    let mut notes_changed = false;
    let follow = live && query.is_empty() && !controller.highlights_only;
    let empty_text = tr(
        language,
        if controller.highlights_only && query.is_empty() {
            "No highlights yet. Mark a message to find it here."
        } else {
            "No matching messages"
        },
    );
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        let scroll_id = ui.make_persistent_id(egui::IdSalt::new("meeting_timeline_scroll"));
        let geometry_id = scroll_id.with("geometry");
        let target = rows::scroll_target(ui, scroll_id, geometry_id, follow, evidence);
        let viewport = ui.available_rect_before_wrap();
        crate::ui::automation::record_output(
            ui,
            "Meeting transcript",
            crate::ui::automation::ElementValue::Number(entries.len() as f64),
            viewport,
        );
        let merged = virtual_list::show_subtitle_rows(
            ui,
            "meeting_timeline_scroll",
            6.0,
            target,
            |row: &virtual_list::MeasuredRow<Entry>| row.id,
            virtual_list::SubtitleMerge {
                enabled: can_merge,
                language,
                index: &|row: &virtual_list::MeasuredRow<Entry>| match row.entry {
                    Entry::Segment(index) => Some(index),
                    _ => None,
                },
                text: &|index| bundle.segments[index].original_text.as_str(),
            },
            |ui| {
                let prepared = measurements.prepare(
                    ui,
                    revision,
                    language,
                    || {
                        entries
                            .iter()
                            .map(|entry| {
                                row_geometry(
                                    ui,
                                    *entry,
                                    bundle,
                                    &by_segment,
                                    &note_drafts,
                                    empty_text,
                                )
                            })
                            .collect()
                    },
                    |row| {
                        prepare_row(
                            ui,
                            row.entry,
                            bundle,
                            &by_segment,
                            &speakers,
                            &note_drafts,
                            empty_text,
                            language,
                        )
                        .0
                    },
                );
                rows::remember_geometry(ui, geometry_id, prepared.as_ref(), 6.0, viewport.height());
                prepared
            },
            |ui, _, height, measured| {
                let (_, row) = prepare_row(
                    ui,
                    measured.entry,
                    bundle,
                    &by_segment,
                    &speakers,
                    &note_drafts,
                    empty_text,
                    language,
                );
                ui.push_id(row.id, |ui| {
                    if let Entry::Segment(index) = row.entry {
                        let segment = &bundle.segments[index];
                        notes_changed |= render_segment(
                            &row,
                            height,
                            bundle,
                            &mut controller.marker_drafts,
                            can_seek && controller.playback.can_seek_segment(segment),
                            can_merge,
                            active_id.as_deref() == Some(segment.id.as_str()),
                            language,
                            action,
                            ui,
                        );
                    } else {
                        ui.add(egui::Label::new(row.original.clone()).wrap());
                    }
                });
            },
        );
        if let Some((source, target)) = merged {
            *action = UiAction::MergeSegments(
                bundle.segments[source].id.clone(),
                bundle.segments[target].id.clone(),
            );
        }
    });
    if notes_changed {
        measurements.invalidate();
    }
    ui.data_mut(|data| data.insert_temp(measurements_id, measurements));
    if evidence.is_some() {
        controller.evidence_target = None;
    }
}

fn row_id(bundle: &MeetingBundle, kind: &str, id: &str) -> egui::Id {
    egui::Id::new(("meeting_row", &bundle.meeting.id, kind, id))
}

fn row_geometry(
    ui: &egui::Ui,
    entry: Entry,
    bundle: &MeetingBundle,
    markers: &HashMap<&str, Vec<usize>>,
    drafts: &HashMap<String, String>,
    empty_text: &str,
) -> virtual_list::MeasuredRow<Entry> {
    let (id, signature) = match entry {
        Entry::Topic(index) => {
            let topic = &bundle.topics[index];
            (
                row_id(bundle, "topic", &topic.id),
                egui::Id::new(&topic.title),
            )
        }
        Entry::Waiting(index) => (
            row_id(bundle, "waiting", &bundle.topics[index].id),
            egui::Id::NULL,
        ),
        Entry::Empty => (row_id(bundle, "empty", ""), egui::Id::new(empty_text)),
        Entry::Segment(index) => {
            let segment = &bundle.segments[index];
            let id = row_id(bundle, "segment", &segment.id);
            let expanded = ui
                .data(|data| data.get_temp::<bool>(id.with("notes")))
                .unwrap_or(false);
            let notes = markers
                .get(segment.id.as_str())
                .into_iter()
                .flatten()
                .map(|index| {
                    let marker = &bundle.markers[*index];
                    if expanded {
                        drafts.get(&marker.id).unwrap_or(&marker.text)
                    } else {
                        &marker.text
                    }
                })
                .collect::<Vec<_>>();
            (
                id,
                egui::Id::new((
                    &segment.original_text,
                    &segment.translated_text,
                    segment.is_final,
                    expanded,
                    notes,
                )),
            )
        }
    };
    virtual_list::MeasuredRow {
        id,
        signature,
        entry,
    }
}

fn prepare_row(
    ui: &egui::Ui,
    entry: Entry,
    bundle: &MeetingBundle,
    markers: &HashMap<&str, Vec<usize>>,
    speakers: &HashMap<&str, &Speaker>,
    drafts: &HashMap<String, String>,
    empty_text: &str,
    language: crate::i18n::UiLanguage,
) -> (f32, Row) {
    let row_id = |kind, id: &str| row_id(bundle, kind, id);
    let Entry::Segment(index) = entry else {
        let (id, text, extra) = match entry {
            Entry::Topic(index) => {
                let topic = &bundle.topics[index];
                (
                    row_id("topic", &topic.id),
                    egui::RichText::new(
                        topic
                            .title
                            .as_deref()
                            .unwrap_or_else(|| tr(language, "Untitled topic")),
                    )
                    .size(15.0)
                    .strong()
                    .color(crate::ui::theme::text_strong()),
                    8.0,
                )
            }
            Entry::Waiting(index) => (
                row_id("waiting", &bundle.topics[index].id),
                egui::RichText::new(tr(language, "Waiting for conversation…"))
                    .italics()
                    .color(crate::ui::theme::text_weak()),
                0.0,
            ),
            _ => (
                row_id("empty", ""),
                egui::RichText::new(empty_text).color(crate::ui::theme::text_weak()),
                0.0,
            ),
        };
        let original = rows::text(ui, text, ui.available_width());
        return (
            original.size().y + extra,
            Row {
                id,
                entry,
                original,
                translated: None,
                updating: None,
                speaker: String::new(),
                notes: Vec::new(),
                show_notes: false,
                header_height: 0.0,
            },
        );
    };
    let segment = &bundle.segments[index];
    let id = row_id("segment", &segment.id);
    let show_notes = ui
        .data(|data| data.get_temp::<bool>(id.with("notes")))
        .unwrap_or(false);
    let margins = components::card_margins(ui.ctx());
    let width = (ui.available_width() - margins.x).max(1.0);
    let original = rows::text(
        ui,
        egui::RichText::new(&segment.original_text)
            .size(13.0)
            .color(crate::ui::theme::text_weak()),
        width,
    );
    let translated = segment.translated_text.as_ref().map(|text| {
        rows::text(
            ui,
            egui::RichText::new(text)
                .size(14.0)
                .color(crate::ui::theme::text_strong()),
            width,
        )
    });
    let updating = (!segment.is_final).then(|| {
        rows::text(
            ui,
            egui::RichText::new(tr(language, "Updating…"))
                .italics()
                .size(11.0)
                .color(crate::ui::theme::text_weak()),
            width,
        )
    });
    let header_height =
        ui.spacing().interact_size.y.max(
            ui.text_style_height(&egui::TextStyle::Body) + 2.0 * ui.spacing().button_padding.y,
        );
    let mut heights = vec![header_height, original.size().y];
    heights.extend(
        translated
            .iter()
            .chain(updating.iter())
            .map(|galley| galley.size().y),
    );
    let notes = markers
        .get(segment.id.as_str())
        .into_iter()
        .flatten()
        .map(|index| {
            let marker = &bundle.markers[*index];
            let text = if show_notes {
                heights.push(super::highlights::editor_height(
                    ui,
                    width,
                    drafts
                        .get(&marker.id)
                        .map(String::as_str)
                        .unwrap_or(&marker.text),
                    language,
                ));
                None
            } else if !marker.text.trim().is_empty() {
                let text = rows::text(
                    ui,
                    egui::RichText::new(&marker.text)
                        .size(12.0)
                        .color(crate::ui::theme::text_normal()),
                    width,
                );
                heights.push(text.size().y);
                Some(text)
            } else {
                None
            };
            Note {
                index: *index,
                text,
            }
        })
        .collect();
    let speaker = segment
        .canonical_speaker_id
        .as_deref()
        .and_then(|id| speakers.get(id))
        .filter(|speaker| !speaker.is_provisional)
        .map(|speaker| speaker.name.clone())
        .or_else(|| {
            segment
                .speaker_token
                .as_deref()
                .and_then(|token| token.rsplit(':').next())
                .and_then(crate::compact_speaker_label)
        })
        .unwrap_or_else(|| tr(language, "Unknown speaker").to_owned());
    let height =
        margins.y + heights.iter().sum::<f32>() + 4.0 * heights.len().saturating_sub(1) as f32;
    (
        height,
        Row {
            id,
            entry,
            original,
            translated,
            updating,
            speaker,
            notes,
            show_notes,
            header_height,
        },
    )
}

fn render_segment(
    row: &Row,
    height: f32,
    bundle: &MeetingBundle,
    drafts: &mut HashMap<String, String>,
    can_seek: bool,
    can_merge: bool,
    active: bool,
    language: crate::i18n::UiLanguage,
    action: &mut UiAction,
    ui: &mut egui::Ui,
) -> bool {
    let Entry::Segment(index) = row.entry else {
        return false;
    };
    let mut notes_changed = false;
    let segment = &bundle.segments[index];
    let rect = egui::Rect::from_min_size(
        ui.next_widget_position(),
        egui::vec2(ui.available_width(), height),
    );
    components::card(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.horizontal(|ui| {
            ui.set_min_height(row.header_height);
            let marked = !row.notes.is_empty();
            let highlight = tr(
                language,
                if marked {
                    "★ Highlight"
                } else {
                    "☆ Highlight"
                },
            );
            let timestamp_width =
                rows::text(ui, super::timestamp_text(segment, active), f32::INFINITY)
                    .size()
                    .x;
            let highlight_width = rows::text(ui, egui::RichText::new(highlight), f32::INFINITY)
                .size()
                .x
                + 2.0 * ui.spacing().button_padding.x;
            let speaker_width = (ui.available_width()
                - timestamp_width
                - highlight_width
                - 2.0 * ui.spacing().item_spacing.x)
                .max(1.0);
            ui.allocate_ui_with_layout(
                egui::vec2(speaker_width, row.header_height),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.set_width(speaker_width);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&row.speaker).strong().size(12.0).color(
                                if active {
                                    crate::ui::theme::primary()
                                } else {
                                    crate::ui::theme::text_strong()
                                },
                            ),
                        )
                        .truncate(),
                    )
                    .on_hover_text(&row.speaker);
                },
            );
            super::timestamp(segment, can_seek, active, language, action, ui);
            if ui
                .selectable_label(marked, highlight)
                .on_hover_text(tr(
                    language,
                    "Mark this message; click again to edit its note.",
                ))
                .clicked()
            {
                if marked {
                    ui.data_mut(|data| data.insert_temp(row.id.with("notes"), !row.show_notes));
                    notes_changed = true;
                    ui.ctx().request_discard("Remeasure expanded meeting notes");
                } else {
                    *action = UiAction::Highlight(segment.id.clone());
                }
            }
        });
        super::sentence_text(
            row.original.clone(),
            &segment.id,
            can_seek,
            can_merge,
            language,
            action,
            ui,
        );
        if let Some(text) = &row.translated {
            super::sentence_text(
                text.clone(),
                &segment.id,
                can_seek,
                can_merge,
                language,
                action,
                ui,
            );
        }
        if let Some(text) = &row.updating {
            ui.add(egui::Label::new(text.clone()));
        }
        for note in &row.notes {
            if row.show_notes {
                super::highlights::note_editor(
                    &bundle.markers[note.index],
                    drafts,
                    language,
                    action,
                    ui,
                );
            } else if let Some(text) = &note.text {
                ui.add(egui::Label::new(text.clone()).wrap());
            }
        }
    });
    rows::record(ui, &segment.id, rect);
    notes_changed
}

fn segment_matches(segment: &Segment, query: &str) -> bool {
    segment.original_text.to_lowercase().contains(query)
        || segment
            .translated_text
            .as_deref()
            .unwrap_or_default()
            .to_lowercase()
            .contains(query)
}
