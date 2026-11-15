use super::super::runtime::{
    BannerConfig, BannerContentType, ChatboxPreview, IdleContent, OscSettings,
};
use crate::{
    i18n::{UiLanguage, tr},
    ui::components,
};
use eframe::egui;

#[cfg(test)]
#[path = "definition_tests.rs"]
mod tests;

/// Definitions follow measured preview rows. Editors stay inside the dropdowns.
pub(super) fn render(
    ui: &mut egui::Ui,
    settings: &mut OscSettings,
    preview: &ChatboxPreview,
    language: UiLanguage,
    lyrics_available: bool,
) -> bool {
    let mut changed = false;
    egui::ScrollArea::horizontal()
        .id_salt("osc_preview_definitions")
        .auto_shrink([false, true])
        .show(ui, |ui| {
            let width = ui.available_width().max(360.0);
            let definition_width = (width * 0.38).clamp(160.0, 220.0);
            let gap = 16.0;
            let preview_width = width - definition_width - gap;
            let color = egui::Color32::from_rgb(241, 245, 249);
            let regions = [
                &preview.regions.header,
                &preview.regions.body,
                &preview.regions.footer,
            ];
            let labels = ["Header:", "Message area:", "Footer:"];
            let galleys = regions
                .iter()
                .enumerate()
                .map(|(index, text)| {
                    let display = if text.is_empty() {
                        tr(language, if index == 1 { "Empty" } else { labels[index] })
                    } else {
                        text.as_str()
                    };
                    let stage = preview.regions.pet_stage && index == 1;
                    let mut job = egui::text::LayoutJob::simple(
                        display.to_owned(),
                        egui::FontId::monospace(13.5),
                        if text.is_empty() {
                            egui::Color32::from_rgb(100, 116, 139)
                        } else {
                            color
                        },
                        if stage {
                            f32::INFINITY
                        } else {
                            preview_width - 28.0
                        },
                    );
                    job.halign = egui::Align::Center;
                    if stage {
                        let measured = ui.painter().layout_job(job.clone()).size().x;
                        if measured > preview_width - 28.0 {
                            for section in &mut job.sections {
                                section.format.font_id.size *= (preview_width - 28.0) / measured;
                            }
                        }
                    }
                    ui.painter().layout_job(job)
                })
                .collect::<Vec<_>>();
            let controls_height = 2.0 * ui.spacing().interact_size.y + ui.spacing().item_spacing.y;
            let merged = preview.regions.unified_lyrics.then(|| {
                let text = regions
                    .iter()
                    .filter(|text| !text.is_empty())
                    .map(|text| text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                let mut job = egui::text::LayoutJob::simple(
                    text,
                    egui::FontId::monospace(13.5),
                    color,
                    preview_width - 28.0,
                );
                job.halign = egui::Align::Center;
                ui.painter().layout_job(job)
            });
            let mut heights = galleys
                .iter()
                .enumerate()
                .map(|(index, galley)| {
                    (galley.size().y + 16.0).max(if index == 1 {
                        controls_height + 12.0
                    } else {
                        ui.spacing().interact_size.y + 12.0
                    })
                })
                .collect::<Vec<_>>();
            if let Some(galley) = &merged {
                let edge = ui.spacing().interact_size.y + 12.0;
                let height = (galley.size().y + 16.0).max(edge * 2.0 + controls_height + 12.0);
                heights = vec![edge, height - edge * 2.0, edge];
            }
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(width, heights.iter().sum()),
                egui::Sense::hover(),
            );
            let bubble =
                egui::Rect::from_min_size(rect.min, egui::vec2(preview_width, rect.height()));
            ui.painter().rect(
                bubble,
                egui::CornerRadius::same(16),
                egui::Color32::from_rgb(15, 23, 42),
                egui::Stroke::new(1.0, egui::Color32::from_rgb(51, 65, 85)),
                egui::StrokeKind::Inside,
            );
            if let Some(galley) = &merged {
                ui.painter().galley(
                    egui::pos2(
                        bubble.center().x,
                        bubble.center().y - galley.rect.center().y,
                    ),
                    galley.clone(),
                    color,
                );
            }
            let mut top = rect.top();
            for (index, (galley, height)) in galleys.into_iter().zip(heights).enumerate() {
                let row = egui::Rect::from_min_size(
                    egui::pos2(rect.left(), top),
                    egui::vec2(width, height),
                );
                if merged.is_none() {
                    ui.painter().galley(
                        egui::pos2(bubble.center().x, row.center().y - galley.rect.center().y),
                        galley,
                        color,
                    );
                }
                let definition_rect = egui::Rect::from_min_size(
                    egui::pos2(bubble.right() + gap, top),
                    egui::vec2(definition_width, height),
                );
                ui.painter().line_segment(
                    [
                        egui::pos2(bubble.right() + 3.0, row.center().y),
                        egui::pos2(definition_rect.left() - 3.0, row.center().y),
                    ],
                    egui::Stroke::new(1.0, crate::ui::theme::border()),
                );
                let definition_rect = if index == 1 {
                    egui::Rect::from_min_size(
                        egui::pos2(
                            definition_rect.left(),
                            row.center().y - controls_height / 2.0,
                        ),
                        egui::vec2(definition_width, controls_height),
                    )
                } else {
                    definition_rect
                };
                let mut definition_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .id_salt(("osc_definition", index))
                        .max_rect(definition_rect)
                        .layout(if index == 1 {
                            egui::Layout::top_down(egui::Align::Min)
                        } else {
                            egui::Layout::left_to_right(egui::Align::Center)
                        }),
                );
                match index {
                    0 => {
                        changed |= banner_definition(
                            &mut definition_ui,
                            "header_type_combo",
                            labels[index],
                            &mut settings.header_config,
                            language,
                            lyrics_available,
                            definition_width,
                        )
                    }
                    1 => {
                        changed |= body_definition(
                            &mut definition_ui,
                            settings,
                            language,
                            lyrics_available,
                            definition_width,
                        );
                        changed |= suffix_definition(
                            &mut definition_ui,
                            settings,
                            language,
                            definition_width,
                        )
                    }
                    _ => {
                        changed |= banner_definition(
                            &mut definition_ui,
                            "footer_type_combo",
                            labels[index],
                            &mut settings.footer_config,
                            language,
                            lyrics_available,
                            definition_width,
                        )
                    }
                }
                top += height;
            }
        });
    changed
}

fn body_definition(
    ui: &mut egui::Ui,
    settings: &mut OscSettings,
    language: UiLanguage,
    lyrics_available: bool,
    width: f32,
) -> bool {
    components::combobox_ui_with_close_behavior(
        ui, "osc_idle_content",
        format!("{} · {}", tr(language, "Message area:"), settings.idle_content.label(language)),
        Some(width), egui::PopupCloseBehavior::CloseOnClickOutside,
        |ui| {
            ui.set_min_width(230.0);
            let mut changed = false;
            for kind in [IdleContent::None, IdleContent::Lyrics] {
                if kind == IdleContent::Lyrics && !lyrics_available { continue; }
                changed |= components::selectable_value(ui, &mut settings.idle_content, kind, kind.label(language)).changed();
            }
            ui.separator();
            ui.small(tr(language, "Show lyrics when there are no messages. All three lyric regions combine into one display."));
            if settings.idle_content == IdleContent::Lyrics && !lyrics_available {
                ui.label(tr(language, "Lyrics unavailable. Check the plugin in Settings."));
            }
            changed
        },
    ).inner.unwrap_or(false)
}

fn banner_definition(
    ui: &mut egui::Ui,
    id: &str,
    label: &'static str,
    banner: &mut BannerConfig,
    language: UiLanguage,
    lyrics_available: bool,
    width: f32,
) -> bool {
    let label = tr(language, label);
    ui.scope(|ui| {
        ui.spacing_mut().combo_height = 360.0;
        components::combobox_ui_with_close_behavior(
            ui,
            id,
            format!("{label} · {}", banner.content_type.label(language)),
            Some(width),
            egui::PopupCloseBehavior::CloseOnClickOutside,
            |ui| {
                ui.set_min_width(230.0);
                let mut changed = false;
                for kind in [
                    BannerContentType::None,
                    BannerContentType::CustomText,
                    BannerContentType::SystemTime,
                    BannerContentType::CpuStatus,
                    BannerContentType::GpuStatus,
                    BannerContentType::Lyrics,
                    BannerContentType::TextPet,
                ] {
                    if kind == BannerContentType::Lyrics && !lyrics_available {
                        continue;
                    }
                    changed |= components::selectable_value(
                        ui,
                        &mut banner.content_type,
                        kind,
                        kind.label(language),
                    )
                    .changed();
                }
                ui.separator();
                // Reserve the detail area in all modes so switching to a pet or a
                // custom editor cannot leave the dropdown's scroll viewport too short.
                changed |= ui
                    .allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), 112.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_min_height(112.0);
                            if banner.content_type == BannerContentType::TextPet {
                                return super::pet::render(ui, &mut banner.pet, language);
                            }
                            let mut changed = ui
                                .add_enabled_ui(
                                    banner.content_type == BannerContentType::CustomText,
                                    |ui| {
                                        text_editor(
                                            ui,
                                            id,
                                            label,
                                            &mut banner.custom_text,
                                            tr(language, "Custom Text"),
                                        )
                                    },
                                )
                                .inner;
                            changed |= ui
                                .add_enabled_ui(
                                    matches!(
                                        banner.content_type,
                                        BannerContentType::CpuStatus | BannerContentType::GpuStatus
                                    ),
                                    |ui| {
                                        components::checkbox(
                                            ui,
                                            &mut banner.show_device_name,
                                            tr(language, "Full Name"),
                                        )
                                        .changed()
                                    },
                                )
                                .inner;
                            changed
                        },
                    )
                    .inner;
                if banner.content_type == BannerContentType::Lyrics && !lyrics_available {
                    ui.label(tr(
                        language,
                        "Lyrics unavailable. Check the plugin in Settings.",
                    ));
                }
                changed
            },
        )
        .inner
        .unwrap_or(false)
    })
    .inner
}

fn suffix_definition(
    ui: &mut egui::Ui,
    settings: &mut OscSettings,
    language: UiLanguage,
    width: f32,
) -> bool {
    let label = tr(language, "Microphone tail:");
    let mode = tr(
        language,
        if settings.microphone_suffix_enabled {
            "Custom Text"
        } else {
            "None"
        },
    );
    components::combobox_ui_with_close_behavior(
        ui,
        "osc_microphone_suffix",
        format!("{label} · {mode}"),
        Some(width),
        egui::PopupCloseBehavior::CloseOnClickOutside,
        |ui| {
            ui.set_min_width(230.0);
            let mut changed = false;
            for (enabled, name) in [(false, "None"), (true, "Custom Text")] {
                changed |= components::selectable_value(
                    ui,
                    &mut settings.microphone_suffix_enabled,
                    enabled,
                    tr(language, name),
                )
                .changed();
            }
            ui.separator();
            changed |= ui
                .add_enabled_ui(settings.microphone_suffix_enabled, |ui| {
                    text_editor(
                        ui,
                        "osc_microphone_suffix_text",
                        label,
                        &mut settings.microphone_suffix,
                        "喵",
                    )
                })
                .inner;
            ui.small(tr(
                language,
                "Give microphone translations a little tail, like: meow~",
            ));
            changed
        },
    )
    .inner
    .unwrap_or(false)
}

fn text_editor(ui: &mut egui::Ui, id: &str, label: &str, text: &mut String, hint: &str) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let response = components::text_edit_ui(
            ui,
            id,
            egui::TextEdit::singleline(text)
                .hint_text(hint)
                .desired_width((ui.available_width() - 36.0).max(120.0)),
        );
        changed |= ui_director::egui::text_response(ui, label, text, response).changed();
        if components::reset_button(ui, id).clicked() {
            text.clear();
            changed = true;
        }
    });
    changed
}
