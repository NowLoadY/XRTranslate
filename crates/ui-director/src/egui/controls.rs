//! One declaration drives inspection and interaction for every shared UI component.
use super::{ElementDescriptor, ElementKind, ElementValue, driver};
use crate::feedback::{Effect, Feedback};
use ::egui::{self, Rect};

pub enum ControlAction {
    Click,
    Set(ElementValue),
}

pub struct Control<'a> {
    label: &'a str,
    kind: ElementKind,
    value: ElementValue,
    options: Vec<String>,
    range: Option<[f64; 2]>,
}

impl<'a> Control<'a> {
    pub fn new(label: &'a str, kind: ElementKind, value: ElementValue) -> Self {
        Self {
            label,
            kind,
            value,
            options: Vec::new(),
            range: None,
        }
    }
    pub fn button(label: &'a str) -> Self {
        Self::new(label, ElementKind::Button, ElementValue::None)
    }
    pub fn options(mut self, options: impl IntoIterator<Item = String>) -> Self {
        self.options = options.into_iter().collect();
        self
    }
    pub fn range(mut self, min: f64, max: f64) -> Self {
        self.range = Some([min, max]);
        self
    }
    /// Custom buttons need only a label and their real egui response.
    pub fn response(self, ui: &egui::Ui, mut response: egui::Response) -> egui::Response {
        if matches!(
            self.record(ui, response.id, response.enabled(), response.rect),
            Some(ControlAction::Click)
        ) {
            response
                .flags
                .insert(egui::response::Flags::FAKE_PRIMARY_CLICKED);
        }
        response
    }

    pub fn record(
        self,
        ui: &egui::Ui,
        id: egui::Id,
        enabled: bool,
        rect: Rect,
    ) -> Option<ControlAction> {
        let driver = driver();
        if !driver.owns_context(ui.ctx()) {
            return None;
        }
        let mut state = driver.frame_state.lock().unwrap();
        let index = state.elements.len();
        let mut element = ElementDescriptor {
            index,
            id_hex: format!("{:016x}", id.value()),
            label: self.label.to_owned(),
            kind: self.kind,
            value: self.value,
            enabled: enabled && ui.is_enabled(),
            rect: [rect.min.x, rect.min.y, rect.width(), rect.height()],
            options: self.options,
            range: self.range,
        };
        // Observe the component's real value on its next render, after its normal handler.
        if state
            .confirm_change
            .as_ref()
            .is_some_and(|(target, _, _)| target == &element.id_hex)
        {
            let (_, before, _) = state.confirm_change.take().unwrap();
            let change = Feedback::changed(
                id.value(),
                &element.label,
                &before.as_text().unwrap_or_default(),
                &element.value.as_text().unwrap_or_default(),
            );
            super::feedback::show_in(
                ui.ctx(),
                change.or_else(|| Some(element.feedback(Effect::Invoke))),
            );
        }
        // The driver resolves labels/indices once against the last snapshot.
        // Components consume only that exact ID, never a second fuzzy lookup.
        let click = state.pending_click.as_deref() == Some(element.id_hex.as_str());
        let value = state
            .pending_set
            .as_ref()
            .filter(|(target, _)| target == &element.id_hex)
            .map(|(_, value)| value);
        let action = if click || value.is_some() {
            let result = if element.enabled {
                requested_action(&element, click, value)
            } else {
                Err("Control is disabled")
            };
            state.pending_click = None;
            state.pending_set = None;
            match result {
                Ok(action) => {
                    if let ControlAction::Set(value) = &action {
                        state.confirm_change = Some((
                            element.id_hex.clone(),
                            element.value.clone(),
                            ui.input(|i| i.time) + 4.0,
                        ));
                        element.value = value.clone();
                    }
                    state.action_performed_this_frame = true;
                    super::feedback::show_in(ui.ctx(), [element.feedback(Effect::Invoke)]);
                    ui.ctx().request_repaint();
                    Some(action)
                }
                Err(error) => {
                    super::feedback::show_in(
                        ui.ctx(),
                        [element.feedback(Effect::Failed(error.into()))],
                    );
                    if let Some(reply) = state.pending_widget_reply.take() {
                        let _ = reply.send(crate::Response::err(error));
                    }
                    None
                }
            }
        } else {
            None
        };
        state.elements.push(element);
        super::feedback::record(ui, id, rect);
        action
    }
}

fn requested_action(
    element: &ElementDescriptor,
    click: bool,
    value: Option<&ElementValue>,
) -> Result<ControlAction, &'static str> {
    if click {
        return match element.kind {
            ElementKind::Toggle | ElementKind::Checkbox => Ok(ControlAction::Set(
                ElementValue::Bool(!element.value.as_bool().unwrap_or(false)),
            )),
            _ if element.actions().contains(&"click") => Ok(ControlAction::Click),
            _ => Err("This control does not support click"),
        };
    }
    let value = value.ok_or("No value supplied")?;
    let value = match element.kind {
        ElementKind::Toggle | ElementKind::Checkbox => value.as_bool().map(ElementValue::Bool),
        ElementKind::Slider => {
            value
                .as_number()
                .filter(|number| number.is_finite())
                .map(|number| {
                    ElementValue::Number(
                        element
                            .range
                            .map_or(number, |[min, max]| number.clamp(min, max)),
                    )
                })
        }
        ElementKind::TextInput => value.as_text().map(ElementValue::Text),
        ElementKind::ComboBox => value
            .as_text()
            .and_then(|text| {
                element
                    .options
                    .iter()
                    .find(|option| option.eq_ignore_ascii_case(&text))
                    .cloned()
            })
            .map(ElementValue::Text),
        _ => None,
    }
    .ok_or("Invalid value for this control; inspect its actions, range and options")?;
    Ok(ControlAction::Set(value))
}

pub fn record_button(
    ui: &mut egui::Ui,
    id: egui::Id,
    label: &str,
    enabled: bool,
    rect: Rect,
) -> bool {
    matches!(
        Control::button(label).record(ui, id, enabled, rect),
        Some(ControlAction::Click)
    )
}

pub fn button_response(
    ui: &mut egui::Ui,
    id: egui::Id,
    label: &str,
    enabled: bool,
    mut response: egui::Response,
) -> egui::Response {
    if record_button(ui, id, label, enabled && response.enabled(), response.rect) {
        response
            .flags
            .insert(egui::response::Flags::FAKE_PRIMARY_CLICKED);
        let now = ui.input(|input| input.time);
        ui.data_mut(|data| data.insert_temp(id.with("click_time"), now));
    }
    response
}

fn record_value(
    ui: &egui::Ui,
    id: egui::Id,
    control: Control<'_>,
    enabled: bool,
    rect: Rect,
) -> Option<ElementValue> {
    match control.record(ui, id, enabled, rect) {
        Some(ControlAction::Set(value)) => Some(value),
        _ => None,
    }
}

pub fn record_toggle(
    ui: &mut egui::Ui,
    id: egui::Id,
    label: &str,
    checked: bool,
    enabled: bool,
    rect: Rect,
) -> Option<bool> {
    record_value(
        ui,
        id,
        Control::new(label, ElementKind::Toggle, ElementValue::Bool(checked)),
        enabled,
        rect,
    )?
    .as_bool()
}
pub fn record_checkbox(
    ui: &mut egui::Ui,
    id: egui::Id,
    label: &str,
    checked: bool,
    enabled: bool,
    rect: Rect,
) -> Option<bool> {
    record_value(
        ui,
        id,
        Control::new(label, ElementKind::Checkbox, ElementValue::Bool(checked)),
        enabled,
        rect,
    )?
    .as_bool()
}
pub fn record_slider(
    ui: &mut egui::Ui,
    id: egui::Id,
    label: &str,
    value: f64,
    enabled: bool,
    rect: Rect,
) -> Option<f64> {
    record_value(
        ui,
        id,
        Control::new(label, ElementKind::Slider, ElementValue::Number(value)),
        enabled,
        rect,
    )?
    .as_number()
}
pub fn record_text_input(
    ui: &mut egui::Ui,
    id: egui::Id,
    label: &str,
    text: &str,
    enabled: bool,
    rect: Rect,
) -> Option<String> {
    record_value(
        ui,
        id,
        Control::new(
            label,
            ElementKind::TextInput,
            ElementValue::Text(text.into()),
        ),
        enabled,
        rect,
    )?
    .as_text()
}

/// Add discovery and editing to a custom text field without replacing its UI handler.
pub fn text_response(
    ui: &egui::Ui,
    label: &str,
    text: &mut String,
    mut response: egui::Response,
) -> egui::Response {
    if let Some(ControlAction::Set(ElementValue::Text(value))) = Control::new(
        label,
        ElementKind::TextInput,
        ElementValue::Text(text.clone()),
    )
    .record(ui, response.id, response.enabled(), response.rect)
    {
        *text = value;
        response.mark_changed();
    }
    response
}

/// Keep a custom numeric widget's formatting and its normal changed handler.
pub fn number_response<T: egui::emath::Numeric>(
    ui: &egui::Ui,
    label: &str,
    value: &mut T,
    range: std::ops::RangeInclusive<T>,
    mut response: egui::Response,
) -> egui::Response {
    if let Some(ControlAction::Set(ElementValue::Number(number))) = Control::new(
        label,
        ElementKind::Slider,
        ElementValue::Number(value.to_f64()),
    )
    .range(range.start().to_f64(), range.end().to_f64())
    .record(ui, response.id, response.enabled(), response.rect)
    {
        *value = T::from_f64(number);
        response.mark_changed();
    }
    response
}

pub fn record_combobox(
    ui: &mut egui::Ui,
    id: egui::Id,
    label: &str,
    current: &str,
    options: impl IntoIterator<Item = String>,
    response: &egui::Response,
) -> Option<String> {
    match Control::new(
        label,
        ElementKind::ComboBox,
        ElementValue::Text(current.into()),
    )
    .options(options)
    .record(ui, id, response.enabled(), response.rect)
    {
        Some(ControlAction::Set(ElementValue::Text(text))) => Some(text),
        Some(ControlAction::Click) => {
            egui::Popup::toggle_id(ui.ctx(), response.id.with("popup"));
            None
        }
        _ => None,
    }
}
