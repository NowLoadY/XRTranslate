use super::super::{
    actions::UiAction,
    presentation::{format_duration, marker_label},
};
use crate::plugins::meeting::{
    controller::MeetingController,
    i18n::tr,
    store::{MarkerKind, Segment, SegmentMarker, Speaker},
};
use crate::ui::components;
use eframe::egui::{self, Color32, CornerRadius, Frame, Margin, Stroke};

pub(super) fn render_timeline(
    controller: &mut MeetingController,
    language: crate::i18n::UiLanguage,
    action: &mut UiAction,
    ui: &mut egui::Ui,
) {
    ui.horizontal(|ui| {
        crate::ui::components::text_edit_ui(
            ui,
            "meeting_search",
            egui::TextEdit::singleline(&mut controller.search)
                .hint_text(tr(language, "Search this meeting"))
                .desired_width(240.0),
        );
        ui.add_space(10.0);
        crate::ui::components::text_edit_ui(
            ui,
            "meeting_new_topic",
            egui::TextEdit::singleline(&mut controller.new_topic_title)
                .hint_text(tr(language, "New topic title"))
                .desired_width(200.0),
        );
        if components::animated_button(ui, tr(language, "New topic")).clicked() {
            *action = UiAction::NewTopic;
        }
    });

    ui.add_space(8.0);

    let speakers = controller
        .bundle
        .as_ref()
        .map(|bundle| bundle.speakers.clone())
        .unwrap_or_default();

    if !speakers.is_empty() {
        egui::CollapsingHeader::new(tr(language, "Manage speakers")).show(ui, |ui| {
            for speaker in &speakers {
                ui.push_id(("speaker-editor", &speaker.id), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        let name = controller
                            .speaker_name_drafts
                            .entry(speaker.id.clone())
                            .or_insert_with(|| speaker.name.clone());
                        crate::ui::components::text_edit_ui(
                            ui,
                            ("speaker_name", &speaker.id),
                            egui::TextEdit::singleline(name).desired_width(180.0),
                        );
                        if components::animated_button(ui, tr(language, "Rename")).clicked() {
                            *action = UiAction::RenameSpeaker(speaker.id.clone(), name.clone());
                        }
                        if speakers.len() > 1 {
                            let target = controller
                                .speaker_merge_targets
                                .entry(speaker.id.clone())
                                .or_insert_with(|| {
                                    speakers
                                        .iter()
                                        .find(|other| other.id != speaker.id)
                                        .map(|other| other.id.clone())
                                        .unwrap_or_default()
                                });

                            let merge_options: Vec<_> = speakers
                                .iter()
                                .filter(|other| other.id != speaker.id)
                                .map(|other| (other.id.clone(), other.name.clone()))
                                .collect();

                            let current_target_name = speakers
                                .iter()
                                .find(|other| other.id == *target)
                                .map(|other| other.name.as_str())
                                .unwrap_or_else(|| tr(language, "Merge into…"));

                            components::searchable_combobox(
                                ui,
                                ("merge-target", &speaker.id),
                                current_target_name,
                                target,
                                &merge_options,
                            );

                            if components::danger_button(ui, tr(language, "Merge")).clicked()
                                && !target.is_empty()
                            {
                                *action =
                                    UiAction::MergeSpeaker(speaker.id.clone(), target.clone());
                            }
                        }
                    });
                });
            }
            ui.label(
                egui::RichText::new(tr(
                    language,
                    "Automatic speaker labels are provisional. Renaming confirms an identity; merging redirects all linked voice clusters.",
                ))
                .size(11.0)
                .color(crate::ui::theme::text_weak()),
            );
        });
        ui.add_space(8.0);
    }

    let Some(bundle) = controller.bundle.as_mut() else {
        return;
    };
    let evidence_target = controller.evidence_target.clone();
    let mut evidence_reached = false;
    let query = controller.search.to_lowercase();
    egui::ScrollArea::vertical()
        .id_salt("meeting_timeline_scroll")
        .stick_to_bottom(query.is_empty())
        .show(ui, |ui| {
            for topic in &bundle.topics {
                let topic_segments = bundle
                    .segments
                    .iter()
                    .filter(|segment| segment.topic_id == topic.id)
                    .collect::<Vec<_>>();
                let visible = query.is_empty()
                    || topic
                        .title
                        .as_deref()
                        .unwrap_or_default()
                        .to_lowercase()
                        .contains(&query)
                    || topic_segments
                        .iter()
                        .any(|segment| segment_matches(segment, &query));
                if !visible {
                    continue;
                }
                ui.push_id(("topic", &topic.id), |ui| {
                    components::section(
                        ui,
                        topic.title.as_deref().unwrap_or("Untitled topic"),
                        |ui| {
                            if topic_segments.is_empty() {
                                ui.label(
                                    egui::RichText::new("No conversation in this topic yet")
                                        .italics()
                                        .color(crate::ui::theme::text_weak()),
                                );
                            }
                            for segment in topic_segments {
                                if !query.is_empty() && !segment_matches(segment, &query) {
                                    continue;
                                }
                                render_segment(
                                    segment,
                                    &bundle.speakers,
                                    &bundle.markers,
                                    evidence_target.as_deref(),
                                    &mut evidence_reached,
                                    language,
                                    action,
                                    ui,
                                );
                                ui.add_space(6.0);
                            }
                        },
                    );
                    ui.add_space(9.0);
                });
            }
        });
    if evidence_reached {
        controller.evidence_target = None;
    }
    ui.separator();
    ui.horizontal(|ui| {
        crate::ui::components::text_edit_ui(
            ui,
            "meeting_quick_note",
            egui::TextEdit::singleline(&mut controller.quick_note)
                .hint_text(tr(language, "Quick note linked to the latest message"))
                .desired_width(f32::INFINITY),
        );
        if components::primary_button(ui, tr(language, "Add note")).clicked() {
            *action = UiAction::QuickNote;
        }
    });
}

fn render_segment(
    segment: &Segment,
    speakers: &[Speaker],
    markers: &[SegmentMarker],
    evidence_target: Option<&str>,
    evidence_reached: &mut bool,
    language: crate::i18n::UiLanguage,
    action: &mut UiAction,
    ui: &mut egui::Ui,
) {
    let frame = Frame::new()
        .fill(Color32::from_rgb(250, 252, 255))
        .stroke(Stroke::new(1.0, Color32::from_rgb(226, 232, 240)))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    let speaker = segment
                        .canonical_speaker_id
                        .as_deref()
                        .and_then(|id| speakers.iter().find(|speaker| speaker.id == id));

                    let speaker_name = speaker
                        .map(|s| s.name.as_str())
                        .unwrap_or_else(|| tr(language, "Unknown speaker"));

                    components::speaker_badge(ui, speaker_name);

                    if segment.speaker_token.is_some() && speaker.is_none() {
                        ui.label(
                            egui::RichText::new(tr(language, "automatic cluster"))
                                .size(11.0)
                                .color(crate::ui::theme::text_weak()),
                        );
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(format_duration(segment.start_ms))
                                .size(11.0)
                                .color(crate::ui::theme::text_weak())
                                .monospace(),
                        );
                    });
                });

                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(&segment.original_text)
                        .size(13.0)
                        .color(Color32::from_rgb(71, 85, 105)),
                );

                if let Some(translated) = &segment.translated_text {
                    ui.add_space(4.0);
                    Frame::new()
                        .fill(Color32::from_rgb(241, 245, 249))
                        .corner_radius(CornerRadius::same(6))
                        .inner_margin(Margin::symmetric(8, 5))
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(translated)
                                    .strong()
                                    .size(13.5)
                                    .color(crate::ui::theme::text_strong()),
                            );
                        });
                }

                if !segment.is_final {
                    ui.add_space(2.0);
                    ui.label(
                        egui::RichText::new(tr(language, "Updating…"))
                            .italics()
                            .size(11.5)
                            .color(Color32::from_rgb(37, 99, 235)),
                    );
                }

                ui.add_space(6.0);

                // Quick Marker Pill Buttons
                ui.horizontal_wrapped(|ui| {
                    if tag_button(
                        ui,
                        tr(language, "Key decision"),
                        Color32::from_rgb(254, 243, 199),
                        Color32::from_rgb(180, 83, 9),
                    )
                    .clicked()
                    {
                        *action = UiAction::AddMarker(segment.id.clone(), MarkerKind::KeyDecision);
                    }
                    if tag_button(
                        ui,
                        tr(language, "Action item"),
                        Color32::from_rgb(209, 250, 229),
                        Color32::from_rgb(4, 120, 87),
                    )
                    .clicked()
                    {
                        *action = UiAction::AddMarker(segment.id.clone(), MarkerKind::ActionItem);
                    }
                    if tag_button(
                        ui,
                        tr(language, "Note"),
                        Color32::from_rgb(224, 231, 255),
                        Color32::from_rgb(67, 56, 202),
                    )
                    .clicked()
                    {
                        *action = UiAction::AddMarker(segment.id.clone(), MarkerKind::Note);
                    }
                });

                let attached_markers: Vec<_> = markers
                    .iter()
                    .filter(|marker| marker.segment_id == segment.id)
                    .collect();

                if !attached_markers.is_empty() {
                    ui.add_space(4.0);
                    for marker in attached_markers {
                        Frame::new()
                            .fill(Color32::from_rgb(255, 255, 255))
                            .stroke(Stroke::new(1.0, Color32::from_rgb(226, 232, 240)))
                            .corner_radius(CornerRadius::same(6))
                            .inner_margin(Margin::symmetric(8, 4))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new(marker_label(marker.kind, language))
                                            .size(11.5)
                                            .strong(),
                                    );
                                    ui.label(
                                        egui::RichText::new(&marker.text)
                                            .size(12.0)
                                            .color(crate::ui::theme::text_strong()),
                                    );
                                });
                            });
                    }
                }
            });
        });

    if evidence_target == Some(segment.id.as_str()) {
        ui.scroll_to_rect(frame.response.rect, Some(egui::Align::Center));
        *evidence_reached = true;
    }
}

fn tag_button(ui: &mut egui::Ui, text: &str, bg: Color32, fg: Color32) -> egui::Response {
    Frame::new()
        .fill(bg)
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).color(fg).size(11.0).strong())
        })
        .response
        .interact(egui::Sense::click())
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
