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
                    ui.add_space(6.0);
                });
            }
        });
    if evidence_reached {
        controller.evidence_target = None;
    }
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
    let frame = ui.scope(|ui| {
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
                ui.label(egui::RichText::new(speaker_name).strong().size(12.0));
                ui.label(
                    egui::RichText::new(format_duration(segment.start_ms))
                        .size(11.0)
                        .color(crate::ui::theme::text_weak())
                        .monospace(),
                );
                ui.menu_button("⋯", |ui| {
                    for kind in [
                        MarkerKind::KeyDecision,
                        MarkerKind::ActionItem,
                        MarkerKind::Note,
                    ] {
                        if ui.button(marker_label(kind, language)).clicked() {
                            *action = UiAction::AddMarker(segment.id.clone(), kind);
                            ui.close();
                        }
                    }
                })
                .response
                .on_hover_text(tr(language, "Add annotation"));
            });
            ui.add(
                egui::Label::new(
                    egui::RichText::new(&segment.original_text)
                        .size(13.0)
                        .color(crate::ui::theme::text_weak()),
                )
                .wrap(),
            );
            if let Some(translated) = &segment.translated_text {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(translated)
                            .size(14.0)
                            .color(crate::ui::theme::text_strong()),
                    )
                    .wrap(),
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
                ui.label(
                    egui::RichText::new(format!(
                        "{} · {}",
                        marker_label(marker.kind, language),
                        marker.text
                    ))
                    .size(12.0)
                    .color(crate::ui::theme::text_normal()),
                );
            }
        });
    });
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
