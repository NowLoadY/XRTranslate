use super::*;
use crate::i18n::{UiLanguage, tr};

#[derive(Clone, Debug, Default)]
pub(super) struct StylePanel {
    pub collapsed: bool,
    preset_name: String,
    profile_id: String,
    removed_style: Option<(usize, xrtranslate_prompt::TranslationStylePreset)>,
}

/// A canvas overlay: all edits go into the same draft and history as the node editor.
pub(super) fn render(
    snapshot: &PromptStudioSnapshot,
    controller: &mut PromptStudioController,
    draft: &mut PromptTemplateProfile,
    ui: &egui::Ui,
    canvas: Rect,
    language: UiLanguage,
    actions: &mut Vec<PromptStudioAction>,
) -> Option<Rect> {
    if controller.domain() != PromptGraphDomain::Translation {
        return None;
    }
    if controller.style_panel.profile_id != draft.id {
        controller.style_panel.profile_id = draft.id.clone();
        controller.style_panel.preset_name.clear();
        controller.style_panel.removed_style = None;
    }
    let before = draft.clone();
    let mut apply = false;
    let mut save = false;
    let area_id = egui::Id::new("translation_style_panel");
    let previous_size =
        egui::containers::AreaState::load(ui.ctx(), area_id).and_then(|state| state.size);
    let area = egui::Area::new(area_id)
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::LEFT_BOTTOM)
        .fixed_pos(canvas.left_bottom() + Vec2::new(10.0, -10.0))
        .constrain_to(canvas)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_width((canvas.width() - 48.0).clamp(100.0, 320.0));
                // Set the budget before placing widgets: Area's cached collapsed size
                // must not constrain expansion, and set_max_height resets the layout cursor.
                ui.set_max_height((canvas.height() - 36.0).clamp(1.0, 620.0));
                ui.horizontal(|ui| {
                    let arrow = if controller.style_panel.collapsed { "▸" } else { "▾" };
                    if ui.button(format!("{arrow} {}", tr(language, "Translation style"))).clicked() {
                        controller.style_panel.collapsed = !controller.style_panel.collapsed;
                    }
                    if ui.small_button(tr(language, "Locate node")).on_hover_text(tr(language, "Locate style node")).clicked() {
                        locate_node(&draft.graph, controller, canvas);
                    }
                });
                if controller.style_panel.collapsed { return; }
                let body_height = ui.available_height().max(1.0);
                let text_height = (body_height - 220.0).clamp(80.0, 340.0);
                egui::ScrollArea::vertical()
                    .id_salt("style_panel_body")
                    .max_height(body_height)
                    .min_scrolled_height(0.0)
                    .show(ui, |ui| {
                        let Some(mut text) = draft.graph.style_text() else {
                            ui.weak(tr(language, "Use a text node’s context menu to designate a shared translation style."));
                            return;
                        };
                        let connected = draft.graph.style_reaches_target(controller.active_provider);
                        if !connected {
                            ui.colored_label(style::ERROR_BORDER, tr(language, "Style node is not connected to this model."));
                        }
                        ui.weak(tr(language, "Style text"));
                        egui::ScrollArea::vertical().id_salt("style_prompt_scroll").max_height(text_height).min_scrolled_height(0.0).show(ui, |ui| {
                            let response = ui.add(egui::TextEdit::multiline(&mut text)
                                .id(egui::Id::new("translation_style_text"))
                                .desired_width(f32::INFINITY).desired_rows(8));
                            if response.changed() { draft.graph.set_style_text(&text); }
                        });
                        ui.separator();
                        ui.weak(tr(language, "Saved styles"));
                        let presets = draft.graph.translation_style.as_ref().unwrap().presets.clone();
                        let selected = presets.iter().position(|preset| preset.name == controller.style_panel.preset_name && preset.text == text)
                            .or_else(|| presets.iter().position(|preset| preset.text == text));
                        let mut remove = None;
                        egui::ScrollArea::vertical().id_salt("saved_translation_styles").max_height(110.0).min_scrolled_height(0.0).show(ui, |ui| {
                            for (index, preset) in presets.iter().enumerate() {
                                ui.push_id(index, |ui| {
                                    ui.horizontal(|ui| {
                                        // Leave room for the shared trash button and row spacing.
                                        if ui.add_sized([ui.available_width() - 40.0, 24.0],
                                            egui::Button::selectable(selected == Some(index), &preset.name).truncate())
                                            .on_hover_text(&preset.name).clicked() {
                                            draft.graph.set_style_text(&preset.text);
                                            controller.style_panel.preset_name = preset.name.clone();
                                            apply = connected;
                                        }
                                        if crate::ui::components::delete_icon_button(ui, ("saved_style", index),
                                            tr(language, "Remove from saved styles. Current style text is kept.")).clicked() {
                                            remove = Some(index);
                                        }
                                    });
                                });
                            }
                        });
                        if let Some(index) = remove {
                            let removed = draft.graph.translation_style.as_mut().unwrap().presets.remove(index);
                            controller.style_panel.removed_style = Some((index, removed));
                            save = true;
                        }
                        if controller.style_panel.removed_style.is_some() {
                            ui.horizontal_wrapped(|ui| {
                                ui.weak(tr(language, "Removed from list. Current text kept."));
                                if ui.small_button(tr(language, "Undo")).clicked() {
                                    let (index, preset) = controller.style_panel.removed_style.take().unwrap();
                                    let presets = &mut draft.graph.translation_style.as_mut().unwrap().presets;
                                    // A graph undo or a later save may already have restored this name.
                                    if !presets.iter().any(|saved| saved.name == preset.name) {
                                        presets.insert(index.min(presets.len()), preset);
                                        save = true;
                                    }
                                }
                            });
                        } else if presets.is_empty() {
                            ui.add(egui::Label::new(RichText::new(tr(language, "No saved styles. Current text is still available above.")).weak()).wrap());
                        }
                        ui.horizontal(|ui| {
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.add_enabled(!controller.style_panel.preset_name.trim().is_empty(), egui::Button::new(tr(language, "Save to list"))).clicked() {
                                    save = draft.graph.save_style_preset(&controller.style_panel.preset_name);
                                    controller.style_panel.removed_style = None;
                                }
                                ui.add(egui::TextEdit::singleline(&mut controller.style_panel.preset_name)
                                    .desired_width(ui.available_width().max(40.0))
                                    .hint_text(tr(language, "Style name")));
                            });
                        });
                        let validation = draft.graph.validate_for_activation();
                        if let Err(error) = &validation {
                            ui.colored_label(style::ERROR_BORDER, error.to_string());
                        }
                        ui.horizontal(|ui| {
                            if ui.add_enabled(connected && validation.is_ok(), egui::Button::new(tr(language, "Apply style"))).clicked() {
                                apply = true;
                            }
                            if controller.dirty || draft.graph != before.graph {
                                ui.weak(tr(language, "Unsaved"));
                            } else if draft.id == snapshot.active_id {
                                ui.weak(tr(language, "Active"));
                            }
                        });
                    });
            });
        });
    if previous_size.is_none_or(|size| (size - area.response.rect.size()).abs().max_elem() > 1.0) {
        if draft == &before && !apply && !save {
            // Repaint at the newly measured bottom anchor in this frame. Do not replay
            // graph mutations just to correct the layout after saving or deleting a style.
            ui.ctx().request_discard("translation style panel resized");
        } else {
            ui.ctx().request_repaint();
        }
    }
    if draft != &before {
        record_edit(controller, draft, before, actions);
    }
    if (apply || save) && draft.graph.validate_for_activation().is_ok() {
        // Save/activate also creates new profiles. Avoid a redundant clone action resetting history.
        actions.retain(|action| !matches!(action, PromptStudioAction::CloneProfile(profile) if profile.id == draft.id));
        actions.push(if apply || draft.id == snapshot.active_id {
            PromptStudioAction::ActivateProfile(draft.clone())
        } else {
            PromptStudioAction::SaveProfile(draft.clone())
        });
        controller.dirty = false;
    }
    Some(area.response.rect)
}

fn record_edit(
    controller: &mut PromptStudioController,
    draft: &mut PromptTemplateProfile,
    mut before: PromptTemplateProfile,
    actions: &mut Vec<PromptStudioAction>,
) {
    if draft.read_only {
        *draft = PromptTemplateLibrary::editable_copy_of(
            draft,
            format!("custom-{}", uuid::Uuid::new_v4()),
        );
        // Preserve the visible layout when the read-only overview becomes an editable graph.
        for node in &mut draft.graph.nodes {
            if let Some(position) = controller.overview_positions.get(&node.id) {
                node.position = *position;
            }
        }
        actions.push(PromptStudioAction::CloneProfile(draft.clone()));
        controller.set_draft(draft.clone());
        controller.style_panel.profile_id = draft.id.clone();
        before.id = draft.id.clone();
        before.name = draft.name.clone();
        before.read_only = false;
        for node in &mut before.graph.nodes {
            if let Some(current) = draft
                .graph
                .nodes
                .iter()
                .find(|current| current.id == node.id)
            {
                node.position = current.position;
            }
        }
    }
    controller.push_history(before);
}

fn locate_node(graph: &PromptNodeGraph, controller: &mut PromptStudioController, canvas: Rect) {
    if let Some(node) = graph
        .translation_style
        .as_ref()
        .and_then(|style| graph.nodes.iter().find(|node| node.id == style.node_id))
    {
        let position = controller.node_position(node);
        let size = node_size(graph, node);
        // Leave the bottom-left overlay clear of the located node.
        let center = Vec2::new(canvas.width() * 0.7, canvas.height() * 0.35);
        controller.canvas.pan =
            center - (Vec2::from(position) + size * 0.5) * controller.canvas.zoom;
        controller.canvas.fit_pending = false;
    }
}

pub(super) fn paint_connection(
    ui: &egui::Ui,
    canvas: Rect,
    panel: Rect,
    graph: &PromptNodeGraph,
    controller: &PromptStudioController,
) {
    let Some(node) = graph
        .translation_style
        .as_ref()
        .and_then(|style| graph.nodes.iter().find(|node| node.id == style.node_id))
    else {
        return;
    };
    if !controller.node_is_visible(node) {
        return;
    }
    let rect = controller.canvas.graph_rect(
        canvas,
        controller.node_position(node),
        node_size(graph, node),
    );
    let from = panel.center();
    let to = rect.clamp(from);
    ui.painter().extend(egui::Shape::dashed_line(
        &[from, to],
        Stroke::new(1.5, GRAPH_ACCENT),
        6.0,
        5.0,
    ));
    ui.painter().rect_stroke(
        rect.expand(3.0),
        4.0,
        Stroke::new(1.5, GRAPH_ACCENT),
        egui::StrokeKind::Outside,
    );
}
