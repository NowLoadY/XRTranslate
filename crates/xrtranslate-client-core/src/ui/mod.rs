pub mod animation;
pub mod components;
pub mod error_report;
pub mod fonts;
pub mod layout;
pub mod modal;
pub mod notifications;
pub mod organic_border;
pub mod organic_line;
pub mod surface;
pub mod theme;
pub mod virtual_list;

#[macro_export]
macro_rules! icon {
    ($name:literal) => {{
        const BYTES: &[u8] =
            $crate::ui::icon_bytes($name).expect(concat!("Missing SDK icon: ", $name));
        $crate::ui::embedded_icon($name, BYTES)
    }};
}
pub use crate::icon;

include!(concat!(env!("OUT_DIR"), "/sdk_icons.rs"));

#[doc(hidden)]
pub fn embedded_icon(name: &str, bytes: &'static [u8]) -> eframe::egui::ImageSource<'static> {
    eframe::egui::ImageSource::Bytes {
        uri: format!("bytes://xrt-icons/{name}").into(),
        bytes: bytes.into(),
    }
}

pub fn icon_source(name: &str) -> eframe::egui::ImageSource<'static> {
    match icon_bytes(name) {
        Some(bytes) => embedded_icon(name, bytes),
        None => eframe::egui::ImageSource::Uri(format!("bytes://xrt-icons/{name}").into()),
    }
}

/// Initialize a native page with the same presentation resources as the shell.
pub fn configure_surface(ctx: &eframe::egui::Context, options: &serde_json::Value) {
    configure_surface_with_fonts(ctx, options, fonts::configure_multilingual_fonts);
}

pub fn configure_surface_with_fonts(
    ctx: &eframe::egui::Context,
    options: &serde_json::Value,
    configure_fonts: fn(&eframe::egui::Context),
) {
    let first = ctx.data_mut(|data| {
        let id = eframe::egui::Id::unique("shared_ui_initialized");
        let first = !data.get_temp::<bool>(id).unwrap_or(false);
        data.insert_temp(id, true);
        first
    });
    if first {
        crate::file_dialog::set_context(ctx.clone());
        notifications::Notifications::default().install(ctx);
        configure_fonts(ctx);
        egui_extras::install_image_loaders(ctx);
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

pub mod previews;
