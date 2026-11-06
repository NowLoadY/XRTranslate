pub mod animation;
pub mod components;
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
    ($name:literal) => {{ $crate::ui::icon_source($name) }};
}
pub use crate::icon;

pub fn icon_source(name: &str) -> eframe::egui::ImageSource<'static> {
    let bytes: &'static [u8] = match name {
        "power.svg" => include_bytes!("../../resources/icons/power.svg"),
        "record.svg" => include_bytes!("../../resources/icons/record.svg"),
        "import.svg" => include_bytes!("../../resources/icons/import.svg"),
        "more.svg" => include_bytes!("../../resources/icons/more.svg"),
        "settings.svg" => include_bytes!("../../resources/icons/settings.svg"),
        "example.svg" => include_bytes!("../../resources/icons/example.svg"),
        "translation.svg" => include_bytes!("../../resources/icons/translation.svg"),
        "media-pause.svg" => include_bytes!("../../resources/icons/media-pause.svg"),
        "media-play.svg" => include_bytes!("../../resources/icons/media-play.svg"),
        "star.svg" => include_bytes!("../../resources/icons/star.svg"),
        "stop.svg" => include_bytes!("../../resources/icons/stop.svg"),
        "pause.svg" => include_bytes!("../../resources/icons/pause.svg"),
        "rabbit.svg" => include_bytes!("../../resources/icons/rabbit.svg"),
        "turtle.svg" => include_bytes!("../../resources/icons/turtle.svg"),
        "file-audio.svg" => include_bytes!("../../resources/icons/file-audio.svg"),
        "file-video.svg" => include_bytes!("../../resources/icons/file-video.svg"),
        "file-subtitles.svg" => include_bytes!("../../resources/icons/file-subtitles.svg"),
        "stream.svg" => include_bytes!("../../resources/icons/stream.svg"),
        "check.svg" => include_bytes!("../../resources/icons/check.svg"),
        "x.svg" => include_bytes!("../../resources/icons/x.svg"),
        "play.svg" => include_bytes!("../../resources/icons/play.svg"),
        "media-collapse.svg" => include_bytes!("../../resources/icons/media-collapse.svg"),
        "media-expand.svg" => include_bytes!("../../resources/icons/media-expand.svg"),
        "media-subtitles.svg" => include_bytes!("../../resources/icons/media-subtitles.svg"),
        "media-audio-tracks.svg" => include_bytes!("../../resources/icons/media-audio-tracks.svg"),
        "export.svg" => include_bytes!("../../resources/icons/export.svg"),
        _ => panic!("Unknown SDK icon: {name}"),
    };
    eframe::egui::ImageSource::Bytes {
        uri: format!("bytes://xrt-icons/{name}").into(),
        bytes: bytes.into(),
    }
}

/// Initialize a native page with the same presentation resources as the shell.
pub fn configure_surface(ctx: &eframe::egui::Context, options: &serde_json::Value) {
    let first = ctx.data_mut(|data| {
        let id = eframe::egui::Id::unique("shared_ui_initialized");
        let first = !data.get_temp::<bool>(id).unwrap_or(false);
        data.insert_temp(id, true);
        first
    });
    if first {
        crate::file_dialog::set_context(ctx.clone());
        notifications::Notifications::default().install(ctx);
        fonts::configure_multilingual_fonts(ctx);
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
