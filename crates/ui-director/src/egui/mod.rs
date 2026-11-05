mod capture;
mod controls;
pub mod driver;
pub mod feedback;
pub mod registry;
pub mod server;
pub mod tools;

use ::egui::{self, Rect};
use std::sync::{Arc, OnceLock};

pub use controls::{
    Control, ControlAction, button_response, number_response, record_button, record_checkbox,
    record_combobox, record_slider, record_text_input, record_toggle, text_response,
};
pub use driver::Driver;
pub use registry::{ElementDescriptor, ElementKind, ElementValue};
pub use server::Server;

static GLOBAL_DRIVER: OnceLock<Arc<Driver>> = OnceLock::new();
static GLOBAL_SERVER: OnceLock<Server> = OnceLock::new();

#[must_use]
pub fn driver() -> Arc<Driver> {
    GLOBAL_DRIVER
        .get_or_init(|| Arc::new(Driver::new()))
        .clone()
}

/// Queued UI actions need a real frame even when the window is hidden. This is
/// a one-frame request; it neither raises the window nor starts continuous painting.
pub(super) fn request_frame(ctx: &egui::Context) {
    ctx.send_viewport_cmd_to(
        egui::ViewportId::ROOT,
        egui::ViewportCommand::RequestPaintWhileHidden,
    );
    ctx.request_repaint();
}

pub fn init(egui_ctx: egui::Context, port: Option<u16>) {
    let d = driver();
    let port = port.unwrap_or(crate::DEFAULT_PORT);
    let server = GLOBAL_SERVER.get_or_init(|| Server::new(port));
    if let Err(e) = server.start(d, egui_ctx) {
        log::warn!("Could not start UI Director Server: {e}");
    }
}

pub fn begin_frame(ctx: &egui::Context, current_page: &str, pages: Vec<registry::Page>) {
    let driver = driver();
    driver.frame_state.lock().unwrap().pages = pages;
    driver.begin_frame(ctx, current_page);
}

pub fn raw_input(ctx: &egui::Context, input: &mut egui::RawInput) {
    capture::receive(ctx, input);
    input
        .events
        .append(&mut driver().frame_state.lock().unwrap().pending_input);
}

pub fn finish_frame(current_page: &str) {
    driver().finish_frame(current_page);
}

/// Expose rendered read-only values through the same component registry.
pub fn record_output(ui: &egui::Ui, label: &str, value: ElementValue, rect: Rect) {
    Control::new(label, ElementKind::Other, value).record(
        ui,
        ui.make_persistent_id(label),
        false,
        rect,
    );
}

pub fn record_text_output(ui: &egui::Ui, text: &str, response: &egui::Response) {
    if ui.is_rect_visible(response.rect) {
        Control::new("Text", ElementKind::Other, ElementValue::Text(text.into())).record(
            ui,
            response.id,
            false,
            response.rect,
        );
    }
}

#[must_use]
pub fn take_pending_page() -> Option<String> {
    let d = driver();
    let mut state = d.frame_state.lock().unwrap();
    let page = state.pending_page.take();
    state.action_performed_this_frame |= page.is_some();
    page
}
