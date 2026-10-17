use super::super::actions::UiAction;
use crate::plugins::meeting::{
    controller::MeetingController,
    i18n::tr,
    store::{Segment, SegmentMarker, Speaker},
};
use crate::ui::components;
use eframe::egui;

pub(super) fn render_timeline(
    controller: &mut MeetingController,
    language: crate::i18n::UiLanguage,
    action: &mut UiAction,
    ui: &mut egui::Ui,
) {
    if controller
        .bundle
        .as_ref()
        .is_some_and(|bundle| !bundle.segments.is_empty())
    {
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
    }

    let live = controller.bundle.as_ref().is_some_and(|bundle| {
        controller.active_meeting_id().as_deref() == Some(bundle.meeting.id.as_str())
    });
    let can_seek = controller.active_meeting_id().is_none();
    let playback = &controller.playback;
    let Some(bundle) = controller.bundle.as_mut() else {
        return;
    };
    let active_id = playback
        .current_segment(&bundle.segments)
        .map(|segment| segment.id.clone());
    let evidence_target = controller.evidence_target.clone();
    let mut evidence_reached = false;
    let query = controller.search.trim().to_lowercase();
    let highlights_only = controller.highlights_only;
    let mut visible_count = 0;
    egui::ScrollArea::vertical()
        .id_salt("meeting_timeline_scroll")
        .stick_to_bottom(live && query.is_empty() && !highlights_only && evidence_target.is_none())
        .show(ui, |ui| {
            for topic in &bundle.topics {
                let topic_matches = topic
                    .title
                    .as_deref()
                    .unwrap_or_default()
                    .to_lowercase()
                    .contains(&query);
                let topic_segments = bundle
                    .segments
                    .iter()
                    .filter(|segment| {
                        let marked = bundle
                            .markers
                            .iter()
                            .any(|marker| marker.segment_id == segment.id);
                        let note_matches = bundle.markers.iter().any(|marker| {
                            marker.segment_id == segment.id
                                && marker.text.to_lowercase().contains(&query)
                        });
                        segment.topic_id == topic.id
                            && (!highlights_only || marked)
                            && (query.is_empty()
                                || topic_matches
                                || segment_matches(segment, &query)
                                || note_matches)
                    })
                    .collect::<Vec<_>>();
                if topic_segments.is_empty() && (!query.is_empty() || highlights_only) {
                    continue;
                }
                visible_count += topic_segments.len();
                ui.push_id(("topic", &topic.id), |ui| {
                    if let Some(title) = topic.title.as_deref() {
                        components::section_heading(ui, title);
                    } else if bundle.topics.len() > 1 {
                        components::section_heading(ui, tr(language, "Untitled topic"));
                    }
                    if topic_segments.is_empty() {
                        ui.label(
                            egui::RichText::new(tr(language, "Waiting for conversation…"))
                                .italics()
                                .color(crate::ui::theme::text_weak()),
                        );
                    }
                    for segment in topic_segments {
                        render_segment(
                            segment,
                            super::segment_preview(&controller.previews, segment)
                                .and_then(|preview| preview.translated.as_deref()),
                            &bundle.speakers,
                            &bundle.markers,
                            &mut controller.marker_drafts,
                            evidence_target.as_deref(),
                            &mut evidence_reached,
                            can_seek && playback.can_seek_segment(segment),
                            active_id.as_deref() == Some(&segment.id),
                            language,
                            action,
                            ui,
                        );
                        ui.add_space(6.0);
                    }
                    ui.add_space(6.0);
                });
            }
            if visible_count == 0 && (highlights_only || !query.is_empty()) {
                ui.weak(tr(
                    language,
                    if highlights_only && query.is_empty() {
                        "No highlights yet. Mark a message to find it here."
                    } else {
                        "No matching messages"
                    },
                ));
            }
            if query.is_empty() && !highlights_only {
                super::render_unstored_previews(ui, &controller.previews, &bundle.segments);
            }
        });
    if evidence_reached {
        controller.evidence_target = None;
    }
}

fn render_segment(
    segment: &Segment,
    preview_translation: Option<&str>,
    speakers: &[Speaker],
    markers: &[SegmentMarker],
    drafts: &mut std::collections::HashMap<String, String>,
    evidence_target: Option<&str>,
    evidence_reached: &mut bool,
    can_seek: bool,
    active: bool,
    language: crate::i18n::UiLanguage,
    action: &mut UiAction,
    ui: &mut egui::Ui,
) {
    let marked = markers.iter().any(|marker| marker.segment_id == segment.id);
    let notes_id = egui::Id::new(("meeting_highlight_notes", &segment.id));
    let mut show_notes = ui
        .data(|data| data.get_temp::<bool>(notes_id))
        .unwrap_or(false);
    let frame =
        ui.push_id(&segment.id, |ui| {
            components::card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 4.0;
                ui.horizontal_wrapped(|ui| {
                    let speaker = segment
                        .canonical_speaker_id
                        .as_deref()
                        .and_then(|id| speakers.iter().find(|speaker| speaker.id == id));
                    let anonymous = segment
                        .speaker_token
                        .as_deref()
                        .and_then(|token| token.rsplit(':').next())
                        .and_then(crate::compact_speaker_label);
                    let speaker_name = speaker
                        .filter(|speaker| !speaker.is_provisional)
                        .map(|speaker| speaker.name.as_str())
                        .or(anonymous.as_deref())
                        .unwrap_or_else(|| tr(language, "Unknown speaker"));
                    ui.label(egui::RichText::new(speaker_name).strong().size(12.0).color(
                        if active {
                            crate::ui::theme::primary()
                        } else {
                            crate::ui::theme::text_strong()
                        },
                    ));
                    super::timestamp(segment, can_seek, active, language, action, ui);
                    if ui
                        .selectable_label(
                            marked,
                            tr(
                                language,
                                if marked {
                                    "★ Highlight"
                                } else {
                                    "☆ Highlight"
                                },
                            ),
                        )
                        .on_hover_text(tr(
                            language,
                            "Mark this message; click again to edit its note.",
                        ))
                        .clicked()
                    {
                        if marked {
                            show_notes = !show_notes;
                            ui.data_mut(|data| data.insert_temp(notes_id, show_notes));
                        } else {
                            *action = UiAction::Highlight(segment.id.clone());
                        }
                    }
                });
                super::sentence_text(
                    egui::RichText::new(&segment.original_text)
                        .size(13.0)
                        .color(crate::ui::theme::text_weak()),
                    &segment.id,
                    can_seek,
                    language,
                    action,
                    ui,
                );
                if let Some(translated) = preview_translation.or(segment.translated_text.as_deref())
                {
                    super::sentence_text(
                        egui::RichText::new(translated)
                            .size(14.0)
                            .color(crate::ui::theme::text_strong()),
                        &segment.id,
                        can_seek,
                        language,
                        action,
                        ui,
                    );
                }
                if !segment.is_final {
                    ui.label(
                        egui::RichText::new(tr(language, "Updating…"))
                            .italics()
                            .size(11.0)
                            .color(crate::ui::theme::text_weak()),
                    );
                }
                for marker in markers
                    .iter()
                    .filter(|marker| marker.segment_id == segment.id)
                {
                    if show_notes {
                        super::highlights::note_editor(marker, drafts, language, action, ui);
                    } else if !marker.text.trim().is_empty() {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&marker.text)
                                    .size(12.0)
                                    .color(crate::ui::theme::text_normal()),
                            )
                            .wrap(),
                        );
                    }
                }
            });
        });
    super::current_marker(ui, frame.response.rect, active);
    if evidence_target == Some(segment.id.as_str()) {
        ui.scroll_to_rect(frame.response.rect, Some(egui::Align::Center));
        *evidence_reached = true;
    }
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
