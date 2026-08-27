pub mod driver;
pub mod registry;
pub mod server;

use std::sync::{Arc, OnceLock};
use eframe::egui::{self, Rect};

pub use driver::{AutomationDriver, DirectorCommand, DirectorResponse};
pub use registry::{ElementDescriptor, ElementKind, ElementValue, FrameSnapshot};
pub use server::{DEFAULT_DIRECTOR_PORT, DirectorServer};

use crate::ui::Page;

static GLOBAL_DRIVER: OnceLock<Arc<AutomationDriver>> = OnceLock::new();
static GLOBAL_SERVER: OnceLock<DirectorServer> = OnceLock::new();

#[must_use]
pub fn driver() -> Arc<AutomationDriver> {
    GLOBAL_DRIVER
        .get_or_init(|| Arc::new(AutomationDriver::new()))
        .clone()
}

pub fn init(egui_ctx: egui::Context, port: Option<u16>) {
    let d = driver();
    let port = port.unwrap_or(DEFAULT_DIRECTOR_PORT);
    let server = GLOBAL_SERVER.get_or_init(|| DirectorServer::new(port));
    if let Err(e) = server.start(d, egui_ctx) {
        log::warn!("Could not start UI Director Server: {e}");
    }
}

pub fn begin_frame(current_page: &str) {
    driver().begin_frame(current_page);
}

pub fn finish_frame() {
    driver().finish_frame();
}

#[must_use]
pub fn take_pending_page() -> Option<Page> {
    let d = driver();
    let mut state = d.frame_state.lock().unwrap();
    state.pending_page.take()
}

#[must_use]
pub fn take_pending_onboarding_step() -> Option<usize> {
    let d = driver();
    let mut state = d.frame_state.lock().unwrap();
    state.pending_onboarding_step.take()
}

/// Records a button element in the current frame and checks if an automated click should be simulated.
pub fn record_button(
    ui: &mut egui::Ui,
    id: egui::Id,
    label: &str,
    enabled: bool,
    rect: Rect,
) -> bool {
    let d = driver();
    let id_hex = format!("{:016x}", id.value());
    let mut should_click = false;

    {
        let mut state = d.frame_state.lock().unwrap();
        let index = state.elements.len();
        state.elements.push(ElementDescriptor {
            index,
            id_hex: id_hex.clone(),
            label: label.to_string(),
            kind: ElementKind::Button,
            value: ElementValue::None,
            enabled,
            rect: [rect.min.x, rect.min.y, rect.width(), rect.height()],
        });

        if let Some(target) = &state.pending_click {
            if matches_target(target, index, &id_hex, label) && enabled {
                should_click = true;
                state.action_performed_this_frame = true;
            }
        }
    }

    if should_click {
        let mut state = d.frame_state.lock().unwrap();
        state.pending_click = None;
        ui.ctx().request_repaint();
    }

    should_click
}

/// Records a toggle switch element in the current frame and checks for automated value changes.
pub fn record_toggle(
    ui: &mut egui::Ui,
    id: egui::Id,
    label: &str,
    checked: bool,
    enabled: bool,
    rect: Rect,
) -> Option<bool> {
    let d = driver();
    let id_hex = format!("{:016x}", id.value());
    let mut new_val = None;

    {
        let mut state = d.frame_state.lock().unwrap();
        let index = state.elements.len();
        state.elements.push(ElementDescriptor {
            index,
            id_hex: id_hex.clone(),
            label: label.to_string(),
            kind: ElementKind::Toggle,
            value: ElementValue::Bool(checked),
            enabled,
            rect: [rect.min.x, rect.min.y, rect.width(), rect.height()],
        });

        if let Some(target) = &state.pending_click {
            if matches_target(target, index, &id_hex, label) && enabled {
                new_val = Some(!checked);
                state.action_performed_this_frame = true;
            }
        } else if let Some((target, val)) = &state.pending_set {
            if matches_target(target, index, &id_hex, label) && enabled {
                if let Some(b) = val.as_bool() {
                    new_val = Some(b);
                    state.action_performed_this_frame = true;
                }
            }
        }
    }

    if new_val.is_some() {
        let mut state = d.frame_state.lock().unwrap();
        state.pending_click = None;
        state.pending_set = None;
        ui.ctx().request_repaint();
    }

    new_val
}

/// Records a checkbox element in the current frame and checks for automated value changes.
pub fn record_checkbox(
    ui: &mut egui::Ui,
    id: egui::Id,
    label: &str,
    checked: bool,
    enabled: bool,
    rect: Rect,
) -> Option<bool> {
    let d = driver();
    let id_hex = format!("{:016x}", id.value());
    let mut new_val = None;

    {
        let mut state = d.frame_state.lock().unwrap();
        let index = state.elements.len();
        state.elements.push(ElementDescriptor {
            index,
            id_hex: id_hex.clone(),
            label: label.to_string(),
            kind: ElementKind::Checkbox,
            value: ElementValue::Bool(checked),
            enabled,
            rect: [rect.min.x, rect.min.y, rect.width(), rect.height()],
        });

        if let Some(target) = &state.pending_click {
            if matches_target(target, index, &id_hex, label) && enabled {
                new_val = Some(!checked);
                state.action_performed_this_frame = true;
            }
        } else if let Some((target, val)) = &state.pending_set {
            if matches_target(target, index, &id_hex, label) && enabled {
                if let Some(b) = val.as_bool() {
                    new_val = Some(b);
                    state.action_performed_this_frame = true;
                }
            }
        }
    }

    if new_val.is_some() {
        let mut state = d.frame_state.lock().unwrap();
        state.pending_click = None;
        state.pending_set = None;
        ui.ctx().request_repaint();
    }

    new_val
}

/// Records a slider element in the current frame and checks for automated value changes.
pub fn record_slider(
    ui: &mut egui::Ui,
    id: egui::Id,
    label: &str,
    value: f64,
    enabled: bool,
    rect: Rect,
) -> Option<f64> {
    let d = driver();
    let id_hex = format!("{:016x}", id.value());
    let mut new_val = None;

    {
        let mut state = d.frame_state.lock().unwrap();
        let index = state.elements.len();
        state.elements.push(ElementDescriptor {
            index,
            id_hex: id_hex.clone(),
            label: label.to_string(),
            kind: ElementKind::Slider,
            value: ElementValue::Number(value),
            enabled,
            rect: [rect.min.x, rect.min.y, rect.width(), rect.height()],
        });

        if let Some((target, val)) = &state.pending_set {
            if matches_target(target, index, &id_hex, label) && enabled {
                if let Some(num) = val.as_number() {
                    new_val = Some(num);
                    state.action_performed_this_frame = true;
                }
            }
        }
    }

    if new_val.is_some() {
        let mut state = d.frame_state.lock().unwrap();
        state.pending_set = None;
        ui.ctx().request_repaint();
    }

    new_val
}

/// Records a combobox / dropdown element in the current frame and checks for automated selection.
pub fn record_combobox(
    ui: &mut egui::Ui,
    id: egui::Id,
    label: &str,
    current_value: &str,
    enabled: bool,
    rect: Rect,
) -> Option<String> {
    let d = driver();
    let id_hex = format!("{:016x}", id.value());
    let mut new_val = None;

    {
        let mut state = d.frame_state.lock().unwrap();
        let index = state.elements.len();
        state.elements.push(ElementDescriptor {
            index,
            id_hex: id_hex.clone(),
            label: label.to_string(),
            kind: ElementKind::ComboBox,
            value: ElementValue::Text(current_value.to_string()),
            enabled,
            rect: [rect.min.x, rect.min.y, rect.width(), rect.height()],
        });

        if let Some((target, val)) = &state.pending_set {
            if matches_target(target, index, &id_hex, label) && enabled {
                if let Some(txt) = val.as_text() {
                    new_val = Some(txt);
                    state.action_performed_this_frame = true;
                }
            }
        }
    }

    if new_val.is_some() {
        let mut state = d.frame_state.lock().unwrap();
        state.pending_set = None;
        ui.ctx().request_repaint();
    }

    new_val
}

/// Records a text edit input element in the current frame and checks for automated text input.
pub fn record_text_input(
    ui: &mut egui::Ui,
    id: egui::Id,
    label: &str,
    text: &str,
    enabled: bool,
    rect: Rect,
) -> Option<String> {
    let d = driver();
    let id_hex = format!("{:016x}", id.value());
    let mut new_val = None;

    {
        let mut state = d.frame_state.lock().unwrap();
        let index = state.elements.len();
        state.elements.push(ElementDescriptor {
            index,
            id_hex: id_hex.clone(),
            label: label.to_string(),
            kind: ElementKind::TextInput,
            value: ElementValue::Text(text.to_string()),
            enabled,
            rect: [rect.min.x, rect.min.y, rect.width(), rect.height()],
        });

        if let Some((target, val)) = &state.pending_set {
            if matches_target(target, index, &id_hex, label) && enabled {
                if let Some(txt) = val.as_text() {
                    new_val = Some(txt);
                    state.action_performed_this_frame = true;
                }
            }
        }
    }

    if new_val.is_some() {
        let mut state = d.frame_state.lock().unwrap();
        state.pending_set = None;
        ui.ctx().request_repaint();
    }

    new_val
}

fn matches_target(target: &str, index: usize, id_hex: &str, label: &str) -> bool {
    let target = target.trim();
    if target.is_empty() {
        return false;
    }

    // 1. Exact 16-char hex ID
    if target.len() == 16 && id_hex.eq_ignore_ascii_case(target) {
        return true;
    }

    // 2. Explicit index "#0"
    if let Some(index_str) = target.strip_prefix('#') {
        if let Ok(idx) = index_str.parse::<usize>() {
            return idx == index;
        }
    }

    // 3. Exact label
    if label.eq_ignore_ascii_case(target) {
        return true;
    }

    // 4. Short numeric index "0"
    if target.len() <= 4 {
        if let Ok(idx) = target.parse::<usize>() {
            return idx == index;
        }
    }

    // 5. Hex ID match
    if id_hex.eq_ignore_ascii_case(target) {
        return true;
    }

    // 6. Label substring match
    if label.to_lowercase().contains(&target.to_lowercase()) {
        return true;
    }

    false
}
