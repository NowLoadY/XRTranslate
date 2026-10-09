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
