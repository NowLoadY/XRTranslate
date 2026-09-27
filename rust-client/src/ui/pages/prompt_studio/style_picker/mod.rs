//! Read-only cards backed by the editor's existing saved style texts.
mod card;

use crate::i18n::{UiLanguage, tr};
use eframe::egui;
use std::collections::HashSet;
use xrtranslate_prompt::{PromptProviderTarget, PromptTemplateLibrary, PromptTemplateProfile};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StyleSelection {
    profile_id: String,
    preset: Option<usize>,
}

struct StyleCard {
    selection: StyleSelection,
    name: String,
    text: String,
}

fn cards(library: &PromptTemplateLibrary) -> Vec<StyleCard> {
    let mut cards = Vec::new();
    let mut seen_presets = HashSet::new();
    let mut seen_text = HashSet::new();
    for profile in &library.profiles {
        if let Some(style) = &profile.graph.translation_style {
            for (index, preset) in style.presets.iter().enumerate() {
                // Copies of the same preset collapse, but a new name is its own card.
                if seen_presets.insert((&preset.name, &preset.text)) {
                    seen_text.insert(preset.text.clone());
                    cards.push(StyleCard {
                        selection: StyleSelection {
                            profile_id: profile.id.clone(),
                            preset: Some(index),
                        },
                        name: preset.name.clone(),
                        text: preset.text.clone(),
                    });
                }
            }
        }
    }
    // Saved graph text is a style too, even when its graph is not active or it
    // hasn't been copied into a named preset. Both views read the same node.
    for profile in &library.profiles {
        let Some(text) = profile.graph.style_text() else {
            continue;
        };
        if text.trim().is_empty() || !seen_text.insert(text.clone()) {
            continue;
        }
        cards.push(StyleCard {
            selection: StyleSelection {
                profile_id: profile.id.clone(),
                preset: None,
            },
            name: if profile.id == library.active_id {
                "Current style".into()
            } else {
                profile.name.clone()
            },
            text,
        });
    }
    cards
}

pub(crate) fn apply(
    library: &PromptTemplateLibrary,
    selection: &StyleSelection,
    target: PromptProviderTarget,
) -> Result<PromptTemplateProfile, String> {
    let source = library
        .profiles
        .iter()
        .find(|profile| profile.id == selection.profile_id)
        .ok_or("Style no longer exists")?;
    let text = match selection.preset {
        Some(index) => source
            .graph
            .translation_style
            .as_ref()
            .and_then(|style| style.presets.get(index))
            .map(|preset| preset.text.clone()),
        None => source.graph.style_text(),
    }
    .ok_or("Style no longer exists")?;
    let active = library.active_profile().ok_or("No active prompt graph")?;
    if !active.graph.style_reaches_target(target) {
        return Err("The current prompt does not support style selection.".into());
    }
    let mut profile = if active.read_only {
        PromptTemplateLibrary::editable_copy_of(active, format!("custom-{}", uuid::Uuid::new_v4()))
    } else {
        active.clone()
    };
    if !profile.graph.set_style_text(&text) {
        return Err("Style text is not available".into());
    }
    profile
        .graph
        .validate_for_activation()
        .map_err(|error| error.to_string())?;
    Ok(profile)
}

pub(super) fn render(
    library: &PromptTemplateLibrary,
    ui: &mut egui::Ui,
    language: UiLanguage,
    target: PromptProviderTarget,
) -> Option<StyleSelection> {
    let cards = cards(library);
    let active = library.active_profile();
    let active_text = active.and_then(|profile| profile.graph.style_text());
    let enabled = active.is_some_and(|profile| profile.graph.style_reaches_target(target));
    if !enabled {
        ui.weak(tr(
            language,
            "The current prompt does not support style selection.",
        ));
    }
    let mut selected = None;
    crate::ui::components::selection_card::grid(
        ui,
        "translation_style_cards",
        cards.len(),
        336.0,
        |ui, index, width| {
            let style = &cards[index];
            let current = enabled && active_text.as_deref() == Some(style.text.as_str());
            if card::render(ui, style, width, current, enabled, language) && !current {
                selected = Some(style.selection.clone());
            }
        },
    );
    selected
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_reuses_saved_text_without_switching_graphs_or_editing_presets() {
        let mut library = PromptTemplateLibrary::default();
        let builtin_count = cards(&library).len();
        let mut custom =
            PromptTemplateLibrary::editable_copy_of(library.active_profile().unwrap(), "custom");
        // Saving unchanged built-in text under a new name still creates a visible card.
        custom.graph.save_style_preset("test");
        custom.graph.nodes[0].label = "Keep my graph".into();
        custom.graph.set_style_text("Use warm {natural} language.");
        custom.graph.save_style_preset("Warm");
        custom.graph.set_style_text("An unsaved style");
        library.profiles.push(custom.clone());
        library.active_id = custom.id.clone();
        let before = library.clone();
        let choices = cards(&library);
        assert_eq!(choices.len(), builtin_count + 3); // Built-ins appear once across copied graphs.
        assert_eq!(choices.last().unwrap().name, "Current style");
        let named_copy = choices.iter().find(|style| style.name == "test").unwrap();
        let default = choices
            .iter()
            .find(|style| style.name == "Default")
            .unwrap();
        assert_eq!(named_copy.text, default.text);
        assert_ne!(named_copy.selection, default.selection);
        let applied = apply(
            &library,
            &named_copy.selection,
            PromptProviderTarget::Hunyuan,
        )
        .unwrap();
        assert_eq!(applied.id, custom.id);
        assert_eq!(
            applied.graph.style_text().as_deref(),
            Some(default.text.as_str())
        );
        let warm = choices.iter().find(|style| style.name == "Warm").unwrap();
        let applied = apply(&library, &warm.selection, PromptProviderTarget::Hunyuan).unwrap();
        custom.graph.set_style_text(&warm.text);
        assert_eq!(applied, custom);
        assert_eq!(library, before);

        library.active_id = library.profiles[0].id.clone();
        let copy = apply(
            &library,
            &warm.selection,
            PromptProviderTarget::OpenAiCompatible,
        )
        .unwrap();
        assert!(!copy.read_only);
        assert_ne!(copy.id, library.active_id);
        assert_eq!(copy.graph.style_text().as_deref(), Some(warm.text.as_str()));
        assert_eq!(library.profiles[0], before.profiles[0]);

        // A saved editor graph's current text appears even when it is not active.
        let choices = cards(&library);
        let edited = choices
            .iter()
            .find(|style| style.text == "An unsaved style")
            .unwrap();
        assert_eq!(edited.name, custom.name);
        let applied = apply(&library, &edited.selection, PromptProviderTarget::Hunyuan).unwrap();
        assert_eq!(
            applied.graph.style_text().as_deref(),
            Some("An unsaved style")
        );

        // External card changes to the same profile must refresh a clean editor snapshot.
        let mut controller = super::super::PromptStudioController::default();
        library.active_id = custom.id.clone();
        controller.open_style_panel(PromptProviderTarget::Hunyuan, &library);
        let old = controller.snapshot(&library).draft;
        library
            .profiles
            .iter_mut()
            .find(|profile| profile.id == custom.id)
            .unwrap()
            .graph
            .set_style_text("Selected in the card picker");
        assert_eq!(
            controller
                .snapshot(&library)
                .draft
                .graph
                .style_text()
                .as_deref(),
            Some("Selected in the card picker")
        );
        controller.set_draft(old);
        assert_eq!(
            controller
                .snapshot(&library)
                .draft
                .graph
                .style_text()
                .as_deref(),
            Some("An unsaved style")
        );
        controller.accept_style_selection(custom.id.clone(), &library);
        assert!(!controller.is_dirty());
        assert_eq!(
            controller
                .snapshot(&library)
                .draft
                .graph
                .style_text()
                .as_deref(),
            Some("Selected in the card picker")
        );

        // Navigating away from the debug editor saves through its usual action contract.
        let mut draft = controller.snapshot(&library).draft;
        draft.graph.set_style_text("Edited before leaving the page");
        controller.set_draft(draft);
        let ctx = egui::Context::default();
        ctx.data_mut(|data| {
            data.insert_temp(
                super::super::state_id(),
                super::super::PageState {
                    editor_open: true,
                    ..Default::default()
                },
            )
        });
        let actions = super::super::leave_page(&ctx, &mut controller, &library);
        let [super::super::PromptStudioAction::ActivateProfile(saved)] = actions.as_slice() else {
            panic!("leaving an active editor must save and activate its draft");
        };
        assert_eq!(
            saved.graph.style_text().as_deref(),
            Some("Edited before leaving the page")
        );
        assert!(!controller.is_dirty());
        *library
            .profiles
            .iter_mut()
            .find(|profile| profile.id == saved.id)
            .unwrap() = saved.clone();
        library.normalize();
        assert!(
            cards(&library)
                .iter()
                .any(|style| style.text == "Edited before leaving the page")
        );
        assert!(super::super::leave_page(&ctx, &mut controller, &library).is_empty());
    }
}
