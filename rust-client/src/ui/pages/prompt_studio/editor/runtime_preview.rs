use eframe::egui;
use xrtranslate_prompt::{PromptExecutionTrace, PromptNode, PromptNodeGraph, PromptNodeKind};

/// Inspect node values on demand without reserving empty space in every card.
pub(super) fn render(
    ui: &mut egui::Ui,
    graph: &PromptNodeGraph,
    node: &PromptNode,
    trace: Option<&PromptExecutionTrace>,
    language: crate::i18n::UiLanguage,
) {
    let live_output = trace
        .and_then(|trace| trace.node(&node.id))
        .map(|node| node.output.clone());
    let output = live_output.clone().or_else(|| match &node.kind {
        PromptNodeKind::Request { .. } => graph.compose_request_preview(&node.id),
        PromptNodeKind::Compose { text }
        | PromptNodeKind::Input {
            block: xrtranslate_prompt::TranslationPromptBlock::CustomText { text },
        } => Some(text.clone()),
        _ => None,
    });
    let Some(output) = output else {
        return;
    };
    ui.separator();
    ui.weak(crate::i18n::tr(
        language,
        if live_output.is_some() {
            "LIVE OUTPUT"
        } else {
            "Preview"
        },
    ));
    egui::ScrollArea::vertical()
        .id_salt(("prompt_node_runtime", &node.id))
        .max_height(300.0)
        .show(ui, |ui| {
            ui.add(
                egui::Label::new(if output.is_empty() {
                    crate::i18n::tr(language, "(empty)")
                } else {
                    &output
                })
                .selectable(true)
                .wrap(),
            );
        });
}

pub(super) fn base_width(kind: &PromptNodeKind) -> f32 {
    match kind {
        PromptNodeKind::Compose { .. } => 320.0,
        PromptNodeKind::Switch { .. } | PromptNodeKind::Request { .. } => 260.0,
        _ => super::NODE_WIDTH,
    }
}
