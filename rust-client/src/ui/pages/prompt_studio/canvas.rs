use super::*;
use crate::ui::graph_canvas;

#[derive(Clone, Copy)]
struct NodePalette {
    fill: Color32,
    header: Color32,
    connector: Color32,
}

fn node_palette(kind: &PromptNodeKind) -> NodePalette {
    let (fill, header, connector) = match kind {
        PromptNodeKind::Input { .. } => ((250, 251, 249), (220, 224, 220), (100, 108, 103)),
        PromptNodeKind::Variable { .. } => ((248, 251, 251), (216, 222, 223), (98, 109, 111)),
        PromptNodeKind::SystemValue { .. } => ((248, 251, 251), (210, 224, 226), (76, 112, 118)),
        PromptNodeKind::ConditionValue { .. } => ((251, 249, 252), (224, 214, 226), (118, 88, 122)),
        PromptNodeKind::BoolValue { .. } => ((249, 251, 248), (218, 229, 214), (69, 118, 65)),
        PromptNodeKind::TextComparison { .. } => ((251, 249, 252), (224, 214, 226), (118, 88, 122)),
        PromptNodeKind::Compose { .. } => ((252, 251, 247), (225, 221, 210), (116, 108, 88)),
        PromptNodeKind::Switch { .. } => ((251, 249, 252), (223, 217, 224), (110, 100, 113)),
        PromptNodeKind::TextSwitch => ((249, 250, 253), (215, 222, 235), (75, 91, 140)),
        PromptNodeKind::Request { .. } => ((248, 250, 252), (215, 222, 228), (96, 107, 117)),
    };
    NodePalette {
        fill: Color32::from_rgb(fill.0, fill.1, fill.2),
        header: Color32::from_rgb(header.0, header.1, header.2),
        connector: Color32::from_rgb(connector.0, connector.1, connector.2),
    }
}

fn node_kind_tag(graph: &PromptNodeGraph, node: &PromptNode) -> String {
    match &node.kind {
        PromptNodeKind::Input { .. } => "DATA".into(),
        PromptNodeKind::Variable { .. } => "VALUE".into(),
        PromptNodeKind::SystemValue { .. } => "HOST · TEXT".into(),
        PromptNodeKind::ConditionValue { .. } => "HOST · BOOL".into(),
        PromptNodeKind::BoolValue { .. } => "VALUE · BOOL".into(),
        PromptNodeKind::TextComparison { .. } => "TEXT → BOOL".into(),
        PromptNodeKind::Compose { .. } => {
            let count = graph.links.iter().filter(|link| link.to == node.id).count();
            format!("COMPOSE · {count}/10")
        }
        PromptNodeKind::Switch { .. } => "BRANCH".into(),
        PromptNodeKind::TextSwitch => format!(
            "TEXT BRANCH · {}",
            graph
                .text_switch_cases(&node.id)
                .map_or(0, |cases| cases.len())
        ),
        PromptNodeKind::Request { roles, .. } => format!("REQUEST · {}", roles.len()),
    }
}

fn input_description(kind: &PromptNodeKind) -> String {
    match kind {
        PromptNodeKind::Input {
            block: TranslationPromptBlock::LanguageOrder,
        } => "Preferred language sequence".into(),
        PromptNodeKind::Input {
            block: TranslationPromptBlock::Terminology,
        } => "Required terminology rows".into(),
        PromptNodeKind::Input {
            block: TranslationPromptBlock::RecentTurns { limit },
        } => limit.map_or_else(
            || "Completed bilingual history".into(),
            |limit| format!("Last {limit} bilingual turns"),
        ),
        PromptNodeKind::Input {
            block: TranslationPromptBlock::PreviousRevision,
        } => "Earlier streaming revision".into(),
        PromptNodeKind::Input {
            block: TranslationPromptBlock::SurroundingSource,
        } => "Nearby source speech".into(),
        PromptNodeKind::Input {
            block: TranslationPromptBlock::CustomText { .. },
        } => "Fixed instruction text".into(),
        PromptNodeKind::SystemValue { .. } => "Host/runtime supplied value".into(),
        PromptNodeKind::ConditionValue { .. } => "Host/runtime supplied condition".into(),
        PromptNodeKind::BoolValue { .. } => "User-editable boolean value".into(),
        PromptNodeKind::TextComparison { .. } => "Compare connected text and output boolean".into(),
        _ => String::new(),
    }
}

fn condition_expression(condition: PromptCondition) -> &'static str {
    match condition {
        PromptCondition::SourceIsAuto => "Is source language set to auto?",
        PromptCondition::HasReferenceContext => "Is reference context available?",
        PromptCondition::HasRecognitionContext => "Is recognition context available?",
        PromptCondition::IsPseudoStreaming => "Is recognition mode pseudo-streaming?",
    }
}

fn request_summary(message_count: usize) -> String {
    let noun = if message_count == 1 {
        "MESSAGE"
    } else {
        "MESSAGES"
    };
    format!("{message_count} {noun} · ONE API REQUEST")
}

#[derive(Clone, Debug, Default)]
struct GraphErrorTarget {
    node_id: Option<String>,
    input_index: Option<u8>,
}

fn parse_validation_error_target(
    error: Option<&xrtranslate_prompt::PromptGraphError>,
) -> GraphErrorTarget {
    let Some(err) = error else {
        return GraphErrorTarget::default();
    };
    let msg = err.to_string();
    if let Some(rest) = msg.strip_prefix("node ") {
        if let Some((node_id, after_node)) = rest.split_once(" input ") {
            if let Some((input_str, _)) = after_node.split_once(" is not connected") {
                if let Ok(idx) = input_str.parse::<u8>() {
                    return GraphErrorTarget {
                        node_id: Some(node_id.trim().to_string()),
                        input_index: Some(idx),
                    };
                }
            }
        }
    }
    if let Some(rest) = msg.strip_prefix("compose node ") {
        if let Some((node_id, _)) = rest.split_once(':') {
            return GraphErrorTarget {
                node_id: Some(node_id.trim().to_string()),
                input_index: None,
            };
        }
    }
    if let Some(rest) = msg.strip_prefix("text switch ") {
        let node_id = rest.split_whitespace().next().unwrap_or_default();
        if !node_id.is_empty() {
            return GraphErrorTarget {
                node_id: Some(node_id.into()),
                input_index: Some(0),
            };
        }
    }
    if let Some(rest) = msg.strip_prefix("provider request ") {
        let node_id = rest.split_whitespace().next().unwrap_or_default();
        if !node_id.is_empty() {
            return GraphErrorTarget {
                node_id: Some(node_id.trim().to_string()),
                input_index: None,
            };
        }
    }
    GraphErrorTarget::default()
}

fn remove_compose_placeholder(text: &mut String, input: u8) {
    let target = format!("{{{input}}}");
    *text = text.replace(&format!("\n\n{target}"), "");
    *text = text.replace(&format!("\n{target}"), "");
    *text = text.replace(&target, "");
    *text = text.trim_end().to_string();
}

fn input_socket_tooltip(
    graph: &PromptNodeGraph,
    node: &PromptNode,
    input: u8,
    error_target: &GraphErrorTarget,
) -> String {
    let socket = input_socket_label(graph, node, input);
    let is_error = error_target.node_id.as_deref() == Some(node.id.as_str())
        && error_target.input_index == Some(input);
    if is_error {
        return format!("{socket} · UNCONNECTED (Required by prompt template)");
    }
    graph
        .links
        .iter()
        .find(|link| link.to == node.id && link.input == input)
        .and_then(|link| graph.nodes.iter().find(|source| source.id == link.from))
        .map_or_else(
            || match &node.kind {
                PromptNodeKind::Compose { text } if text.contains(&format!("{{{input}}}")) => {
                    format!("{socket} · Unconnected placeholder in text (Right-click to remove {{{input}}})")
                }
                _ => format!("{socket} · Not connected (Drag to connect)"),
            },
            |source| {
                format!(
                    "{socket} · Connected to {}\n(Drag to pull off · Right-click to disconnect)",
                    node_display_label(source)
                )
            },
        )
}

pub(super) fn render_graph_editor(
    snapshot: &PromptStudioSnapshot,
    controller: &mut PromptStudioController,
    ui: &mut egui::Ui,
    language: crate::i18n::UiLanguage,
    actions: &mut Vec<PromptStudioAction>,
) {
    let Some(mut draft) = controller.draft.clone() else {
        return;
    };
    controller.sync_branch_filters(&draft.graph);
    let runtime_trace = (snapshot.selected_id == snapshot.active_id && !controller.dirty)
        .then(|| controller.runtime_trace.clone())
        .flatten()
        .filter(|trace| {
            trace.target == controller.active_provider
                && trace.graph_fingerprint == draft.graph.fingerprint()
        });
    let validation_error = draft.graph.validate_for_activation().err();
    let error_target = parse_validation_error_target(validation_error.as_ref());
    let editable = !draft.read_only;
    crate::ui::layout::flow_row(ui, |ui| {
        ui.label(
            RichText::new(crate::i18n::tr(language, "GRAPH /"))
                .font(egui::FontId::monospace(10.0))
                .color(style::MUTED)
                .strong(),
        );
        ui.add_space(5.0);
        if editable {
            if crate::ui::components::text_edit_ui(
                ui,
                "prompt_graph_name",
                egui::TextEdit::singleline(&mut draft.name)
                    .font(egui::FontId::monospace(12.0))
                    .desired_width(260.0),
            )
            .changed()
            {
                controller.mark_dirty();
            }
        } else {
            ui.label(
                RichText::new(crate::i18n::tr_dynamic(language, &draft.name))
                    .font(egui::FontId::monospace(12.0))
                    .color(style::INK)
                    .strong(),
            );
        }
        ui.add_space(10.0);
        if !editable {
            status_chip(ui, crate::i18n::tr(language, "LOCKED"));
        }
        ui.separator();
        render_provider_tabs(controller, ui);
    });
    ui.add_space(2.0);
    crate::ui::layout::flow_row(ui, |ui| {
        render_branch_filters(&draft.graph, controller, ui, language);
    });
    ui.add_space(2.0);
    crate::ui::layout::flow_row(ui, |ui| {
        if editable {
            render_node_toolbar(&mut draft, controller, ui, language);
            if small_outline_button(
                ui,
                crate::i18n::tr(language, "AUTO LAYOUT"),
                crate::i18n::tr(language, "Automatically arrange nodes"),
            )
            .clicked()
            {
                let before = draft.clone();
                draft.graph.auto_layout();
                controller.canvas.fit_pending = true;
                controller.push_history(before);
            }
            if small_outline_button(
                ui,
                crate::i18n::tr(language, "FIT"),
                crate::i18n::tr(language, "Fit graph to canvas"),
            )
            .clicked()
            {
                controller.canvas.fit_pending = true;
            }
            if small_icon_button(ui, "-", crate::i18n::tr(language, "Zoom out")).clicked() {
                controller.canvas.zoom = (controller.canvas.zoom - 0.1).clamp(0.25, 1.6);
            }
            if small_icon_button(ui, "+", crate::i18n::tr(language, "Zoom in")).clicked() {
                controller.canvas.zoom = (controller.canvas.zoom + 0.1).clamp(0.25, 1.6);
            }
            ui.separator();
            let undo_enabled = controller.can_undo();
            let undo_btn = ui
                .add_enabled(
                    undo_enabled,
                    egui::Button::new(
                        RichText::new(crate::i18n::tr(language, "UNDO"))
                            .font(egui::FontId::monospace(9.5))
                            .color(if undo_enabled {
                                style::INK
                            } else {
                                style::MUTED
                            }),
                    )
                    .fill(Color32::TRANSPARENT)
                    .stroke(Stroke::new(1.0, style::BAR_BORDER))
                    .corner_radius(CornerRadius::same(1))
                    .min_size(Vec2::new(52.0, 25.0)),
                )
                .on_hover_text(crate::i18n::tr(language, "Undo last action (Ctrl+Z)"));
            if undo_btn.clicked() {
                controller.undo();
                if let Some(d) = &controller.draft {
                    draft = d.clone();
                }
            }
            let redo_enabled = controller.can_redo();
            let redo_btn = ui
                .add_enabled(
                    redo_enabled,
                    egui::Button::new(
                        RichText::new(crate::i18n::tr(language, "REDO"))
                            .font(egui::FontId::monospace(9.5))
                            .color(if redo_enabled {
                                style::INK
                            } else {
                                style::MUTED
                            }),
                    )
                    .fill(Color32::TRANSPARENT)
                    .stroke(Stroke::new(1.0, style::BAR_BORDER))
                    .corner_radius(CornerRadius::same(1))
                    .min_size(Vec2::new(52.0, 25.0)),
                )
                .on_hover_text(crate::i18n::tr(
                    language,
                    "Redo last action (Ctrl+Y / Ctrl+Shift+Z)",
                ));
            if redo_btn.clicked() {
                controller.redo();
                if let Some(d) = &controller.draft {
                    draft = d.clone();
                }
            }
        }
        crate::ui::layout::flow_group(ui, 260.0, |ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if editable {
                    if small_outline_button(
                        ui,
                        crate::i18n::tr(language, "DELETE"),
                        crate::i18n::tr(language, "Delete prompt design"),
                    )
                    .clicked()
                    {
                        actions.push(PromptStudioAction::DeleteProfile(draft.id.clone()));
                    }
                    if validation_error.is_none()
                        && draft.id != snapshot.active_id
                        && style::command_button(ui, crate::i18n::tr(language, "ACTIVATE"), true)
                            .clicked()
                    {
                        actions.push(PromptStudioAction::ActivateProfile(draft.clone()));
                        controller.dirty = false;
                    }
                } else {
                    if validation_error.is_none()
                        && draft.id != snapshot.active_id
                        && style::command_button(ui, crate::i18n::tr(language, "ACTIVATE"), true)
                            .clicked()
                    {
                        actions.push(PromptStudioAction::ActivateProfile(draft.clone()));
                        controller.dirty = false;
                    }
                    if small_outline_button(
                        ui,
                        crate::i18n::tr(language, "EDIT COPY"),
                        crate::i18n::tr(language, "Create an editable graph copy"),
                    )
                    .clicked()
                    {
                        let mut copy = PromptTemplateLibrary::editable_copy_of(
                            &draft,
                            format!("custom-{}", uuid::Uuid::new_v4()),
                        );
                        copy.name = format!("{} copy", draft.name);
                        actions.push(PromptStudioAction::CloneProfile(copy.clone()));
                        controller.set_draft(copy);
                    }
                }
                if small_outline_button(
                    ui,
                    crate::i18n::tr(language, "EXPORT"),
                    crate::i18n::tr(language, "Export graph project file"),
                )
                .clicked()
                {
                    actions.push(PromptStudioAction::ExportProfile(draft.clone()));
                }
                if small_outline_button(
                    ui,
                    crate::i18n::tr(language, "IMPORT"),
                    crate::i18n::tr(language, "Import graph project file"),
                )
                .clicked()
                {
                    actions.push(PromptStudioAction::ImportProfile);
                }
            });
        });
    });
    if let Some(error) = &validation_error {
        ui.label(
            RichText::new(format!(
                "{} / {error}",
                crate::i18n::tr(language, "INVALID GRAPH")
            ))
            .font(egui::FontId::monospace(10.0))
            .color(style::ERROR_BORDER),
        );
    }
    ui.add_space(4.0);

    Frame::new()
        .fill(style::CANVAS_FILL)
        .stroke(Stroke::new(1.0, style::CANVAS_BORDER))
        .corner_radius(CornerRadius::same(2))
        .inner_margin(Margin::same(5))
        .show(ui, |ui| {
            let canvas_height = ui.available_height().max(1.0);
            let (canvas, response) = ui.allocate_exact_size(
                Vec2::new(ui.available_width(), canvas_height),
                Sense::click_and_drag(),
            );
            controller.canvas.canvas_size = canvas.size();

            let pointer_over_node_or_link =
                response.interact_pointer_pos().is_some_and(|pointer| {
                    let over_node = draft
                        .graph
                        .nodes
                        .iter()
                        .filter(|node| controller.node_is_visible(node))
                        .any(|node| {
                            controller
                                .canvas
                                .graph_rect(canvas, node.position, node_size(&draft.graph, node))
                                .contains(pointer)
                        });
                    let over_link = draft.graph.links.iter().any(|link| {
                        let endpoints_visible = draft
                            .graph
                            .nodes
                            .iter()
                            .filter(|node| node.id == link.from || node.id == link.to)
                            .all(|node| controller.node_is_visible(node));
                        if !endpoints_visible {
                            return false;
                        }
                        if let Some((from, to)) =
                            link_points(canvas, controller, &draft.graph, link)
                        {
                            graph_canvas::distance_to_curve(
                                pointer,
                                graph_canvas::bezier_points(from, to),
                            ) <= 12.0
                        } else {
                            false
                        }
                    });
                    over_node || over_link
                });

            let wire_cancelled = editable && controller.handle_secondary_wire_cancel(canvas, ui);
            let is_pulling_wire = controller.wire_active();

            if editable
                && response.secondary_clicked()
                && !wire_cancelled
                && !pointer_over_node_or_link
            {
                controller.add_node_center = response
                    .interact_pointer_pos()
                    .map(|pointer| controller.canvas.graph_position(canvas, pointer));
            }
            if editable && !wire_cancelled && !pointer_over_node_or_link && !is_pulling_wire {
                let preferred_center = controller.add_node_center;
                response.context_menu(|ui| {
                    render_node_menu(&mut draft, controller, ui, language, preferred_center);
                });
            }
            if controller.canvas.fit_pending {
                navigation::fit_graph_to_canvas(&draft.graph, controller, canvas.size());
                controller.canvas.fit_pending = false;
            }
            let mut canvas_ui = graph_canvas::canvas_viewport(ui, canvas);
            controller.handle_navigation(canvas, &response, &canvas_ui, true, false);
            let pointer_over_node = response.interact_pointer_pos().is_some_and(|pointer| {
                draft
                    .graph
                    .nodes
                    .iter()
                    .filter(|node| controller.node_is_visible(node))
                    .any(|node| {
                        controller
                            .canvas
                            .graph_rect(canvas, node.position, node_size(&draft.graph, node))
                            .contains(pointer)
                    })
            });
            let pointer_over_link = pointer_over_node_or_link && !pointer_over_node;
            let selectable_nodes = draft
                .graph
                .nodes
                .iter()
                .filter(|node| controller.node_is_visible(node))
                .map(|node| {
                    (
                        node.id.clone(),
                        controller.canvas.graph_rect(
                            canvas,
                            node.position,
                            node_size(&draft.graph, node),
                        ),
                    )
                })
                .collect::<Vec<_>>();
            controller.handle_canvas_selection(
                &response,
                &canvas_ui,
                editable,
                pointer_over_node,
                pointer_over_link,
                selectable_nodes,
            );
            if response.hovered() {
                let scroll = canvas_ui.input(|input| input.smooth_scroll_delta.y);
                if scroll.abs() > f32::EPSILON {
                    let pointer = canvas_ui
                        .input(|input| input.pointer.hover_pos())
                        .unwrap_or(canvas.center());
                    let over_runtime_preview = draft
                        .graph
                        .nodes
                        .iter()
                        .filter(|node| controller.node_is_visible(node))
                        .any(|node| {
                            let rect = controller.canvas.graph_rect(
                                canvas,
                                node.position,
                                node_size(&draft.graph, node),
                            );
                            node_scale(rect, node) >= 0.58
                                && runtime_preview::pane_rect(rect, node_scale(rect, node))
                                    .contains(pointer)
                        });
                    if !over_runtime_preview {
                        controller.canvas.zoom_at_pointer(canvas, pointer, scroll);
                    }
                }
            }
            if editable {
                match crate::ui::graph_editor::shortcut(&canvas_ui) {
                    Some(crate::ui::graph_editor::GraphShortcut::Undo) => {
                        controller.undo();
                        if let Some(d) = &controller.draft {
                            draft = d.clone();
                        }
                    }
                    Some(crate::ui::graph_editor::GraphShortcut::Redo) => {
                        controller.redo();
                        if let Some(d) = &controller.draft {
                            draft = d.clone();
                        }
                    }
                    Some(crate::ui::graph_editor::GraphShortcut::Delete) => {
                        let before = draft.clone();
                        let mut modified = false;
                        let (selected, selected_links) = controller.take_selection();
                        for id in &selected {
                            draft.graph.remove_node(&id);
                            modified = true;
                        }
                        for link_key in &selected_links {
                            draft.graph.links.retain(|link| {
                                !(link.from == link_key.from
                                    && link.to == link_key.to
                                    && link.input == link_key.input)
                            });
                            modified = true;
                        }
                        if modified {
                            controller.push_history(before);
                        }
                    }
                    Some(crate::ui::graph_editor::GraphShortcut::Cancel) => {
                        controller.cancel_current_operation();
                        controller.cancel_editing_title();
                    }
                    None => {}
                }
            }
            graph_canvas::paint_grid(&canvas_ui, canvas, &controller.canvas, style::GRID);
            render_links(&mut canvas_ui, canvas, &mut draft, controller, editable);
            render_nodes(
                &mut canvas_ui,
                canvas,
                &mut draft,
                controller,
                editable,
                runtime_trace.as_ref(),
                &error_target,
                language,
            );
            if editable
                && controller.rewire_link.is_some()
                && controller.wire_from.is_some()
                && canvas_ui.input(|input| input.pointer.any_released())
                && let Some(commit) = controller.finish_wire(None)
            {
                commit_prompt_wire(&mut draft, controller, commit);
            }
            render_wire_preview(&mut canvas_ui, canvas, &draft, controller);
            render_selection_box(&mut canvas_ui, controller);
            navigation::render_canvas_navigation_hint(&canvas_ui, canvas, language);
        });
    controller.sync_branch_filters(&draft.graph);
    controller.draft = Some(draft);
}

fn render_provider_tabs(controller: &mut PromptStudioController, ui: &mut egui::Ui) {
    let tabs: &[(PromptProviderTarget, &str)] = match controller.domain() {
        xrtranslate_prompt::PromptGraphDomain::Translation => &[
            (PromptProviderTarget::OpenAiCompatible, "OPENAI"),
            (PromptProviderTarget::Hunyuan, "HUNYUAN"),
        ],
        xrtranslate_prompt::PromptGraphDomain::Asr => &[
            (PromptProviderTarget::AsrInstruction, "ASR INSTRUCTION"),
            (PromptProviderTarget::AsrContextBias, "ASR CONTEXT BIAS"),
        ],
    };
    for &(target, label) in tabs {
        if style::provider_tab(ui, label, controller.active_provider == target).clicked() {
            controller.select_provider(target);
        }
    }
}

fn render_branch_filters(
    graph: &PromptNodeGraph,
    controller: &mut PromptStudioController,
    ui: &mut egui::Ui,
    language: crate::i18n::UiLanguage,
) {
    let conditions = controller.branch_conditions();
    let text_filters = controller.text_branch_filters(graph);
    if conditions.is_empty() && text_filters.is_empty() {
        return;
    }
    ui.label(
        RichText::new(crate::i18n::tr(language, "VIEW /"))
            .font(egui::FontId::monospace(9.5))
            .color(style::MUTED),
    );
    for condition in conditions {
        let selected = controller.branch_filter(condition);
        let (label, options) = branch_filter_definition(condition);
        ui.label(
            RichText::new(crate::i18n::tr(language, label))
                .font(egui::FontId::monospace(9.0))
                .color(style::MUTED),
        );
        let selected_branch_text = crate::i18n::tr(
            language,
            options
                .iter()
                .find_map(|(label, value)| (*value == selected).then_some(*label))
                .unwrap_or("All"),
        );
        crate::ui::components::combobox_ui_with_width(
            ui,
            (
                "prompt_branch_filter",
                controller.active_provider,
                condition,
            ),
            selected_branch_text,
            Some(112.0),
            |ui| {
                for (option_label, value) in options {
                    if ui
                        .selectable_label(selected == value, crate::i18n::tr(language, option_label))
                        .clicked()
                    {
                        controller.set_branch_filter(graph, condition, value);
                    }
                }
            },
        );
    }
    for (source_id, label, cases) in text_filters {
        let selected = controller.text_branch_filter(&source_id).map(str::to_owned);
        ui.label(
            RichText::new(crate::i18n::tr_dynamic(language, &label))
                .font(egui::FontId::monospace(9.0))
                .color(style::MUTED),
        );
        let selected_text_filter = selected
            .as_deref()
            .map(display_text_case)
            .unwrap_or_else(|| crate::i18n::tr(language, "All").to_owned());
        crate::ui::components::combobox_ui_with_width(
            ui,
            (
                "prompt_text_branch_filter",
                controller.active_provider,
                &source_id,
            ),
            selected_text_filter,
            Some(112.0),
            |ui| {
                if ui
                    .selectable_label(selected.is_none(), crate::i18n::tr(language, "All"))
                    .clicked()
                {
                    controller.set_text_branch_filter(graph, &source_id, None);
                }
                for case in cases {
                    if ui
                        .selectable_label(
                            selected.as_deref() == Some(&case),
                            display_text_case(&case),
                        )
                        .clicked()
                    {
                        controller.set_text_branch_filter(graph, &source_id, Some(case));
                    }
                }
            },
        );
    }
}

fn display_text_case(value: &str) -> String {
    value
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().collect::<String>() + chars.as_str()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn branch_filter_definition(
    condition: PromptCondition,
) -> (&'static str, [(&'static str, Option<bool>); 3]) {
    match condition {
        PromptCondition::IsPseudoStreaming => (
            "MODE",
            [
                ("All", None),
                ("Ordinary", Some(false)),
                ("Pseudo-streaming", Some(true)),
            ],
        ),
        PromptCondition::SourceIsAuto => (
            "SOURCE",
            [
                ("All", None),
                ("Explicit", Some(false)),
                ("Auto", Some(true)),
            ],
        ),
        PromptCondition::HasReferenceContext => (
            "REFERENCE CONTEXT",
            [
                ("All", None),
                ("No context", Some(false)),
                ("With context", Some(true)),
            ],
        ),
        PromptCondition::HasRecognitionContext => (
            "RECOGNITION CONTEXT",
            [
                ("All", None),
                ("No context", Some(false)),
                ("With context", Some(true)),
            ],
        ),
    }
}

fn render_node_toolbar(
    draft: &mut PromptTemplateProfile,
    controller: &mut PromptStudioController,
    ui: &mut egui::Ui,
    language: crate::i18n::UiLanguage,
) {
    ui.menu_button(crate::i18n::tr(language, "+ ADD NODE"), |ui| {
        render_node_menu(draft, controller, ui, language, None);
    });
}

fn render_node_menu(
    draft: &mut PromptTemplateProfile,
    controller: &mut PromptStudioController,
    ui: &mut egui::Ui,
    language: crate::i18n::UiLanguage,
    preferred_center: Option<[f32; 2]>,
) {
    let page = PromptNodePage::for_target(controller.active_provider);
    ui.menu_button(crate::i18n::tr(language, "Input"), |ui| {
        ui.label(
            RichText::new(crate::i18n::tr(language, "RUNTIME VALUES"))
                .small()
                .strong(),
        );
        let variables: &[(&str, PromptVariable)] = match controller.domain() {
            xrtranslate_prompt::PromptGraphDomain::Translation => &[
                ("Source language", PromptVariable::SourceLanguage),
                ("Target language", PromptVariable::TargetLanguage),
                ("Current input", PromptVariable::CurrentInput),
                ("Recognition mode", PromptVariable::RecognitionMode),
            ],
            xrtranslate_prompt::PromptGraphDomain::Asr => &[
                ("Source language", PromptVariable::SourceLanguage),
                ("Expected languages", PromptVariable::TargetLanguage),
                ("Recognition context", PromptVariable::RecognitionContext),
                ("Recognition mode", PromptVariable::RecognitionMode),
            ],
        };
        for &(label, variable) in variables {
            if ui.button(crate::i18n::tr(language, label)).clicked() {
                let position = node_add_position(controller, &draft.graph, preferred_center);
                let before = draft.clone();
                let id = draft.graph.add_variable(page, variable, position);
                finish_node_add(draft, controller, before, id);
                ui.close();
            }
        }
        if controller.domain() == xrtranslate_prompt::PromptGraphDomain::Translation {
            ui.separator();
            ui.label(
                RichText::new(crate::i18n::tr(language, "REFERENCE DATA"))
                    .small()
                    .strong(),
            );
            for (label, block) in available_blocks() {
                if ui.button(crate::i18n::tr(language, label)).clicked() {
                    let position = node_add_position(controller, &draft.graph, preferred_center);
                    let before = draft.clone();
                    let value = match block {
                        TranslationPromptBlock::LanguageOrder => PromptSystemValue::LanguageOrder,
                        TranslationPromptBlock::Terminology => PromptSystemValue::Terminology,
                        TranslationPromptBlock::RecentTurns { limit } => {
                            PromptSystemValue::RecentTurns { limit }
                        }
                        TranslationPromptBlock::PreviousRevision => {
                            PromptSystemValue::PreviousRevision
                        }
                        TranslationPromptBlock::SurroundingSource => {
                            PromptSystemValue::SurroundingSource
                        }
                        TranslationPromptBlock::CustomText { .. } => unreachable!(),
                    };
                    let id = draft.graph.add_system_value(page, value, position);
                    finish_node_add(draft, controller, before, id);
                    ui.close();
                }
            }
        }
    });
    ui.menu_button(crate::i18n::tr(language, "Logic"), |ui| {
        let conditions: &[(&str, PromptCondition)] = match controller.domain() {
            xrtranslate_prompt::PromptGraphDomain::Translation => &[(
                "Has reference context",
                PromptCondition::HasReferenceContext,
            )],
            xrtranslate_prompt::PromptGraphDomain::Asr => &[(
                "Has recognition context",
                PromptCondition::HasRecognitionContext,
            )],
        };
        ui.label(
            RichText::new(crate::i18n::tr(language, "HOST CONDITIONS"))
                .small()
                .strong(),
        );
        for &(label, condition) in conditions {
            if ui.button(crate::i18n::tr(language, label)).clicked() {
                let position = node_add_position(controller, &draft.graph, preferred_center);
                let before = draft.clone();
                let id = draft.graph.add_condition_value(page, condition, position);
                finish_node_add(draft, controller, before, id);
                ui.close();
            }
        }
        ui.separator();
        ui.label(
            RichText::new(crate::i18n::tr(language, "USER / DERIVED VALUES"))
                .small()
                .strong(),
        );
        if ui
            .button(crate::i18n::tr(language, "Boolean value"))
            .clicked()
        {
            let position = node_add_position(controller, &draft.graph, preferred_center);
            let before = draft.clone();
            let id = draft.graph.add_boolean_value(page, false, position);
            finish_node_add(draft, controller, before, id);
            ui.close();
        }
        if ui
            .button(crate::i18n::tr(language, "Text comparison"))
            .clicked()
        {
            let position = node_add_position(controller, &draft.graph, preferred_center);
            let before = draft.clone();
            let id = draft.graph.add_text_comparison(
                page,
                PromptTextComparison::Equals,
                String::new(),
                false,
                position,
            );
            finish_node_add(draft, controller, before, id);
            ui.close();
        }
        if ui
            .button(crate::i18n::tr(language, "Text branch selector"))
            .on_hover_text(crate::i18n::tr(
                language,
                "Connect finite text such as Recognition mode; its possible outputs become named branches",
            ))
            .clicked()
        {
            let position = node_add_position(controller, &draft.graph, preferred_center);
            let before = draft.clone();
            let id = draft.graph.add_text_switch(page, position);
            finish_node_add(draft, controller, before, id);
            ui.close();
        }
        ui.separator();
        if ui
            .button(crate::i18n::tr(language, "Conditional switch"))
            .clicked()
        {
            let position = node_add_position(controller, &draft.graph, preferred_center);
            let before = draft.clone();
            let id = draft.graph.add_conditional_switch(page, position);
            finish_node_add(draft, controller, before, id);
            ui.close();
        }
    });
    ui.menu_button(crate::i18n::tr(language, "Compose"), |ui| {
        if ui
            .button(crate::i18n::tr(language, "Compose"))
            .on_hover_text(crate::i18n::tr(
                language,
                "Arrange fixed text and connected {0}-{4} input slots",
            ))
            .clicked()
        {
            let position = node_add_position(controller, &draft.graph, preferred_center);
            let before = draft.clone();
            let id = draft
                .graph
                .add_compose(page, "Write prompt text here: {0}".into(), position);
            finish_node_add(draft, controller, before, id);
            ui.close();
        }
    });
    ui.menu_button(crate::i18n::tr(language, "Request"), |ui| {
        let target = controller.active_provider;
        let exists = draft.graph.nodes.iter().any(
            |node| matches!(node.kind, PromptNodeKind::Request { target: value, .. } if value == target),
        );
        let (label, roles) = match target {
            PromptProviderTarget::OpenAiCompatible => (
                "OpenAI request",
                vec![PromptMessageRole::System, PromptMessageRole::User],
            ),
            PromptProviderTarget::Hunyuan => {
                ("Hunyuan request", vec![PromptMessageRole::User])
            }
            PromptProviderTarget::AsrInstruction => {
                ("ASR prompt request", vec![PromptMessageRole::System])
            }
            PromptProviderTarget::AsrContextBias => {
                ("ASR context request", vec![PromptMessageRole::User])
            }
        };
        if ui
            .add_enabled(!exists, egui::Button::new(crate::i18n::tr(language, label)))
            .on_disabled_hover_text(crate::i18n::tr(language, "This provider page already has its API request"))
            .clicked()
        {
            let position = node_add_position(controller, &draft.graph, preferred_center);
            let before = draft.clone();
            let id = draft.graph.add_request(target, roles, position);
            finish_node_add(draft, controller, before, id);
            ui.close();
        }
    });
}

fn node_add_position(
    controller: &PromptStudioController,
    graph: &PromptNodeGraph,
    preferred_center: Option<[f32; 2]>,
) -> [f32; 2] {
    preferred_center.map_or_else(
        || controller.new_node_position(graph),
        |center| controller.new_node_position_near(graph, center),
    )
}

fn finish_node_add(
    draft: &mut PromptTemplateProfile,
    controller: &mut PromptStudioController,
    before: PromptTemplateProfile,
    id: String,
) {
    draft.graph.layout_version = 0;
    controller.select_node(id, false);
    controller.push_history(before);
}

fn render_links(
    ui: &mut egui::Ui,
    canvas: Rect,
    profile: &mut PromptTemplateProfile,
    controller: &mut PromptStudioController,
    editable: bool,
) {
    let links = profile.graph.links.clone();
    let mut remove_link = None;

    let hovered_link_idx = ui.ctx().pointer_hover_pos().and_then(|pointer| {
        crate::ui::graph_editor::closest_link(
            pointer,
            links.iter().enumerate().filter_map(|(index, link)| {
                let endpoints_visible = profile
                    .graph
                    .nodes
                    .iter()
                    .filter(|node| node.id == link.from || node.id == link.to)
                    .all(|node| controller.node_is_visible(node));
                if !endpoints_visible {
                    return None;
                }
                let Some((from, to)) = link_points(canvas, controller, &profile.graph, link) else {
                    return None;
                };
                Some((index, graph_canvas::bezier_points(from, to)))
            }),
            12.0,
        )
    });

    if editable {
        if let Some(idx) = hovered_link_idx {
            let link = &links[idx];
            let link_key = PromptLinkKey {
                from: link.from.clone(),
                to: link.to.clone(),
                input: link.input,
            };

            if ui.input(|i| i.pointer.primary_clicked()) {
                let extend = ui.input(|i| i.modifiers.shift || i.modifiers.ctrl);
                controller.select_link(link_key, extend);
            }

            if ui.input(|i| i.pointer.secondary_clicked())
                && !controller.secondary_action_suppressed()
            {
                remove_link = Some(idx);
            }
        }
    }

    for (index, link) in links.iter().enumerate() {
        let endpoints_visible = profile
            .graph
            .nodes
            .iter()
            .filter(|node| node.id == link.from || node.id == link.to)
            .all(|node| controller.node_is_visible(node));
        if !endpoints_visible {
            continue;
        }
        let Some((from, to)) = link_points(canvas, controller, &profile.graph, link) else {
            continue;
        };
        let points = graph_canvas::bezier_points(from, to);
        let link_key = PromptLinkKey {
            from: link.from.clone(),
            to: link.to.clone(),
            input: link.input,
        };
        let is_selected = controller.selected_links.contains(&link_key);
        let is_hovered = hovered_link_idx == Some(index);

        let source_color = profile
            .graph
            .nodes
            .iter()
            .find(|node| node.id == link.from)
            .map(|node| node_palette(&node.kind).connector)
            .unwrap_or(GRAPH_ACCENT);

        let (wire_color, stroke_width) = if is_selected {
            (style::LINK_SELECTED, 3.2)
        } else if is_hovered {
            (style::GRAPH_ACCENT, 2.6)
        } else {
            (
                Color32::from_rgba_unmultiplied(
                    source_color.r(),
                    source_color.g(),
                    source_color.b(),
                    178,
                ),
                1.6,
            )
        };

        graph_canvas::paint_wire(ui, points, Stroke::new(stroke_width, wire_color));
    }

    if hovered_link_idx.is_some() {
        if let Some(pointer) = ui.ctx().pointer_hover_pos() {
            ui.interact(
                Rect::from_center_size(pointer, Vec2::splat(16.0)),
                ui.make_persistent_id("hovered_link_tooltip"),
                Sense::hover(),
            )
            .on_hover_text(if editable {
                "Click to select · Press Del to delete · Right-click to disconnect"
            } else {
                "Connection wire"
            });
        }
    }

    if let Some(index) = remove_link {
        let before = profile.clone();
        let removed = profile.graph.links.remove(index);
        controller.selected_links.remove(&PromptLinkKey {
            from: removed.from,
            to: removed.to,
            input: removed.input,
        });
        controller.push_history(before);
    }
}

fn render_nodes(
    ui: &mut egui::Ui,
    canvas: Rect,
    profile: &mut PromptTemplateProfile,
    controller: &mut PromptStudioController,
    editable: bool,
    runtime_trace: Option<&PromptExecutionTrace>,
    error_target: &GraphErrorTarget,
    language: crate::i18n::UiLanguage,
) {
    let nodes = profile
        .graph
        .nodes
        .iter()
        .filter(|node| controller.node_is_visible(node))
        .cloned()
        .collect::<Vec<_>>();
    let mut remove_id = None;
    for node in nodes {
        let display_position = controller.display_position(&node.id, node.position);
        let rect = controller.canvas.graph_rect(
            canvas,
            display_position,
            node_size(&profile.graph, &node),
        );
        let header = Rect::from_min_size(
            rect.min,
            Vec2::new(rect.width(), NODE_HEADER_HEIGHT * node_scale(rect, &node)),
        );
        let response = ui.interact(
            header,
            ui.make_persistent_id(("prompt_node", &node.id)),
            if editable {
                Sense::click_and_drag()
            } else {
                Sense::hover()
            },
        );
        let response = if matches!(node.kind, PromptNodeKind::Request { .. }) {
            let preview = profile
                .graph
                .compose_request_preview(&node.id)
                .unwrap_or_else(|| "(no connected messages)".into());
            response.on_hover_text(format!(
                "API REQUEST PREVIEW\n\n{}",
                truncate_preview(&preview, 1200)
            ))
        } else if editable {
            response.on_hover_text(crate::i18n::tr(
                language,
                "Drag header to move · Double-click to rename",
            ))
        } else {
            response
        };
        if editable && response.double_clicked() {
            let initial = if node.label.trim().is_empty() || node.label == "COMPOSE TEXT" {
                node_display_label(&node)
            } else {
                node.label.clone()
            };
            controller.start_editing_title(node.id.clone(), initial, profile.clone());
        } else if editable && response.clicked() {
            let extend = ui.input(|input| input.modifiers.shift || input.modifiers.ctrl);
            controller.select_node(node.id.clone(), extend);
        }
        if editable && response.drag_started() {
            let positions = profile
                .graph
                .nodes
                .iter()
                .map(|node| (node.id.clone(), node.position))
                .collect::<Vec<_>>();
            controller.begin_node_drag(node.id.clone(), positions);
            controller.drag_start_profile = Some(profile.clone());
        }
        if editable
            && response.dragged()
            && controller.drag_node.as_deref() == Some(node.id.as_str())
        {
            controller.update_node_drag(response.drag_delta());
            let movements = controller
                .drag_origins
                .iter()
                .map(|(node_id, origin)| crate::ui::graph_editor::NodeMove {
                    node_id: node_id.clone(),
                    position: controller.display_position(node_id, *origin),
                })
                .collect::<Vec<_>>();
            for movement in movements {
                if let Some(target) = profile
                    .graph
                    .nodes
                    .iter_mut()
                    .find(|target| target.id == movement.node_id)
                {
                    target.position = movement.position;
                }
            }
            if response.drag_delta().length_sq() > f32::EPSILON {
                controller.mark_dirty();
            }
        }
        if editable
            && response.drag_stopped()
            && controller.drag_node.as_deref() == Some(node.id.as_str())
        {
            let movements = controller.finish_node_drag(Some(16.0));
            for movement in movements {
                if let Some(target) = profile
                    .graph
                    .nodes
                    .iter_mut()
                    .find(|target| target.id == movement.node_id)
                {
                    target.position = movement.position;
                }
            }
            if let Some(before) = controller.drag_start_profile.take() {
                if before.graph.nodes != profile.graph.nodes {
                    controller.push_history(before);
                }
            }
        }
        let scale = node_scale(rect, &node);
        let close_rect = Rect::from_center_size(
            Pos2::new(rect.right() - 13.0 * scale, rect.top() + 13.0 * scale),
            Vec2::splat((22.0 * scale).max(14.0)),
        );
        if editable
            && ui
                .interact(
                    close_rect,
                    ui.make_persistent_id(("prompt_node_remove", &node.id)),
                    Sense::click(),
                )
                .clicked()
        {
            remove_id = Some(node.id.clone());
        }
        let selected = controller.selected_nodes.contains(&node.id);
        draw_node(
            ui,
            rect,
            &node,
            profile,
            controller,
            editable,
            selected,
            runtime_trace,
            error_target,
            language,
        );
        render_node_sockets(
            ui,
            rect,
            &node,
            profile,
            controller,
            editable,
            error_target,
            language,
        );
    }
    if let Some(id) = remove_id {
        let before = profile.clone();
        profile.graph.remove_node(&id);
        controller.selected_nodes.remove(&id);
        controller.push_history(before);
    }
}

fn draw_node(
    ui: &mut egui::Ui,
    rect: Rect,
    node: &PromptNode,
    profile: &mut PromptTemplateProfile,
    controller: &mut PromptStudioController,
    editable: bool,
    selected: bool,
    runtime_trace: Option<&PromptExecutionTrace>,
    error_target: &GraphErrorTarget,
    language: crate::i18n::UiLanguage,
) {
    let scale = node_scale(rect, node);
    let header_height = NODE_HEADER_HEIGHT * scale;
    let palette = node_palette(&node.kind);
    let title = crate::i18n::tr_dynamic(language, &node_display_label(node)).into_owned();
    let is_error = error_target.node_id.as_deref() == Some(node.id.as_str());

    ui.painter()
        .rect_filled(rect, CornerRadius::same(2), palette.fill);
    let (border_stroke, border_color) = if selected {
        (2.0, GRAPH_ACCENT)
    } else if is_error {
        (2.0, style::ERROR_BORDER)
    } else {
        (1.0, style::NODE_BORDER)
    };
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(2),
        Stroke::new(border_stroke, border_color),
        egui::epaint::StrokeKind::Inside,
    );
    ui.painter().rect_filled(
        Rect::from_min_size(rect.min, Vec2::new(rect.width(), header_height)),
        CornerRadius::same(2),
        if is_error {
            style::ERROR_FILL
        } else {
            palette.header
        },
    );
    let show_kind = scale >= 0.72;
    let kind_tag =
        crate::i18n::tr_dynamic(language, &node_kind_tag(&profile.graph, node)).into_owned();
    let title_font_size = (9.5 * scale).max(7.0);
    let kind_font_size = 7.5 * scale;
    let kind_width = if show_kind {
        kind_tag.chars().count() as f32 * kind_font_size * 0.62 + 10.0 * scale
    } else {
        0.0
    };
    let close_width = if editable { 22.0 * scale } else { 0.0 };
    let title_width =
        (rect.width() - 18.0 * scale - kind_width - close_width).max(title_font_size * 8.0);
    let title_chars = (title_width / (title_font_size * 0.62)).floor() as usize;

    let is_renaming = editable
        && controller
            .editing_title
            .as_ref()
            .is_some_and(|edit| edit.node_id == node.id);

    if is_renaming {
        let mut committed_text = None;
        let mut cancelled = false;
        if let Some(edit) = &mut controller.editing_title {
            let title_rect = Rect::from_min_size(
                Pos2::new(rect.left() + 7.0 * scale, rect.top() + 2.0 * scale),
                Vec2::new(title_width, (header_height - 4.0 * scale).max(14.0)),
            );
            ui.painter().rect_filled(
                title_rect.expand(1.0),
                CornerRadius::same(1),
                Color32::WHITE,
            );
            ui.painter().rect_stroke(
                title_rect.expand(1.0),
                CornerRadius::same(1),
                Stroke::new(1.0, GRAPH_ACCENT),
                egui::epaint::StrokeKind::Inside,
            );
            let edit_response = ui.put(
                title_rect,
                egui::TextEdit::singleline(&mut edit.text)
                    .font(egui::FontId::monospace(title_font_size))
                    .text_color(style::NODE_TEXT)
                    .desired_width(title_width)
                    .frame(egui::Frame::NONE),
            );
            edit_response.request_focus();

            let enter_pressed = ui.input(|i| i.key_pressed(egui::Key::Enter));
            let escape_pressed = ui.input(|i| i.key_pressed(egui::Key::Escape));
            let lost_focus = edit_response.lost_focus();

            if escape_pressed {
                cancelled = true;
            } else if enter_pressed || lost_focus {
                committed_text = Some(edit.text.trim().to_string());
            }
        }
        if cancelled {
            controller.cancel_editing_title();
        } else if let Some(new_text) = committed_text {
            let before = controller.title_edit_start_profile.take();
            if let Some(actual) = profile
                .graph
                .nodes
                .iter_mut()
                .find(|actual| actual.id == node.id)
            {
                if actual.label != new_text {
                    actual.label = new_text;
                    if let Some(before_profile) = before {
                        controller.push_history(before_profile);
                    }
                }
            }
            controller.editing_title = None;
        }
    } else {
        ui.painter().text(
            Pos2::new(rect.left() + 10.0 * scale, rect.top() + 8.0 * scale),
            egui::Align2::LEFT_TOP,
            truncate_preview(&title, title_chars),
            egui::FontId::monospace(title_font_size),
            if is_error {
                style::ERROR_BORDER
            } else {
                style::NODE_TEXT
            },
        );
    }
    if show_kind {
        ui.painter().text(
            Pos2::new(
                rect.right() - (if editable { 28.0 } else { 8.0 }) * scale,
                rect.top() + 9.0 * scale,
            ),
            egui::Align2::RIGHT_TOP,
            kind_tag,
            egui::FontId::monospace(kind_font_size),
            style::NODE_MUTED,
        );
    }
    if editable {
        ui.painter().text(
            Pos2::new(rect.right() - 8.0 * scale, rect.top() + 8.0 * scale),
            egui::Align2::RIGHT_TOP,
            "×",
            egui::FontId::monospace(12.0 * scale),
            style::NODE_TEXT,
        );
    }
    if scale < 0.58 {
        return;
    }
    match &node.kind {
        PromptNodeKind::Input {
            block: TranslationPromptBlock::CustomText { .. },
        } if editable => {
            let before = profile.graph.clone();
            let mut text_changed = false;
            if let Some(actual) = profile
                .graph
                .nodes
                .iter_mut()
                .find(|actual| actual.id == node.id)
            {
                if let PromptNodeKind::Input {
                    block: TranslationPromptBlock::CustomText { text },
                } = &mut actual.kind
                {
                    let body = Rect::from_min_max(
                        Pos2::new(rect.left() + 9.0 * scale, rect.top() + 34.0 * scale),
                        Pos2::new(
                            runtime_preview::configuration_right(rect, scale),
                            rect.bottom() - 6.0 * scale,
                        ),
                    );
                    ui.scope_builder(
                        UiBuilder::new()
                            .max_rect(body)
                            .layout(Layout::top_down(Align::Min)),
                        |ui| {
                            ui.set_clip_rect(ui.clip_rect().intersect(body));
                            if ui
                                .add(
                                    egui::TextEdit::multiline(text)
                                        .font(egui::FontId::monospace(10.0 * scale))
                                        .text_color(style::NODE_TEXT)
                                        .desired_rows(
                                            ((node.layout_height() - 54.0) / 13.0).floor().max(3.0)
                                                as usize,
                                        )
                                        .frame(egui::Frame::NONE),
                                )
                                .changed()
                            {
                                text_changed = true;
                            }
                        },
                    );
                }
            }
            if text_changed {
                profile.graph.sync_text_switch_cases(&before);
                profile.graph.layout_version = 0;
                controller.mark_dirty();
            }
        }
        PromptNodeKind::Input { .. } => {
            ui.painter().text(
                Pos2::new(rect.left() + 10.0 * scale, rect.top() + 47.0 * scale),
                egui::Align2::LEFT_TOP,
                input_description(&node.kind),
                egui::FontId::monospace(10.0 * scale),
                style::NODE_TEXT,
            );
        }
        PromptNodeKind::Variable { variable } => {
            ui.painter().text(
                Pos2::new(rect.left() + 10.0 * scale, rect.top() + 47.0 * scale),
                egui::Align2::LEFT_TOP,
                format!("[{}]", variable_name(*variable)),
                egui::FontId::monospace(10.0 * scale),
                style::NODE_TEXT,
            );
        }
        PromptNodeKind::SystemValue { value } => {
            let possible = profile
                .graph
                .possible_text_outputs(&node.id)
                .map(|values| format!("\nPOSSIBLE\n{}", values.join(" | ")))
                .unwrap_or_default();
            ui.painter().text(
                Pos2::new(rect.left() + 10.0 * scale, rect.top() + 47.0 * scale),
                egui::Align2::LEFT_TOP,
                format!("HOST INPUT\n[{}]{possible}", system_value_label(*value)),
                egui::FontId::monospace(10.0 * scale),
                style::NODE_TEXT,
            );
        }
        PromptNodeKind::ConditionValue { condition } => {
            ui.painter().text(
                Pos2::new(rect.left() + 10.0 * scale, rect.top() + 47.0 * scale),
                egui::Align2::LEFT_TOP,
                format!("HOST CONDITION\n{}", condition_expression(*condition)),
                egui::FontId::monospace(9.0 * scale),
                style::NODE_TEXT,
            );
        }
        PromptNodeKind::BoolValue { .. } if editable => {
            let before = profile.clone();
            let mut changed = false;
            if let Some(PromptNode {
                kind: PromptNodeKind::BoolValue { value },
                ..
            }) = profile
                .graph
                .nodes
                .iter_mut()
                .find(|actual| actual.id == node.id)
            {
                let body = Rect::from_min_max(
                    Pos2::new(rect.left() + 10.0 * scale, rect.top() + 42.0 * scale),
                    Pos2::new(
                        runtime_preview::configuration_right(rect, scale),
                        rect.bottom(),
                    ),
                );
                changed = ui
                    .put(
                        body,
                        egui::Checkbox::new(value, if *value { "TRUE" } else { "FALSE" }),
                    )
                    .changed();
            }
            if changed {
                controller.push_history(before);
            }
        }
        PromptNodeKind::BoolValue { value } => {
            ui.painter().text(
                Pos2::new(rect.left() + 10.0 * scale, rect.top() + 47.0 * scale),
                egui::Align2::LEFT_TOP,
                if *value { "TRUE" } else { "FALSE" },
                egui::FontId::monospace(10.0 * scale),
                style::NODE_TEXT,
            );
        }
        PromptNodeKind::TextComparison { .. } if editable => {
            let before = profile.clone();
            let Some(PromptNode {
                kind:
                    PromptNodeKind::TextComparison {
                        operator,
                        expected,
                        case_sensitive,
                    },
                ..
            }) = profile
                .graph
                .nodes
                .iter_mut()
                .find(|actual| actual.id == node.id)
            else {
                return;
            };
            let content_right = runtime_preview::configuration_right(rect, scale);
            let operator_rect = Rect::from_min_size(
                Pos2::new(rect.left() + 22.0 * scale, rect.top() + 39.0 * scale),
                Vec2::new(
                    (content_right - rect.left() - 28.0 * scale).max(80.0),
                    24.0 * scale,
                ),
            );
            let mut discrete_changed = false;
            ui.scope_builder(UiBuilder::new().max_rect(operator_rect), |ui| {
                crate::ui::components::combobox_ui(
                    ui,
                    ("prompt_text_comparison", &node.id),
                    format!("{operator:?}"),
                    |ui| {
                        for option in [
                            PromptTextComparison::Equals,
                            PromptTextComparison::NotEquals,
                            PromptTextComparison::Contains,
                            PromptTextComparison::StartsWith,
                            PromptTextComparison::EndsWith,
                        ] {
                            discrete_changed |= ui
                                .selectable_value(operator, option, format!("{option:?}"))
                                .changed();
                        }
                    },
                );
            });
            let expected_rect = Rect::from_min_size(
                Pos2::new(rect.left() + 22.0 * scale, rect.top() + 70.0 * scale),
                Vec2::new(
                    (content_right - rect.left() - 28.0 * scale).max(80.0),
                    24.0 * scale,
                ),
            );
            let response = ui.scope_builder(UiBuilder::new().max_rect(expected_rect), |ui| {
                crate::ui::components::text_edit_ui(
                    ui,
                    ("prompt_node_expected", &node.id),
                    egui::TextEdit::singleline(expected)
                        .hint_text("Expected text")
                        .font(egui::FontId::monospace(9.0 * scale)),
                )
            }).inner;
            if response.gained_focus() && controller.text_edit_start_profile.is_none() {
                controller.text_edit_start_profile = Some(before.clone());
            }
            if response.changed() {
                controller.mark_dirty();
            }
            if response.lost_focus()
                && let Some(edit_start) = controller.text_edit_start_profile.take()
            {
                controller.push_history(edit_start);
            }
            let case_rect = Rect::from_min_size(
                Pos2::new(rect.left() + 22.0 * scale, rect.top() + 101.0 * scale),
                Vec2::new(
                    (content_right - rect.left() - 28.0 * scale).max(80.0),
                    22.0 * scale,
                ),
            );
            discrete_changed |= ui
                .put(
                    case_rect,
                    egui::Checkbox::new(case_sensitive, "Case sensitive"),
                )
                .changed();
            if discrete_changed {
                controller.push_history(before);
            }
        }
        PromptNodeKind::TextComparison {
            operator,
            expected,
            case_sensitive,
        } => {
            ui.painter().text(
                Pos2::new(rect.left() + 20.0 * scale, rect.top() + 47.0 * scale),
                egui::Align2::LEFT_TOP,
                format!(
                    "{operator:?} {expected:?}\n{}",
                    if *case_sensitive {
                        "CASE SENSITIVE"
                    } else {
                        "IGNORE CASE"
                    }
                ),
                egui::FontId::monospace(9.0 * scale),
                style::NODE_TEXT,
            );
        }
        PromptNodeKind::Compose { .. } if editable => {
            let mut changed = false;
            if let Some(actual) = profile
                .graph
                .nodes
                .iter_mut()
                .find(|actual| actual.id == node.id)
            {
                if let PromptNodeKind::Compose { text } = &mut actual.kind {
                    let body = Rect::from_min_max(
                        Pos2::new(rect.left() + 30.0 * scale, rect.top() + 34.0 * scale),
                        Pos2::new(
                            runtime_preview::configuration_right(rect, scale),
                            rect.bottom() - 6.0 * scale,
                        ),
                    );
                    ui.scope_builder(
                        UiBuilder::new()
                            .max_rect(body)
                            .layout(Layout::top_down(Align::Min)),
                        |ui| {
                            ui.set_clip_rect(ui.clip_rect().intersect(body));
                            if ui
                                .add(
                                    egui::TextEdit::multiline(text)
                                        .font(egui::FontId::monospace(10.0 * scale))
                                        .text_color(style::NODE_TEXT)
                                        .desired_rows(
                                            ((node.layout_height() - 54.0) / 13.0).floor().max(5.0)
                                                as usize,
                                        )
                                        .frame(egui::Frame::NONE),
                                )
                                .changed()
                            {
                                changed = true;
                            }
                        },
                    );
                }
            }
            if changed {
                let inputs = profile
                    .graph
                    .nodes
                    .iter()
                    .find(|actual| actual.id == node.id)
                    .and_then(|actual| match &actual.kind {
                        PromptNodeKind::Compose { text } => compose_input_indexes(text).ok(),
                        _ => None,
                    })
                    .unwrap_or_default();
                profile
                    .graph
                    .links
                    .retain(|link| link.to != node.id || inputs.contains(&link.input));
                profile.graph.layout_version = 0;
                controller.mark_dirty();
            }
        }
        PromptNodeKind::Compose { text } => {
            let body = Rect::from_min_max(
                Pos2::new(rect.left() + 30.0 * scale, rect.top() + 35.0 * scale),
                Pos2::new(
                    runtime_preview::configuration_right(rect, scale),
                    rect.bottom() - 6.0 * scale,
                ),
            );
            ui.scope_builder(
                UiBuilder::new()
                    .max_rect(body)
                    .layout(Layout::top_down(Align::Min)),
                |ui| {
                    ui.set_clip_rect(ui.clip_rect().intersect(body));
                    ui.label(
                        RichText::new(text.as_str())
                            .font(egui::FontId::monospace(10.0 * scale))
                            .color(style::NODE_TEXT),
                    );
                },
            );
        }
        PromptNodeKind::Switch { .. } => {
            ui.painter().text(
                Pos2::new(rect.left() + 18.0 * scale, rect.bottom() - 18.0 * scale),
                egui::Align2::LEFT_BOTTOM,
                "Select FALSE / TRUE from connected BOOL",
                egui::FontId::monospace(9.0 * scale),
                style::NODE_TEXT,
            );
        }
        PromptNodeKind::TextSwitch => {
            let cases = profile.graph.text_switch_cases(&node.id);
            ui.painter().text(
                Pos2::new(rect.left() + 18.0 * scale, rect.bottom() - 18.0 * scale),
                egui::Align2::LEFT_BOTTOM,
                cases.map_or_else(
                    || "No named branches: selector has no finite values".into(),
                    |cases| format!("{} named text branches", cases.len()),
                ),
                egui::FontId::monospace(9.0 * scale),
                style::NODE_TEXT,
            );
        }
        PromptNodeKind::Request { roles, .. } => {
            ui.painter().text(
                Pos2::new(rect.left() + 12.0 * scale, rect.bottom() - 12.0 * scale),
                egui::Align2::LEFT_BOTTOM,
                request_summary(roles.len()),
                egui::FontId::monospace(9.0 * scale),
                style::NODE_TEXT,
            );
        }
    }
    runtime_preview::render(
        ui,
        rect,
        &profile.graph,
        node,
        runtime_trace,
        scale,
        language,
    );
}

fn commit_prompt_wire(
    profile: &mut PromptTemplateProfile,
    controller: &mut PromptStudioController,
    commit: crate::ui::graph_editor::WireCommit<String, (String, u8), PromptLinkKey>,
) {
    let before = profile.clone();
    if let Some(replaced) = &commit.replaced {
        profile.graph.links.retain(|link| {
            !(link.from == replaced.from && link.to == replaced.to && link.input == replaced.input)
        });
    }
    let connected = commit
        .to
        .as_ref()
        .is_some_and(|(target, input)| profile.graph.connect(&commit.from, target, *input));
    let disconnected = commit.to.is_none() && commit.replaced.is_some();
    if connected || disconnected {
        controller.push_history(before);
    } else if commit.replaced.is_some() {
        *profile = before;
    }
}

fn render_node_sockets(
    ui: &mut egui::Ui,
    rect: Rect,
    node: &PromptNode,
    profile: &mut PromptTemplateProfile,
    controller: &mut PromptStudioController,
    editable: bool,
    error_target: &GraphErrorTarget,
    language: crate::i18n::UiLanguage,
) {
    let scale = node_scale(rect, node);
    if !matches!(node.kind, PromptNodeKind::Request { .. }) {
        let output = socket_position(&profile.graph, rect, node, false, 0);
        let output_response = ui.interact(
            Rect::from_center_size(output, Vec2::splat(22.0)),
            ui.make_persistent_id(("prompt_output_socket", &node.id)),
            if editable {
                Sense::click_and_drag()
            } else {
                Sense::hover()
            },
        );
        if editable
            && let Some(commit) = controller.interact_output_port(&output_response, node.id.clone())
        {
            commit_prompt_wire(profile, controller, commit);
        }
        ui.painter().circle_filled(
            output,
            (SOCKET_RADIUS * scale).max(2.5),
            if controller.wire_from.as_deref() == Some(node.id.as_str()) {
                style::GRAPH_ACCENT
            } else {
                node_palette(&node.kind).connector
            },
        );
    }

    let inputs = input_socket_indexes(&profile.graph, node);
    if !inputs.is_empty() {
        for input in inputs {
            let position = socket_position(&profile.graph, rect, node, true, input);
            let connected_link = profile
                .graph
                .links
                .iter()
                .find(|link| link.to == node.id && link.input == input)
                .cloned();
            let is_error_socket = error_target.node_id.as_deref() == Some(node.id.as_str())
                && error_target.input_index == Some(input);

            let input_response = ui
                .interact(
                    Rect::from_center_size(position, Vec2::splat(22.0)),
                    ui.make_persistent_id(("prompt_input_socket", &node.id, input)),
                    if editable {
                        Sense::click_and_drag()
                    } else {
                        Sense::hover()
                    },
                )
                .on_hover_text(input_socket_tooltip(
                    &profile.graph,
                    node,
                    input,
                    error_target,
                ));

            if editable {
                let connected = connected_link.as_ref().map(|link| {
                    (
                        link.from.clone(),
                        PromptLinkKey {
                            from: link.from.clone(),
                            to: link.to.clone(),
                            input: link.input,
                        },
                    )
                });
                if let Some(commit) = controller.interact_input_port(
                    &input_response,
                    (node.id.clone(), input),
                    connected,
                ) {
                    commit_prompt_wire(profile, controller, commit);
                }
                if connected_link.is_some() {
                    if input_response.secondary_clicked()
                        && !controller.secondary_action_suppressed()
                    {
                        let before = profile.clone();
                        profile
                            .graph
                            .links
                            .retain(|l| !(l.to == node.id && l.input == input));
                        controller.push_history(before);
                    }
                } else if input_response.secondary_clicked()
                    && !controller.secondary_action_suppressed()
                {
                    let before = profile.clone();
                    let mut removed = false;
                    if let Some(actual) = profile
                        .graph
                        .nodes
                        .iter_mut()
                        .find(|actual| actual.id == node.id)
                    {
                        if let PromptNodeKind::Compose { text } = &mut actual.kind {
                            remove_compose_placeholder(text, input);
                            removed = true;
                        }
                    }
                    if removed {
                        profile.graph.layout_version = 0;
                        controller.push_history(before);
                    }
                }
            }

            let socket_color = if is_error_socket {
                style::ERROR_BORDER
            } else if connected_link.is_some() {
                style::NODE_TEXT
            } else {
                style::NODE_MUTED
            };

            ui.painter()
                .circle_filled(position, (SOCKET_RADIUS * scale).max(2.5), socket_color);

            if is_error_socket {
                ui.painter().circle_stroke(
                    position,
                    (SOCKET_RADIUS * scale + 3.0).max(5.0),
                    Stroke::new(1.5, style::ERROR_BORDER),
                );
            }

            if matches!(
                node.kind,
                PromptNodeKind::Compose { .. }
                    | PromptNodeKind::Switch { .. }
                    | PromptNodeKind::TextSwitch
                    | PromptNodeKind::TextComparison { .. }
                    | PromptNodeKind::Request { .. }
            ) {
                let label_color = if is_error_socket {
                    style::ERROR_BORDER
                } else {
                    style::NODE_TEXT
                };
                ui.painter().text(
                    Pos2::new(position.x + 12.0 * scale, position.y - 6.0 * scale),
                    egui::Align2::LEFT_TOP,
                    crate::i18n::tr_dynamic(
                        language,
                        &input_socket_label(&profile.graph, node, input),
                    ),
                    egui::FontId::monospace((9.0 * scale).max(6.5)),
                    label_color,
                );
            }
        }
    }
}

fn render_wire_preview(
    ui: &mut egui::Ui,
    canvas: Rect,
    profile: &PromptTemplateProfile,
    controller: &PromptStudioController,
) {
    if let Some(from_id) = controller.wire_from.as_deref() {
        let Some(node) = profile.graph.nodes.iter().find(|node| node.id == from_id) else {
            return;
        };
        let from = socket_position(
            &profile.graph,
            controller.canvas.graph_rect(
                canvas,
                controller.display_position(&node.id, node.position),
                node_size(&profile.graph, node),
            ),
            node,
            false,
            0,
        );
        let to = ui
            .ctx()
            .pointer_hover_pos()
            .map(|position| position.clamp(canvas.min, canvas.max))
            .unwrap_or_else(|| Pos2::new(from.x + 100.0, from.y));
        let points = graph_canvas::bezier_points(from, to);
        graph_canvas::paint_wire(ui, points, Stroke::new(2.0, style::LINK_SELECTED));
    } else if let Some((to_id, to_input)) = controller.wire_from_input.as_ref() {
        let Some(node) = profile.graph.nodes.iter().find(|node| node.id == *to_id) else {
            return;
        };
        let to = socket_position(
            &profile.graph,
            controller.canvas.graph_rect(
                canvas,
                controller.display_position(&node.id, node.position),
                node_size(&profile.graph, node),
            ),
            node,
            true,
            *to_input,
        );
        let from = ui
            .ctx()
            .pointer_hover_pos()
            .map(|position| position.clamp(canvas.min, canvas.max))
            .unwrap_or_else(|| Pos2::new(to.x - 100.0, to.y));
        let points = graph_canvas::bezier_points(from, to);
        graph_canvas::paint_wire(ui, points, Stroke::new(2.0, style::LINK_SELECTED));
    }
}

fn link_points(
    canvas: Rect,
    controller: &PromptStudioController,
    graph: &PromptNodeGraph,
    link: &PromptLink,
) -> Option<(Pos2, Pos2)> {
    let from = graph.nodes.iter().find(|node| node.id == link.from)?;
    let to = graph.nodes.iter().find(|node| node.id == link.to)?;
    Some((
        socket_position(
            graph,
            controller.canvas.graph_rect(
                canvas,
                controller.display_position(&from.id, from.position),
                node_size(graph, from),
            ),
            from,
            false,
            0,
        ),
        socket_position(
            graph,
            controller.canvas.graph_rect(
                canvas,
                controller.display_position(&to.id, to.position),
                node_size(graph, to),
            ),
            to,
            true,
            link.input,
        ),
    ))
}

fn render_selection_box(ui: &egui::Ui, controller: &PromptStudioController) {
    let Some(selection) = controller.selection_rect() else {
        return;
    };
    graph_canvas::paint_selection_box(ui, selection, GRAPH_ACCENT);
}

fn socket_position(
    graph: &PromptNodeGraph,
    rect: Rect,
    node: &PromptNode,
    input: bool,
    index: u8,
) -> Pos2 {
    let scale = node_scale(rect, node);
    if input {
        if matches!(
            node.kind,
            PromptNodeKind::Compose { .. }
                | PromptNodeKind::Switch { .. }
                | PromptNodeKind::TextSwitch
                | PromptNodeKind::Request { .. }
        ) {
            let row = input_socket_indexes(graph, node)
                .iter()
                .position(|value| *value == index)
                .unwrap_or_default();
            return Pos2::new(
                rect.left(),
                rect.top() + (NODE_HEADER_HEIGHT + 22.0 + row as f32 * 25.0) * scale,
            );
        }
        return Pos2::new(rect.left(), rect.center().y);
    }
    Pos2::new(rect.right(), rect.center().y)
}

fn node_scale(rect: Rect, node: &PromptNode) -> f32 {
    rect.width() / (runtime_preview::base_width(&node.kind) + runtime_preview::WIDTH)
}

fn input_socket_indexes(graph: &PromptNodeGraph, node: &PromptNode) -> Vec<u8> {
    match &node.kind {
        PromptNodeKind::Compose { .. } => graph.compose_input_socket_indexes(&node.id),
        PromptNodeKind::Switch { .. } => vec![0, 1, 2],
        PromptNodeKind::TextComparison { .. } => vec![0],
        PromptNodeKind::TextSwitch => (0..=graph
            .text_switch_cases(&node.id)
            .map(|cases| cases.len().min(u8::MAX as usize) as u8)
            .unwrap_or_default())
            .collect(),
        PromptNodeKind::Request { roles, .. } => (0..roles.len() as u8).collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_prompt_rewire_restores_the_original_connection_atomically() {
        let mut profile = default_profile();
        let original = profile.graph.links[0].clone();
        let before = profile.clone();
        let mut controller = PromptStudioController::default();
        commit_prompt_wire(
            &mut profile,
            &mut controller,
            crate::ui::graph_editor::WireCommit {
                from: "missing-source".to_owned(),
                to: Some((original.to.clone(), original.input)),
                replaced: Some(PromptLinkKey {
                    from: original.from,
                    to: original.to,
                    input: original.input,
                }),
            },
        );
        assert_eq!(profile, before);
    }

    #[test]
    fn panned_canvas_controls_cannot_expand_the_parent_layout() {
        let context = egui::Context::default();
        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
            let (canvas, _) = ui.allocate_exact_size(Vec2::new(240.0, 160.0), Sense::hover());
            let parent_layout = ui.min_rect();
            let mut text = String::from("offscreen node editor");

            {
                let mut viewport = graph_canvas::canvas_viewport(ui, canvas);
                let offscreen = Rect::from_min_size(
                    canvas.max + Vec2::new(10_000.0, 10_000.0),
                    Vec2::new(400.0, 200.0),
                );
                viewport.put(offscreen, egui::TextEdit::multiline(&mut text));
            }

            assert_eq!(ui.min_rect(), parent_layout);
        });
        output.textures_delta.clear();
    }

    #[test]
    fn fit_keeps_complete_node_bounds_inside_the_canvas() {
        let mut graph = PromptNodeGraph::empty();
        graph.add_variable(
            PromptNodePage::OpenAiCompatible,
            PromptVariable::CurrentInput,
            [100.0, 100.0],
        );
        graph.add_request(
            PromptProviderTarget::OpenAiCompatible,
            vec![PromptMessageRole::System, PromptMessageRole::User],
            [700.0, 500.0],
        );
        let available = Vec2::new(1000.0, 600.0);
        let canvas = Rect::from_min_size(Pos2::ZERO, available);
        let mut controller = PromptStudioController::default();

        navigation::fit_graph_to_canvas(&graph, &mut controller, available);

        for node in &graph.nodes {
            let rect = controller
                .canvas
                .graph_rect(canvas, node.position, node_size(&graph, node));
            assert!(canvas.contains(rect.min));
            assert!(canvas.contains(rect.max));
        }
    }

    #[test]
    fn parse_validation_error_locates_unconnected_node_and_input() {
        let err = xrtranslate_prompt::PromptGraphError::new(
            "node hunyuan-explicit-instruction input 2 is not connected",
        );
        let target = parse_validation_error_target(Some(&err));
        assert_eq!(
            target.node_id.as_deref(),
            Some("hunyuan-explicit-instruction")
        );
        assert_eq!(target.input_index, Some(2));
    }

    #[test]
    fn remove_compose_placeholder_cleans_unused_slots() {
        let mut text = String::from("Translate: {0}\n\n{1}\n\n{2}");
        remove_compose_placeholder(&mut text, 2);
        assert_eq!(text, "Translate: {0}\n\n{1}");

        remove_compose_placeholder(&mut text, 1);
        assert_eq!(text, "Translate: {0}");
    }

    #[test]
    fn test_node_position_delta_accumulates_and_snaps() {
        let mut graph = PromptNodeGraph::empty();
        let id = graph.add_variable(
            PromptNodePage::OpenAiCompatible,
            PromptVariable::CurrentInput,
            [100.0, 100.0],
        );
        let mut controller = PromptStudioController::default();
        controller.selected_nodes.insert(id.clone());

        // Simulate 3 frames of dragging
        for _ in 0..3 {
            let delta = Vec2::new(10.0, 5.0);
            for target in &mut graph.nodes {
                if controller.selected_nodes.contains(&target.id) {
                    target.position[0] += delta.x;
                    target.position[1] += delta.y;
                }
            }
        }

        let node = graph.nodes.iter().find(|n| n.id == id).unwrap();
        assert_eq!(node.position, [130.0, 115.0]);

        // Snap to 16px grid
        for target in &mut graph.nodes {
            if controller.selected_nodes.contains(&target.id) {
                target.position[0] = (target.position[0] / 16.0).round() * 16.0;
                target.position[1] = (target.position[1] / 16.0).round() * 16.0;
            }
        }

        let node = graph.nodes.iter().find(|n| n.id == id).unwrap();
        assert_eq!(node.position, [128.0, 112.0]);
    }
}
