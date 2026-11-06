pub mod animation;
pub mod components;
pub mod fonts;
pub mod layout;
pub mod modal;
pub mod notifications;
pub mod organic_border;
pub mod organic_line;
pub mod theme;
pub mod virtual_list;

/// Initialize a native page with the same presentation resources as the shell.
pub fn configure_surface(ctx: &eframe::egui::Context, options: &serde_json::Value) {
    let first = ctx.data_mut(|data| {
        let id = eframe::egui::Id::unique("shared_ui_initialized");
        let first = !data.get_temp::<bool>(id).unwrap_or(false);
        data.insert_temp(id, true);
        first
    });
    if first {
        notifications::Notifications::default().install(ctx);
        fonts::configure_multilingual_fonts(ctx);
        egui_extras::install_image_loaders(ctx);
        crate::file_dialog::set_context(ctx.clone());
    }
    if let Ok(theme) = serde_json::from_value(options["theme"].clone()) {
        theme::install_context(ctx, theme);
    }
}

pub mod resource_browser;

pub fn forward_notices(ctx: &eframe::egui::Context, context: &mut xrtranslate_plugin::Context) {
    for error in crate::file_dialog::take_errors() {
        context.event("notice", serde_json::json!({"message":error}));
    }
    if let Some(notices) = ctx.data(|data| {
        data.get_temp::<notifications::Notifications>(eframe::egui::Id::unique("notifications"))
    }) {
        for (message, details) in notices.take_pending() {
            context.event(
                "notice",
                serde_json::json!({"message":message,"details":details}),
            );
        }
    }
}
