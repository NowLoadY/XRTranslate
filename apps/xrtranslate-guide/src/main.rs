#![cfg_attr(windows, windows_subsystem = "windows")]
mod labels;
mod remote;
mod window;
mod worker;

use eframe::egui;
use std::{path::PathBuf, sync::Arc};

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let option = |key| {
        args.iter()
            .position(|arg| arg == key)
            .and_then(|i| args.get(i + 1))
    };
    let port = option("--port")
        .and_then(|s| s.parse().ok())
        .unwrap_or(ui_director::DEFAULT_PORT);
    let director_port = option("--director-port").and_then(|s| s.parse().ok());
    let client = option("--client").map(PathBuf::from);
    let language = labels::system_language();
    eframe::run_native(
        labels::text("User Guide", language),
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([540.0, 300.0])
                .with_min_inner_size([480.0, 280.0])
                .with_icon(
                    eframe::icon_data::from_png_bytes(include_bytes!(
                        "../../../rust-client/resources/branding/xrtranslate-logo.png"
                    ))
                    .expect("bundled application icon"),
                )
                .with_always_on_top(),
            renderer: eframe::Renderer::Glow,
            ..Default::default()
        },
        Box::new(move |cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::light());
            let mut fonts = egui::FontDefinitions::default();
            let root =
                PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| "C:/Windows".into()))
                    .join("Fonts");
            for name in ["msyh.ttc", "malgun.ttf", "YuGothM.ttc"] {
                if let Ok(bytes) = std::fs::read(root.join(name)) {
                    fonts
                        .font_data
                        .insert(name.into(), Arc::new(egui::FontData::from_owned(bytes)));
                    fonts
                        .families
                        .get_mut(&egui::FontFamily::Proportional)
                        .unwrap()
                        .push(name.into());
                }
            }
            cc.egui_ctx.set_fonts(fonts);
            cc.egui_ctx.style_mut_of(egui::Theme::Light, |style| {
                style.spacing.item_spacing = egui::vec2(10.0, 12.0);
                style.spacing.button_padding = egui::vec2(14.0, 7.0);
                style.visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(10);
                style.visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(10);
                style.visuals.widgets.active.corner_radius = egui::CornerRadius::same(10);
                style.always_scroll_the_only_direction = true;
                style.visuals.panel_fill = egui::Color32::from_rgb(250, 250, 252);
                style.visuals.selection.bg_fill = egui::Color32::from_rgb(220, 232, 255);
                style.visuals.selection.stroke.color = egui::Color32::from_rgb(46, 98, 220);
            });
            if let Some(port) = director_port {
                ui_director::egui::init(cc.egui_ctx.clone(), Some(port));
            }
            Ok(Box::new(window::Window::new(
                port,
                client,
                cc.egui_ctx.clone(),
                language,
            )))
        }),
    )
}
