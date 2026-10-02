mod actions;
mod detail;
mod library;
mod presentation;
mod setup;

use super::{MeetingAction, MeetingPlugin, MeetingUiSnapshot, controller::MeetingRoute};
use actions::apply_action;
use eframe::egui;

pub(super) fn render(
    plugin: &mut MeetingPlugin,
    snapshot: &MeetingUiSnapshot,
    ui: &mut egui::Ui,
) -> MeetingAction {
    if ui.available_width() < 600.0 {
        ui.spacing_mut().interact_size = ui.spacing().interact_size.max(egui::Vec2::splat(44.0));
    }
    let action = match plugin.controller.route {
        MeetingRoute::Library => {
            library::render_library(&mut plugin.controller, snapshot.language, ui)
        }
        MeetingRoute::Create => setup::render_setup(&mut plugin.controller, snapshot, ui),
        MeetingRoute::Detail => detail::render_detail(
            &mut plugin.controller,
            snapshot.language,
            snapshot.waiting_for_microphone,
            ui,
        ),
    };
    apply_action(&mut plugin.controller, action, snapshot)
}
