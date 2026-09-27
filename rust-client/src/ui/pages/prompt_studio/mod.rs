//! Public style selection and the explicitly unlocked prompt editor.
mod editor;
mod style_picker;

pub(crate) use editor::PromptStudioController;
pub(crate) use editor::access::editor_enabled;
pub(crate) use style_picker::apply as selected_style_profile;

use crate::i18n::{UiLanguage, tr};
use eframe::egui::{self, Id};
use std::{
    path::Path,
    time::{Duration, Instant},
};
use xrtranslate_prompt::{
    PromptGraphDomain, PromptProviderTarget, PromptTemplateLibrary, PromptTemplateProfile,
};

#[derive(Clone, Debug)]
pub enum PromptStudioAction {
    SelectStyle(style_picker::StyleSelection),
    SwitchDomain(PromptGraphDomain),
    SelectProfile(String),
    CreateProfile(PromptTemplateProfile),
    DeleteProfile(String),
    SaveProfile(PromptTemplateProfile),
    ActivateProfile(PromptTemplateProfile),
    CloneProfile(PromptTemplateProfile),
    ExportProfile(PromptTemplateProfile),
    ImportProfile,
}

#[derive(Clone, Default)]
struct PageState {
    editor_open: bool,
    beta: bool,
    checked: Option<Instant>,
    editor_allowed: bool,
}

fn state_id() -> Id {
    Id::new("prompt_studio_page")
}

pub(crate) fn leave_page(
    ctx: &egui::Context,
    controller: &mut PromptStudioController,
    library: &PromptTemplateLibrary,
) -> Vec<PromptStudioAction> {
    let mut actions = Vec::new();
    ctx.data_mut(|data| {
        if let Some(mut state) = data.get_temp::<PageState>(state_id()) {
            if state.editor_open {
                let snapshot = controller.snapshot(library);
                editor::save_before_switch(&snapshot, controller, &mut actions);
            }
            state.editor_open = false;
            state.checked = None;
            data.insert_temp(state_id(), state);
        }
    });
    actions
}

pub(crate) fn render(
    library: &PromptTemplateLibrary,
    controller: &mut PromptStudioController,
    ui: &mut egui::Ui,
    language: UiLanguage,
    target: PromptProviderTarget,
    beta: bool,
    project_root: &Path,
) -> Vec<PromptStudioAction> {
    let mut state = ui
        .ctx()
        .data_mut(|data| data.get_temp::<PageState>(state_id()).unwrap_or_default());
    // Only watch while this page is visible; file I/O stays out of the animation loop.
    if state.beta != beta
        || state
            .checked
            .is_none_or(|time| time.elapsed() >= Duration::from_secs(1))
    {
        state.editor_allowed = editor_enabled(beta, project_root);
        state.checked = Some(Instant::now());
        state.beta = beta;
    }
    if beta {
        ui.ctx().request_repaint_after(Duration::from_secs(1));
    }
    if !state.editor_allowed {
        state.editor_open = false;
    }

    let mut actions = Vec::new();
    if state.editor_open {
        if crate::ui::components::secondary_button(ui, tr(language, "Back to styles")).clicked() {
            let snapshot = controller.snapshot(library);
            editor::save_before_switch(&snapshot, controller, &mut actions);
            state.editor_open = false;
        }
    } else {
        ui.horizontal(|ui| {
            ui.heading(tr(language, "Translation style"));
            if state.editor_allowed {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if crate::ui::components::secondary_button(ui, tr(language, "Edit prompts"))
                        .clicked()
                        && editor_enabled(beta, project_root)
                    {
                        controller.open_style_panel(target, library);
                        state.editor_open = true;
                    }
                });
            }
        });
    }
    ui.add_space(12.0);
    if state.editor_open {
        let snapshot = controller.snapshot(library);
        actions.extend(editor::render(&snapshot, controller, ui, language));
    } else if let Some(selection) = style_picker::render(library, ui, language, target) {
        actions.push(PromptStudioAction::SelectStyle(selection));
    }
    ui.ctx()
        .data_mut(|data| data.insert_temp(state_id(), state));
    actions
}
