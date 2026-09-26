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

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(
        ctx: &egui::Context,
        controller: &mut PromptStudioController,
        library: &PromptTemplateLibrary,
        size: Vec2,
        events: Vec<egui::Event>,
    ) -> (egui::FullOutput, Vec<PromptStudioAction>) {
        egui_extras::install_image_loaders(ctx);
        let snapshot = controller.snapshot(library);
        let mut actions = Vec::new();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                events,
                ..Default::default()
            },
            |ui| {
                actions = super::super::render(&snapshot, controller, ui, UiLanguage::English);
            },
        );
        output.textures_delta.clear();
        (output, actions)
    }

    fn text_position(output: &egui::FullOutput, text: &str) -> Pos2 {
        fn find(shape: &egui::Shape, text: &str) -> Option<Pos2> {
            match shape {
                egui::Shape::Text(shape) if shape.galley.text() == text => {
                    Some(shape.pos + shape.galley.rect.center().to_vec2())
                }
                egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| find(shape, text)),
                _ => None,
            }
        }
        output
            .shapes
            .iter()
            .find_map(|shape| find(&shape.shape, text))
            .unwrap_or_else(|| panic!("missing UI text: {text}"))
    }

    fn click(
        ctx: &egui::Context,
        controller: &mut PromptStudioController,
        library: &PromptTemplateLibrary,
        size: Vec2,
        point: Pos2,
    ) -> Vec<PromptStudioAction> {
        let mut actions = Vec::new();
        for pressed in [true, false] {
            actions.extend(
                frame(
                    ctx,
                    controller,
                    library,
                    size,
                    vec![
                        egui::Event::PointerMoved(point),
                        egui::Event::PointerButton {
                            pos: point,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                )
                .1,
            );
        }
        actions
    }

    #[test]
    fn typing_in_panel_edits_the_shared_node_and_preserves_builtin_and_history() {
        let ctx = egui::Context::default();
        let mut library = PromptTemplateLibrary::default();
        let builtin = library.active_graph();
        let mut controller = PromptStudioController::default();
        let size = Vec2::new(1200.0, 850.0);
        for _ in 0..3 {
            frame(&ctx, &mut controller, &library, size, vec![]);
        }
        ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("translation_style_text")));
        let (_, actions) = frame(
            &ctx,
            &mut controller,
            &library,
            size,
            vec![egui::Event::Text("Keep {names}. ".into())],
        );
        let [PromptStudioAction::CloneProfile(copy)] = actions.as_slice() else {
            panic!("{actions:?}")
        };
        assert!(!copy.read_only);
        library.profiles.push(copy.clone());
        // Main applies this action after rendering; selecting the dirty draft must keep its history.
        controller.select_profile(copy.id.clone(), &library);
        assert!(
            controller
                .snapshot(&library)
                .draft
                .graph
                .style_text()
                .unwrap()
                .contains("Keep {names}. ")
        );
        assert_eq!(library.active_graph(), builtin);
        controller.undo();
        assert_eq!(
            controller.draft.as_ref().unwrap().graph.style_text(),
            builtin.style_text()
        );
        controller.redo();
        assert_eq!(
            controller.draft.as_ref().unwrap().graph.style_text(),
            copy.graph.style_text()
        );
    }

    #[test]
    fn saved_style_click_activates_same_graph_and_saving_preserves_named_texts() {
        let ctx = egui::Context::default();
        let mut library = PromptTemplateLibrary::default();
        let mut profile = PromptTemplateLibrary::editable_copy_of(
            library.active_profile().unwrap(),
            "style-ui-test",
        );
        let default = profile.graph.style_text().unwrap();
        profile
            .graph
            .set_style_text("Speak like a poet. Keep {names}.");
        profile.graph.save_style_preset("My poetic voice");
        profile.graph.set_style_text(&default);
        library.active_id = profile.id.clone();
        library.profiles.push(profile.clone());
        let mut controller = PromptStudioController::default();
        let size = Vec2::new(1200.0, 900.0);
        for _ in 0..3 {
            frame(&ctx, &mut controller, &library, size, vec![]);
        }
        let (output, _) = frame(&ctx, &mut controller, &library, size, vec![]);
        let actions = click(
            &ctx,
            &mut controller,
            &library,
            size,
            text_position(&output, "My poetic voice"),
        );
        let [PromptStudioAction::ActivateProfile(applied)] = actions.as_slice() else {
            panic!("{actions:?}")
        };
        assert_eq!(applied.id, profile.id);
        assert_eq!(
            applied.graph.style_text().as_deref(),
            Some("Speak like a poet. Keep {names}.")
        );
        assert_eq!(applied.graph.links, profile.graph.links);
        controller.undo();
        assert_eq!(
            controller
                .draft
                .as_ref()
                .unwrap()
                .graph
                .style_text()
                .unwrap(),
            default
        );
        controller.redo();
        controller.style_panel.preset_name = "My own name".into();
        let (output, _) = frame(&ctx, &mut controller, &library, size, vec![]);
        let actions = click(
            &ctx,
            &mut controller,
            &library,
            size,
            text_position(&output, "Save to list"),
        );
        let [PromptStudioAction::ActivateProfile(saved)] = actions.as_slice() else {
            panic!("{actions:?}")
        };
        assert_eq!(
            saved
                .graph
                .translation_style
                .as_ref()
                .unwrap()
                .presets
                .len(),
            3
        );
        assert_eq!(
            saved
                .graph
                .translation_style
                .as_ref()
                .unwrap()
                .presets
                .last()
                .unwrap()
                .name,
            "My own name"
        );
    }

    #[test]
    fn removing_and_saving_a_style_keeps_current_text_and_offers_restore() {
        let ctx = egui::Context::default();
        let mut library = PromptTemplateLibrary::default();
        let original = PromptTemplateLibrary::editable_copy_of(
            library.active_profile().unwrap(),
            "remove-style-test",
        );
        library.active_id = original.id.clone();
        library.profiles.push(original.clone());
        let mut controller = PromptStudioController::default();
        let size = Vec2::new(1200.0, 850.0);
        for _ in 0..3 {
            frame(&ctx, &mut controller, &library, size, vec![]);
        }
        // Click the same SVG trash-button widget used by onboarding, aligned with the saved row.
        let remove = |controller: &mut PromptStudioController, library: &PromptTemplateLibrary| {
            let (output, _) = frame(&ctx, controller, library, size, vec![]);
            let panel =
                egui::containers::AreaState::load(&ctx, egui::Id::new("translation_style_panel"))
                    .unwrap()
                    .rect();
            let point = Pos2::new(panel.right() - 20.0, text_position(&output, "Default").y);
            click(&ctx, controller, library, size, point)
        };
        let actions = remove(&mut controller, &library);
        let [PromptStudioAction::ActivateProfile(removed)] = actions.as_slice() else {
            panic!("{actions:?}")
        };
        assert!(
            removed
                .graph
                .translation_style
                .as_ref()
                .unwrap()
                .presets
                .is_empty()
        );
        assert_eq!(removed.graph.nodes, original.graph.nodes);
        assert_eq!(removed.graph.links, original.graph.links);
        // Persisting the removal does not remove the active node text.
        let persisted: PromptTemplateProfile =
            serde_json::from_str(&serde_json::to_string(removed).unwrap()).unwrap();
        assert_eq!(persisted.graph.style_text(), original.graph.style_text());
        *library
            .profiles
            .iter_mut()
            .find(|profile| profile.id == original.id)
            .unwrap() = persisted;
        for _ in 0..2 {
            frame(&ctx, &mut controller, &library, size, vec![]);
        }
        let (output, _) = frame(&ctx, &mut controller, &library, size, vec![]);
        text_position(&output, "Removed from list. Current text kept.");
        let actions = click(
            &ctx,
            &mut controller,
            &library,
            size,
            text_position(&output, "Undo"),
        );
        let [PromptStudioAction::ActivateProfile(restored)] = actions.as_slice() else {
            panic!("{actions:?}")
        };
        assert_eq!(restored.graph, original.graph);
        *library
            .profiles
            .iter_mut()
            .find(|profile| profile.id == original.id)
            .unwrap() = restored.clone();
        for _ in 0..2 {
            frame(&ctx, &mut controller, &library, size, vec![]);
        }
        let actions = remove(&mut controller, &library);
        let [PromptStudioAction::ActivateProfile(removed)] = actions.as_slice() else {
            panic!("{actions:?}")
        };
        *library
            .profiles
            .iter_mut()
            .find(|profile| profile.id == original.id)
            .unwrap() = removed.clone();
        controller.style_panel.preset_name = "Default".into();
        for _ in 0..2 {
            frame(&ctx, &mut controller, &library, size, vec![]);
        }
        let (output, _) = frame(&ctx, &mut controller, &library, size, vec![]);
        let actions = click(
            &ctx,
            &mut controller,
            &library,
            size,
            text_position(&output, "Save to list"),
        );
        let [PromptStudioAction::ActivateProfile(saved)] = actions.as_slice() else {
            panic!("{actions:?}")
        };
        assert_eq!(saved.graph, original.graph);
        assert!(controller.style_panel.removed_style.is_none());
    }

    #[test]
    fn many_named_styles_scroll_inside_the_small_panel() {
        let ctx = egui::Context::default();
        let mut library = PromptTemplateLibrary::default();
        let mut profile = PromptTemplateLibrary::editable_copy_of(
            library.active_profile().unwrap(),
            "many-styles",
        );
        for index in 0..25 {
            profile
                .graph
                .set_style_text(&format!("Custom style {index}"));
            profile
                .graph
                .save_style_preset(&format!("Saved voice {index}"));
        }
        profile
            .graph
            .save_style_preset(&"A deliberately long style name ".repeat(15));
        library.active_id = profile.id.clone();
        library.profiles.push(profile);
        let mut controller = PromptStudioController::default();
        let size = Vec2::new(1100.0, 800.0);
        for _ in 0..3 {
            frame(&ctx, &mut controller, &library, size, vec![]);
        }
        let (output, _) = frame(&ctx, &mut controller, &library, size, vec![]);
        let pointer = text_position(&output, "Saved voice 0");
        let before = (controller.canvas.pan, controller.canvas.zoom);
        // Exercise the actual scroll area to expose a saved prompt beyond the initial rows.
        for _ in 0..15 {
            frame(
                &ctx,
                &mut controller,
                &library,
                size,
                vec![
                    egui::Event::PointerMoved(pointer),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        phase: egui::TouchPhase::Move,
                        delta: Vec2::new(0.0, -100.0),
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
        let (output, _) = frame(&ctx, &mut controller, &library, size, vec![]);
        let last = text_position(&output, "Saved voice 24");
        let panel =
            egui::containers::AreaState::load(&ctx, egui::Id::new("translation_style_panel"))
                .unwrap()
                .rect();
        assert!(panel.contains(last));
        assert!(panel.width() < 350.0 && panel.height() < 650.0, "{panel:?}");
        assert_eq!((controller.canvas.pan, controller.canvas.zoom), before);
    }

    #[test]
    fn reopening_after_collapse_restores_the_full_panel_and_visible_actions() {
        let ctx = egui::Context::default();
        let mut library = PromptTemplateLibrary::default();
        let mut profile = PromptTemplateLibrary::editable_copy_of(
            library.active_profile().unwrap(),
            "reopen-panel",
        );
        profile
            .graph
            .translation_style
            .as_mut()
            .unwrap()
            .presets
            .clear();
        library.active_id = profile.id.clone();
        library.profiles.push(profile);
        let mut controller = PromptStudioController::default();
        let size = Vec2::new(1100.0, 900.0);
        let area = || {
            egui::containers::AreaState::load(&ctx, egui::Id::new("translation_style_panel"))
                .unwrap()
                .rect()
        };
        for _ in 0..3 {
            frame(&ctx, &mut controller, &library, size, vec![]);
        }
        let expanded = area();
        assert!(expanded.height() > 440.0, "{expanded:?}");
        for _ in 0..3 {
            let (output, _) = frame(&ctx, &mut controller, &library, size, vec![]);
            click(
                &ctx,
                &mut controller,
                &library,
                size,
                text_position(&output, "▾ Translation style"),
            );
            let (output, _) = frame(&ctx, &mut controller, &library, size, vec![]);
            assert!(area().height() < 50.0);
            click(
                &ctx,
                &mut controller,
                &library,
                size,
                text_position(&output, "▸ Translation style"),
            );
            for _ in 0..3 {
                frame(&ctx, &mut controller, &library, size, vec![]);
            }
            let (output, _) = frame(&ctx, &mut controller, &library, size, vec![]);
            assert!(
                (area().height() - expanded.height()).abs() < 2.0,
                "initial={expanded:?}, reopened={:?}",
                area()
            );
            for label in ["Saved styles", "Save to list", "Apply style"] {
                let point = text_position(&output, label);
                assert!(area().contains(point));
                assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == label && shape.clip_rect.contains(point))));
            }
        }
    }

    #[test]
    fn panel_is_anchored_collapsible_and_hover_scroll_does_not_move_graph() {
        let ctx = egui::Context::default();
        let library = PromptTemplateLibrary::default();
        let mut controller = PromptStudioController::default();
        let size = Vec2::new(1100.0, 720.0);
        for _ in 0..3 {
            frame(&ctx, &mut controller, &library, size, vec![]);
        }
        let area = || {
            egui::containers::AreaState::load(&ctx, egui::Id::new("translation_style_panel"))
                .unwrap()
                .rect()
        };
        let rect = area();
        assert!(
            rect.left() < 35.0 && rect.bottom() > size.y - 35.0,
            "{rect:?}"
        );
        assert!(rect.width() < 350.0 && rect.height() < 650.0, "{rect:?}");
        let before = (controller.canvas.pan, controller.canvas.zoom);
        let (output, _) = frame(
            &ctx,
            &mut controller,
            &library,
            size,
            vec![
                egui::Event::PointerMoved(rect.center()),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    phase: egui::TouchPhase::Move,
                    delta: Vec2::new(0.0, -80.0),
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        assert_eq!((controller.canvas.pan, controller.canvas.zoom), before);
        // Hover draws the dashed leader (individual line segments) in the graph's accent color.
        assert!(output.shapes.iter().any(|s| matches!(&s.shape, egui::Shape::LineSegment { stroke, .. } if stroke.color == GRAPH_ACCENT && stroke.width == 1.5)));
        let title = text_position(&output, "▾ Translation style");
        click(&ctx, &mut controller, &library, size, title);
        frame(&ctx, &mut controller, &library, size, vec![]);
        assert!(controller.style_panel.collapsed);
        assert!(area().height() < 50.0);
        let small = Vec2::new(800.0, 540.0);
        for _ in 0..2 {
            frame(&ctx, &mut controller, &library, small, vec![]);
        }
        assert!(area().bottom() <= small.y && area().bottom() > small.y - 35.0);
        let (output, _) = frame(&ctx, &mut controller, &library, small, vec![]);
        click(
            &ctx,
            &mut controller,
            &library,
            small,
            text_position(&output, "▸ Translation style"),
        );
        for _ in 0..2 {
            frame(&ctx, &mut controller, &library, small, vec![]);
        }
        let (output, _) = frame(&ctx, &mut controller, &library, small, vec![]);
        assert!(area().top() >= 0.0 && area().bottom() <= small.y);
        for label in ["Save to list", "Apply style"] {
            let point = text_position(&output, label);
            assert!(area().contains(point));
            assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == label && shape.clip_rect.contains(point))));
        }
        let actions = click(
            &ctx,
            &mut controller,
            &library,
            small,
            text_position(&output, "Apply style"),
        );
        assert!(matches!(
            actions.as_slice(),
            [PromptStudioAction::ActivateProfile(_)]
        ));
    }
}
