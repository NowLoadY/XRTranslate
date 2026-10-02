//! Shared desktop surfaces: subtitles and a transparent screen-translation frame.
mod platform;
#[cfg(target_os = "linux")]
pub(crate) use platform::configure_software_environment;

use crate::i18n::{UiLanguage, tr};
use crate::overlay_ipc::{
    OcrOverlayState, OverlayCommand, OverlayControls, OverlayEvent, OverlayRegion, OverlayState,
};
use crate::ui::{components, theme};
use eframe::egui::{self, Color32, Rect, RichText, Stroke, Vec2};
use std::{
    io::{BufRead, Write},
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};

const GAP: f32 = 6.0;
const CORNER: u8 = 16;

pub fn run_native_overlay() -> eframe::Result<()> {
    #[cfg(target_os = "linux")]
    platform::validate_software_environment().map_err(eframe::Error::AppCreation)?;
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("XRTranslate Floating Window")
            .with_inner_size([460.0, 360.0])
            .with_min_inner_size([320.0, 160.0])
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top()
            .with_active(false)
            .with_taskbar(false)
            .with_visible(false),
        #[cfg(target_os = "linux")]
        renderer: eframe::Renderer::Glow,
        #[cfg(windows)]
        renderer: eframe::Renderer::Wgpu,
        persist_window: false,
        ..Default::default()
    };
    #[cfg(target_os = "linux")]
    {
        // XWayland supplies the positioning and stacking needed by a desktop
        // overlay, while the main application can keep its native Wayland UI.
        use winit::platform::x11::EventLoopBuilderExtX11;
        options.event_loop_builder = Some(Box::new(|builder| {
            builder.with_x11();
        }));
    }
    #[cfg(windows)]
    {
        options = crate::configure_transparent_wgpu(
            options,
            crate::window_backdrop::WindowBackdrop::Transparent,
        );
    }
    eframe::run_native(
        "XRTranslate Floating Window",
        options,
        Box::new(|context| {
            #[cfg(target_os = "linux")]
            {
                use eframe::glow::HasContext;
                let gl = context
                    .gl
                    .as_ref()
                    .ok_or("Software rendering is unavailable.")?;
                let renderer = unsafe { gl.get_parameter_string(eframe::glow::RENDERER) };
                if !renderer.to_ascii_lowercase().contains("llvmpipe") {
                    return Err("Software rendering is unavailable.".into());
                }
                log::info!("Floating window software renderer: {renderer}");
            }
            crate::ui::fonts::configure_multilingual_fonts(&context.egui_ctx);
            theme::apply_theme(&context.egui_ctx);
            context.egui_ctx.all_styles_mut(|style| {
                for widget in [
                    &mut style.visuals.widgets.hovered,
                    &mut style.visuals.widgets.active,
                    &mut style.visuals.widgets.open,
                ] {
                    widget.fg_stroke.color = theme::text_strong();
                }
                style.visuals.hyperlink_color = theme::text_strong();
            });
            egui_extras::install_image_loaders(&context.egui_ctx);
            #[cfg(windows)]
            crate::window_backdrop::apply(
                context,
                crate::window_backdrop::WindowBackdrop::Transparent,
            )?;
            let window = context
                .winit_window()
                .ok_or("Unable to open the floating window.")?;
            let input = platform::InputRegion::new(window)?;
            let work = input.work_area(window);
            let size = window.inner_size();
            window.set_outer_position(winit::dpi::PhysicalPosition::new(
                (work.right() - size.width as f32 - 36.0).max(work.left()) as i32,
                (work.top() + 36.0) as i32,
            ));
            let (sender, receiver) = mpsc::sync_channel(2);
            let ctx = context.egui_ctx.clone();
            std::thread::spawn(move || {
                for line in std::io::stdin().lock().lines().map_while(Result::ok) {
                    let command = if line == "HIDE" {
                        Ok(OverlayCommand::Hide)
                    } else {
                        serde_json::from_str(&line)
                    };
                    if let Ok(command) = command {
                        if sender.send(command).is_err() {
                            return;
                        }
                        ctx.request_repaint();
                    }
                }
                let _ = sender.send(OverlayCommand::Hide);
                ctx.request_repaint();
            });
            Ok(Box::new(OverlayWindow {
                receiver,
                input,
                language: UiLanguage::English,
                controls: OverlayControls::default(),
                subtitles: OverlayState {
                    font_size: 14,
                    max_items: 5,
                    visible_entries: Vec::new(),
                    partial_text: None,
                    vad_active: false,
                },
                ocr: None,
                edit_region: false,
                pending_region: None,
                sent_region: None,
                closing: false,
                shown: false,
                result: ResultWindow::default(),
            }))
        }),
    )
}

struct OverlayWindow {
    receiver: Receiver<OverlayCommand>,
    input: platform::InputRegion,
    language: UiLanguage,
    controls: OverlayControls,
    subtitles: OverlayState,
    ocr: Option<OcrOverlayState>,
    edit_region: bool,
    pending_region: Option<(OverlayRegion, Instant)>,
    sent_region: Option<OverlayRegion>,
    closing: bool,
    shown: bool,
    result: ResultWindow,
}

#[derive(Default)]
struct ResultWindow {
    geometry: Option<Rect>,
    shown: bool,
    pending: Option<(OverlayRegion, Instant)>,
    reported: Option<OverlayRegion>,
    changing: bool,
}

impl ResultWindow {
    fn moving(&mut self) {
        if !self.changing {
            send_event(OverlayEvent::ResultChanging);
            self.changing = true;
        }
        self.reported = None;
        if let Some((_, changed)) = &mut self.pending {
            *changed = Instant::now();
        }
    }

    fn observe(&mut self, rect: Rect) {
        self.geometry = Some(rect);
        // Cover the complete result window, including its antialiased edges.
        let region = OverlayRegion {
            x: rect.left().floor() as i32,
            y: rect.top().floor() as i32,
            width: (rect.right().ceil() - rect.left().floor()) as u32,
            height: (rect.bottom().ceil() - rect.top().floor()) as u32,
        };
        let now = Instant::now();
        if self.pending.is_none_or(|(previous, _)| previous != region) {
            self.moving();
            self.pending = Some((region, now));
        }
        if self.reported != Some(region)
            && self.pending.is_some_and(|(_, changed)| {
                now.duration_since(changed) >= Duration::from_millis(300)
            })
        {
            self.reported = Some(region);
            self.changing = false;
            send_event(OverlayEvent::ResultRegionChanged(Some(region)));
        }
    }

    fn hide(&mut self) {
        if self.shown || self.changing {
            send_event(OverlayEvent::ResultRegionChanged(None));
        }
        self.shown = false;
        self.pending = None;
        self.reported = None;
        self.changing = false;
    }
}

impl OverlayWindow {
    fn close(&mut self, ctx: &egui::Context) {
        if !self.closing {
            send_event(OverlayEvent::CloseRequested);
        }
        self.closing = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    fn header(&mut self, ui: &mut egui::Ui) -> Rect {
        panel(6)
            .show(ui, |ui| {
                ui.spacing_mut().button_padding = Vec2::splat(4.0);
                let (bar, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), components::INPUT_TOGGLE_SIZE),
                    egui::Sense::hover(),
                );
                let slot = |x| {
                    Rect::from_center_size(
                        egui::pos2(x, bar.center().y),
                        Vec2::splat(components::INPUT_TOGGLE_SIZE),
                    )
                };
                let drag = ui
                    .interact(
                        Rect::from_min_size(bar.min, egui::vec2(26.0, bar.height())),
                        ui.id().with("drag"),
                        egui::Sense::drag(),
                    )
                    .on_hover_cursor(egui::CursorIcon::Grab);
                for x in [10.0, 15.0] {
                    for y in [-5.0, 0.0, 5.0] {
                        ui.painter().circle_filled(
                            egui::pos2(bar.left() + x, bar.center().y + y),
                            1.2,
                            theme::text_weak(),
                        );
                    }
                }
                if drag.drag_started() {
                    self.edit_region |= self.controls.ocr_enabled == Some(true);
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                if self.controls.ocr_enabled.is_some() {
                    let label = tr(
                        self.language,
                        if self.edit_region {
                            "Confirm recognition area"
                        } else {
                            "Adjust recognition area"
                        },
                    );
                    let response = icon_button(ui, slot(bar.left() + 50.0), label);
                    let center = response.rect.center();
                    let stroke = Stroke::new(1.6, theme::text_normal());
                    if self.edit_region {
                        ui.painter().add(egui::Shape::line(
                            vec![
                                center + egui::vec2(-7.0, 0.0),
                                center + egui::vec2(-2.0, 5.0),
                                center + egui::vec2(8.0, -6.0),
                            ],
                            stroke,
                        ));
                    } else {
                        for (x, y) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                            let corner = center + egui::vec2(x, y) * 9.0;
                            ui.painter().add(egui::Shape::line(
                                vec![
                                    corner - egui::vec2(x * 5.0, 0.0),
                                    corner,
                                    corner - egui::vec2(0.0, y * 5.0),
                                ],
                                stroke,
                            ));
                        }
                    }
                    if response.clicked() {
                        self.edit_region = !self.edit_region;
                    }
                }
                // Tooltips must never be painted over the area being captured.
                ui.ctx().all_styles_mut(|style| {
                    style.interaction.tooltip_delay =
                        if self.edit_region || self.controls.ocr_enabled == Some(true) {
                            f32::INFINITY
                        } else {
                            0.5
                        };
                    style.interaction.tooltip_grace_time = 0.0;
                });
                let active = self.controls.translation_enabled;
                let label = tr(
                    self.language,
                    if active {
                        "Stop Translation"
                    } else {
                        "Start Translation"
                    },
                );
                let response = ui
                    .scope_builder(
                        egui::UiBuilder::new().max_rect(slot(bar.center().x)),
                        |ui| {
                            ui.add(
                                egui::Button::image(
                                    egui::Image::new(egui::include_image!(
                                        "../resources/icons/translation.svg"
                                    ))
                                    .fit_to_exact_size(Vec2::splat(22.0)),
                                )
                                .min_size(Vec2::splat(components::INPUT_TOGGLE_SIZE))
                                .corner_radius(12)
                                .fill(if active {
                                    theme::text_strong().gamma_multiply(0.16)
                                } else {
                                    theme::surface_subtle()
                                })
                                .stroke(if active {
                                    Stroke::new(1.5, theme::text_strong())
                                } else {
                                    Stroke::new(1.0, theme::border())
                                }),
                            )
                        },
                    )
                    .inner;
                response.widget_info(|| {
                    egui::WidgetInfo::selected(egui::WidgetType::Button, true, active, label)
                });
                if response.on_hover_text(label).clicked() {
                    send_event(OverlayEvent::TranslationEnabled(!active));
                }
                use components::InputIcon;
                for (icon, enabled, x, id) in [
                    (
                        InputIcon::Microphone,
                        self.controls.microphone_enabled,
                        bar.center().x - 44.0,
                        "microphone_input",
                    ),
                    (
                        InputIcon::SystemAudio,
                        self.controls.system_audio_enabled,
                        bar.center().x
                            + if self.controls.microphone_enabled.is_some() {
                                44.0
                            } else {
                                -44.0
                            },
                        "system_audio_input",
                    ),
                    (
                        InputIcon::Text,
                        self.controls.ocr_enabled,
                        bar.center().x
                            + if self.controls.microphone_enabled.is_some()
                                && self.controls.system_audio_enabled.is_some()
                            {
                                88.0
                            } else {
                                44.0
                            },
                        "screen_text_input",
                    ),
                ] {
                    let Some(enabled) = enabled else { continue };
                    let label = tr(
                        self.language,
                        match (icon, enabled) {
                            (InputIcon::Microphone, true) => {
                                "Turn off microphone input (including meetings)"
                            }
                            (InputIcon::Microphone, false) => {
                                "Turn on microphone input (including meetings)"
                            }
                            (InputIcon::SystemAudio, true) => "Turn off system audio translation",
                            (InputIcon::SystemAudio, false) => "Turn on system audio translation",
                            (InputIcon::Text, true) => "Turn off screen text translation",
                            (InputIcon::Text, false) => "Turn on screen text translation",
                        },
                    );
                    let response = ui
                        .scope_builder(egui::UiBuilder::new().max_rect(slot(x)), |ui| {
                            components::input_toggle_with_accent(
                                ui,
                                id,
                                enabled,
                                icon,
                                label,
                                Some(theme::text_strong()),
                            )
                        })
                        .inner;
                    if response.clicked() {
                        if matches!(icon, InputIcon::Text) && !enabled {
                            self.edit_region = true;
                            self.pending_region = None;
                            if self.sent_region.take().is_some() {
                                send_event(OverlayEvent::RegionChanging);
                            }
                        }
                        send_event(match icon {
                            InputIcon::Microphone => OverlayEvent::MicrophoneEnabled(!enabled),
                            InputIcon::SystemAudio => OverlayEvent::SystemAudioEnabled(!enabled),
                            InputIcon::Text => OverlayEvent::OcrEnabled(!enabled),
                        });
                    }
                }
                let close = icon_button(ui, slot(bar.right() - 18.0), tr(self.language, "Close"));
                let center = close.rect.center();
                for y in [-1.0, 1.0] {
                    ui.painter().line_segment(
                        [
                            center + egui::vec2(-5.0, y * 5.0),
                            center + egui::vec2(5.0, -y * 5.0),
                        ],
                        Stroke::new(1.6, theme::text_weak()),
                    );
                }
                if close.clicked() {
                    self.close(ui.ctx());
                }
            })
            .response
            .rect
    }

    fn result(&mut self, ctx: &egui::Context, frame: &eframe::Frame, anchor: Rect) {
        let ocr = self.ocr.as_ref().filter(|ocr| {
            self.controls.ocr_enabled == Some(true)
                && (!ocr.source.is_empty()
                    || !ocr.translated.is_empty()
                    || ocr.status.is_some())
        });
        let waiting = self.controls.microphone_enabled == Some(true)
            || self.controls.system_audio_enabled == Some(true);
        if ocr.is_none()
            && self.subtitles.visible_entries.is_empty()
            && self.subtitles.partial_text.is_none()
            && !waiting
        {
            self.result.hide();
            return;
        }
        let Some(window) = frame.winit_window() else {
            return;
        };
        let Ok(position) = window.inner_position() else {
            return;
        };
        let scale = ctx.pixels_per_point();
        let work = (self.input.work_area(window) / scale).shrink(8.0);
        let anchor = anchor.translate(egui::vec2(position.x as f32, position.y as f32) / scale);
        let default = beside(anchor, work, |_| 240.0).unwrap_or_else(|| {
            let size = egui::vec2(320.0_f32.min(work.width()), 240.0_f32.min(work.height()));
            fit_rect(size, anchor.center(), work)
        }) * scale;
        let result = &mut self.result;
        let desired = result.geometry.unwrap_or(default);
        let subtitles = &self.subtitles;
        let language = self.language;
        let mut close_requested = false;
        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("translation_results"),
            egui::ViewportBuilder::default()
                .with_title(tr(language, "Translation"))
                .with_inner_size([320.0, 240.0])
                .with_min_inner_size([120.0, 80.0])
                .with_decorations(false)
                .with_transparent(false)
                .with_resizable(true)
                .with_always_on_top()
                .with_active(false)
                .with_taskbar(false)
                .with_visible(false),
            |ui, _| {
                // Initial placement uses this window's DPI, which can differ from the bar's.
                let child_scale = ui.ctx().pixels_per_point();
                let actual = ui.ctx().input(|input| input.viewport().inner_rect);
                close_requested = ui.ctx().input(|input| input.viewport().close_requested());
                if !result.shown {
                    let desired = desired / child_scale;
                    let aligned = actual
                        .is_some_and(|actual| rect_matches(actual, desired, 1.0 / child_scale));
                    if aligned != result.shown {
                        ui.ctx()
                            .send_viewport_cmd(egui::ViewportCommand::Visible(aligned));
                        result.shown = aligned;
                    }
                    if !aligned {
                        if actual.is_none_or(|actual| {
                            actual.min.distance(desired.min) > 1.0 / child_scale
                        }) {
                            ui.ctx()
                                .send_viewport_cmd(egui::ViewportCommand::OuterPosition(
                                    desired.min,
                                ));
                        }
                        if actual.is_none_or(|actual| {
                            (actual.size() - desired.size()).length() > 1.0 / child_scale
                        }) {
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::InnerSize(
                                desired.size(),
                            ));
                        }
                        ui.ctx().request_repaint_after(Duration::from_millis(16));
                    }
                }
                ui.painter()
                    .rect_filled(ui.max_rect().expand(1.0), 0, Color32::from_gray(250));
                egui::CentralPanel::default()
                    .frame(panel(10).fill(Color32::from_gray(250)).corner_radius(0))
                    .show(ui, |ui| {
                        let (handle, _) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width(), 12.0),
                            egui::Sense::hover(),
                        );
                        let drag = ui
                            .interact(handle, ui.id().with("drag_results"), egui::Sense::drag())
                            .on_hover_cursor(egui::CursorIcon::Grab);
                        for x in [-8.0, -4.0, 0.0, 4.0, 8.0] {
                            ui.painter().circle_filled(
                                handle.center() + egui::vec2(x, 0.0),
                                1.0,
                                theme::text_weak(),
                            );
                        }
                        if drag.drag_started() {
                            result.moving();
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
                        }
                        ui.style_mut().spacing.scroll = egui::style::ScrollStyle::solid();
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                result_contents(ui, language, subtitles, ocr, waiting);
                            });
                        let corner = Rect::from_min_size(
                            ui.max_rect().right_bottom() - Vec2::splat(12.0),
                            Vec2::splat(12.0),
                        );
                        ui.painter().line_segment(
                            [corner.left_bottom(), corner.right_top()],
                            Stroke::new(1.5, theme::text_weak()),
                        );
                        if resize(ui, corner, egui::viewport::ResizeDirection::SouthEast) {
                            result.moving();
                        }
                    });
                if result.shown
                    && let Some(actual) = actual
                {
                    result.observe(actual * child_scale);
                }
            },
        );
        if close_requested {
            self.close(ctx);
        }
        ctx.request_repaint_after(Duration::from_millis(100));
    }
}

impl eframe::App for OverlayWindow {
    fn clear_color(&self, _: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        while let Ok(command) = self.receiver.try_recv() {
            match command {
                OverlayCommand::Language(language) => self.language = language,
                OverlayCommand::Controls(controls) => {
                    if controls.ocr_enabled != self.controls.ocr_enabled {
                        self.edit_region = controls.ocr_enabled == Some(true);
                    }
                    self.controls = controls;
                }
                OverlayCommand::Subtitles(state) => self.subtitles = state,
                OverlayCommand::Ocr(state) => self.ocr = state,
                OverlayCommand::Hide => {
                    self.closing = true;
                    self.close(ui.ctx());
                }
            }
        }
        if ui.ctx().input(|input| input.viewport().close_requested()) {
            self.close(ui.ctx());
        }
        if self.closing {
            return;
        }
        let mut regions = Vec::new();
        let mut selection = None;
        let mut anchor = Rect::NOTHING;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.inner_margin(4))
            .show(ui, |ui| {
                let header = self.header(ui);
                regions.push((header, CORNER as f32));
                anchor = header;
                ui.add_space(GAP);
                if self.edit_region
                    || self.controls.ocr_enabled == Some(true)
                    || (self.pending_region.is_some() && self.sent_region.is_none())
                {
                    let rect = ui.available_rect_before_wrap().shrink(2.0);
                    selection = Some(rect.shrink(12.0));
                    anchor = header.union(rect);
                    if self.edit_region {
                        let color = Color32::WHITE;
                        ui.painter().rect_stroke(
                            rect.translate(egui::vec2(0.0, 1.0)),
                            0.0,
                            Stroke::new(5.0, Color32::from_black_alpha(112)),
                            egui::StrokeKind::Inside,
                        );
                        ui.painter().rect_stroke(
                            rect,
                            0.0,
                            Stroke::new(2.0, color),
                            egui::StrokeKind::Inside,
                        );
                        use egui::viewport::ResizeDirection::*;
                        for (edge, direction) in [
                            (
                                Rect::from_min_max(
                                    rect.left_top(),
                                    rect.right_top() + egui::vec2(0.0, GAP),
                                ),
                                North,
                            ),
                            (
                                Rect::from_min_max(
                                    rect.left_bottom() - egui::vec2(0.0, GAP),
                                    rect.right_bottom(),
                                ),
                                South,
                            ),
                            (
                                Rect::from_min_max(
                                    rect.left_top(),
                                    rect.left_bottom() + egui::vec2(GAP, 0.0),
                                ),
                                West,
                            ),
                            (
                                Rect::from_min_max(
                                    rect.right_top() - egui::vec2(GAP, 0.0),
                                    rect.right_bottom(),
                                ),
                                East,
                            ),
                        ] {
                            resize(ui, edge, direction);
                            regions.push((edge, 0.0));
                        }
                        for (point, direction) in [
                            (rect.left_top(), NorthWest),
                            (rect.right_top() - egui::vec2(12.0, 0.0), NorthEast),
                            (rect.left_bottom() - egui::vec2(0.0, 12.0), SouthWest),
                            (rect.right_bottom() - egui::vec2(12.0, 12.0), SouthEast),
                        ] {
                            let corner = Rect::from_min_size(point, Vec2::splat(12.0));
                            ui.painter().rect_filled(corner, 2.0, color);
                            resize(ui, corner, direction);
                            regions.push((corner, 2.0));
                        }
                    }
                }
            });
        let Some(window) = frame.winit_window() else {
            return;
        };
        if self
            .input
            .update(&regions, ui.ctx().pixels_per_point())
            .is_err()
        {
            self.close(ui.ctx());
            return;
        }
        if !self.shown {
            window.set_visible(true);
            self.shown = true;
        }
        if let Some(selection) = selection {
            let Ok(position) = window.inner_position() else {
                return;
            };
            let scale = ui.ctx().pixels_per_point();
            let rect = selection * scale;
            let region = OverlayRegion {
                x: position.x + rect.left().ceil() as i32,
                y: position.y + rect.top().ceil() as i32,
                width: (rect.right().floor() - rect.left().ceil()).max(1.0) as u32,
                height: (rect.bottom().floor() - rect.top().ceil()).max(1.0) as u32,
            };
            let now = Instant::now();
            if self
                .pending_region
                .as_ref()
                .is_none_or(|(pending, _)| *pending != region)
            {
                if self.sent_region.take().is_some() {
                    send_event(OverlayEvent::RegionChanging);
                }
                self.pending_region = Some((region, now));
            }
            if self.sent_region != Some(region)
                && self.pending_region.is_some_and(|(_, changed)| {
                    now.duration_since(changed) >= Duration::from_millis(300)
                })
            {
                self.sent_region = Some(region);
                send_event(OverlayEvent::RegionChanged(region));
            }
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        self.result(ui.ctx(), frame, anchor);
    }
}

fn result_contents(
    ui: &mut egui::Ui,
    language: UiLanguage,
    subtitles: &OverlayState,
    ocr: Option<&OcrOverlayState>,
    waiting: bool,
) {
    let size = subtitles.font_size.clamp(10, 32) as f32;
    if let Some(ocr) = ocr {
        let (text, color) = if ocr.translated.is_empty() {
            (&ocr.source, theme::text_weak())
        } else {
            (&ocr.translated, theme::text_strong())
        };
        if !text.is_empty() {
            ui.add(egui::Label::new(RichText::new(text).size(size).color(color)).selectable(true));
        }
        if let Some(status) = &ocr.status {
            ui.label(RichText::new(status).color(theme::text_weak()));
        }
        if ocr.busy {
            ui.add(egui::Spinner::new().size(14.0).color(theme::text_weak()));
        }
        ui.add_space(GAP);
    }
    for entry in &subtitles.visible_entries {
        card(
            ui,
            &entry.source,
            &entry.translated,
            size,
            entry.vad_active,
            entry.live,
        );
        ui.add_space(GAP);
    }
    if let Some(partial) = &subtitles.partial_text {
        card(ui, "", partial, size, subtitles.vad_active, true);
    } else if waiting && subtitles.visible_entries.is_empty() {
        card(
            ui,
            "",
            tr(language, "Waiting for speech"),
            size,
            subtitles.vad_active,
            false,
        );
    }
}

fn panel(margin: i8) -> egui::Frame {
    egui::Frame::new()
        .fill(theme::content_backdrop(true))
        .stroke(Stroke::new(1.0, theme::border().gamma_multiply(0.35)))
        .corner_radius(CORNER)
        .inner_margin(margin)
}

fn icon_button(ui: &mut egui::Ui, rect: Rect, label: &str) -> egui::Response {
    let response = ui
        .scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
            ui.add(
                egui::Button::new("")
                    .min_size(rect.size())
                    .corner_radius(12)
                    .frame(false),
            )
        })
        .inner;
    response
        .widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, false, label));
    response.on_hover_text(label)
}

fn card(
    ui: &mut egui::Ui,
    source: &str,
    translated: &str,
    size: f32,
    active: bool,
    live: bool,
) -> Rect {
    let border = if active {
        theme::text_strong()
    } else if live {
        theme::text_normal()
    } else {
        theme::border()
    };
    panel(10)
        .stroke(Stroke::new(1.0, border.gamma_multiply(0.35)))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if !source.is_empty() {
                ui.label(
                    RichText::new(source)
                        .size(size * 0.8)
                        .color(theme::text_weak()),
                );
            }
            if !translated.is_empty() {
                ui.label(
                    RichText::new(translated)
                        .size(size)
                        .color(theme::text_strong()),
                );
            }
        })
        .response
        .rect
        .intersect(ui.clip_rect())
}

fn resize(ui: &mut egui::Ui, rect: Rect, direction: egui::viewport::ResizeDirection) -> bool {
    use egui::viewport::ResizeDirection::*;
    let cursor = match direction {
        North | South => egui::CursorIcon::ResizeVertical,
        East | West => egui::CursorIcon::ResizeHorizontal,
        NorthWest | SouthEast => egui::CursorIcon::ResizeNwSe,
        NorthEast | SouthWest => egui::CursorIcon::ResizeNeSw,
    };
    let response = ui
        .interact(
            rect,
            ui.id().with(("resize", direction as u8)),
            egui::Sense::drag(),
        )
        .on_hover_cursor(cursor);
    if response.drag_started() {
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::BeginResize(direction));
    }
    response.drag_started()
}

fn rect_matches(a: Rect, b: Rect, tolerance: f32) -> bool {
    a.min.distance(b.min) <= tolerance && (a.size() - b.size()).length() <= tolerance
}

fn fit_rect(size: Vec2, center: egui::Pos2, bounds: Rect) -> Rect {
    // Equal sizes can invert the clamp range by a fraction at noninteger DPI.
    let max = (bounds.max - size).max(bounds.min);
    Rect::from_min_size((center - size * 0.5).clamp(bounds.min, max), size)
}

fn side_spaces(anchor: Rect, bounds: Rect) -> [Rect; 4] {
    let gap = 8.0;
    [
        Rect::from_min_max(egui::pos2(anchor.right() + gap, bounds.top()), bounds.max),
        Rect::from_min_max(bounds.min, egui::pos2(anchor.left() - gap, bounds.bottom())),
        Rect::from_min_max(bounds.min, egui::pos2(bounds.right(), anchor.top() - gap)),
        Rect::from_min_max(egui::pos2(bounds.left(), anchor.bottom() + gap), bounds.max),
    ]
    .map(|space| space.intersect(bounds))
}

fn beside(anchor: Rect, bounds: Rect, height_for_width: impl Fn(f32) -> f32) -> Option<Rect> {
    side_spaces(anchor, bounds)
        .into_iter()
        .filter(|space| space.width() >= 120.0 && space.height() >= 80.0)
        .map(|space| {
            let width = 320.0_f32.min(space.width());
            let height = height_for_width(width).clamp(80.0, 360.0);
            let size = egui::vec2(width, height.min(space.height()));
            let rect = fit_rect(size, anchor.center(), space);
            (
                rect,
                (height - size.y) / height,
                rect.center().distance_sq(anchor.center()),
            )
        })
        .min_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.2.total_cmp(&b.2)))
        .map(|(rect, _, _)| rect)
}

fn send_event(event: OverlayEvent) {
    let mut stdout = std::io::stdout().lock();
    if serde_json::to_writer(&mut stdout, &event).is_ok() {
        let _ = stdout.write_all(b"\n");
        let _ = stdout.flush();
    }
}
