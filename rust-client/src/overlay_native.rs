//! Shared desktop surfaces: subtitles and a transparent screen-translation frame.
mod platform;
#[cfg(target_os = "linux")]
pub(crate) use platform::configure_software_environment;

use crate::i18n::{UiLanguage, tr};
use crate::overlay_ipc::{
    OcrOverlayState, OverlayCommand, OverlayControls, OverlayEvent, OverlayRegion, OverlayState,
};
use crate::ui::{components, components::avatar, theme};
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
            .with_min_inner_size([360.0, 160.0])
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top()
            .with_active(false)
            .with_taskbar(false)
            .with_visible(false),
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
        if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_options.wgpu_setup
        {
            setup.instance_descriptor.backends = eframe::wgpu::Backends::GL;
        }
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
            let graphics = context
                .wgpu_render_state
                .as_ref()
                .ok_or("Avatar rendering is unavailable.")?;
            #[cfg(target_os = "linux")]
            {
                let renderer = graphics.adapter.get_info();
                if !renderer.name.to_ascii_lowercase().contains("llvmpipe") {
                    return Err("Software rendering is unavailable.".into());
                }
                log::info!("Floating window software renderer: {}", renderer.name);
            }
            avatar::install(
                &graphics.device,
                graphics.target_format,
                &mut graphics.renderer.write(),
            );
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
                collapsed: false,
                companion: avatar::Surface::default(),
                companion_region: None,
                pending_region: None,
                sent_region: None,
                closing: false,
                shown: false,
                stacking: WindowStacking::default(),
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
    collapsed: bool,
    companion: avatar::Surface,
    companion_region: Option<OverlayRegion>,
    pending_region: Option<(OverlayRegion, Instant)>,
    sent_region: Option<OverlayRegion>,
    closing: bool,
    shown: bool,
    stacking: WindowStacking,
    result: ResultWindow,
}

#[derive(Default)]
struct WindowStacking {
    state: Option<(bool, bool)>,
}

impl WindowStacking {
    fn update(&mut self, ctx: &egui::Context) {
        let state = ctx.input(|input| {
            let viewport = input.viewport();
            (
                viewport.focused.unwrap_or(false),
                viewport.minimized.unwrap_or(false),
            )
        });
        if self.state != Some(state) {
            // A hidden X11 window is not managed yet, so its initial stacking
            // request can be ignored. Reassert after showing or changing focus
            // without activating the window or repeatedly raising it.
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
                egui::viewport::WindowLevel::AlwaysOnTop,
            ));
            self.state = Some(state);
        }
    }
}

#[derive(Default)]
struct ResultWindow {
    geometry: Option<Rect>,
    shown: bool,
    pending: Option<(OverlayRegion, Instant)>,
    reported: Option<OverlayRegion>,
    changing: bool,
    stacking: WindowStacking,
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
        self.stacking = WindowStacking::default();
    }
}

impl OverlayWindow {
    fn invalidate_region(&mut self) {
        self.pending_region = None;
        if self.sent_region.take().is_some() {
            send_event(OverlayEvent::RegionChanging);
        }
    }

    fn close(&mut self, ctx: &egui::Context) {
        if !self.closing {
            send_event(OverlayEvent::CloseRequested);
        }
        self.closing = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    fn header(&mut self, ui: &mut egui::Ui) -> Rect {
        // Tooltips must never be painted over the area being captured.
        ui.ctx().all_styles_mut(|style| {
            style.interaction.tooltip_delay = if self.controls.ocr_enabled == Some(true) {
                f32::INFINITY
            } else {
                0.5
            };
            style.interaction.tooltip_grace_time = 0.0;
        });
        if self.collapsed {
            // Keep the header height unchanged so folding never moves the OCR area.
            let (rect, response) = ui.allocate_exact_size(
                Vec2::splat(components::INPUT_TOGGLE_SIZE + 14.0),
                egui::Sense::click_and_drag(),
            );
            let response = response.on_hover_cursor(egui::CursorIcon::Grab);
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "XRTranslate")
            });
            if !self.companion.visible() {
                let logo = rect.shrink(4.0);
                ui.painter().circle_filled(
                    logo.center(),
                    logo.width() * 0.5,
                    theme::content_backdrop(true),
                );
                egui::Image::new(egui::include_image!(
                    "../resources/branding/xrtranslate-logo.png"
                ))
                .corner_radius(255)
                .paint_at(ui, logo.shrink(5.0));
            }
            if response.drag_started() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            if response.clicked() {
                self.collapsed = false;
                send_event(OverlayEvent::CompanionDetached(false));
            }
            return rect;
        }
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
                if drag.hovered() || drag.dragged() {
                    ui.painter().rect_filled(
                        drag.rect.shrink2(egui::vec2(1.0, 2.0)),
                        8,
                        theme::text_strong().gamma_multiply(0.08),
                    );
                }
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
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                let collapse = ui.put(
                    Rect::from_min_size(
                        bar.min + egui::vec2(26.0, 0.0),
                        egui::vec2(24.0, bar.height()),
                    ),
                    egui::Button::new(
                        egui::Image::new(egui::include_image!(
                            "../resources/icons/chevron-left.svg"
                        ))
                        .fit_to_exact_size(Vec2::splat(16.0))
                        .tint(theme::text_weak()),
                    )
                    .min_size(egui::vec2(24.0, bar.height()))
                    .corner_radius(10),
                );
                if collapse.clicked() {
                    self.collapsed = true;
                    send_event(OverlayEvent::CompanionDetached(true));
                }
                let step = components::INPUT_TOGGLE_SIZE + 4.0;
                let inputs = usize::from(self.controls.microphone_enabled.is_some())
                    + usize::from(self.controls.system_audio_enabled.is_some())
                    + usize::from(self.controls.ocr_enabled.is_some())
                    + usize::from(self.controls.auto_input_available);
                let left = bar.left() + 72.0;
                let right = (bar.right() - 62.0 - inputs as f32 * step).max(left);
                let first = (bar.center().x - inputs as f32 * step * 0.5).clamp(left, right);
                let translation_x = first
                    + if self.controls.microphone_enabled.is_some() {
                        step
                    } else {
                        0.0
                    };
                let active = self.controls.translation_enabled;
                let label = tr(
                    self.language,
                    if active {
                        "Stop Translation"
                    } else {
                        "Start Translation"
                    },
                );
                let response = icon_button(ui, slot(translation_x), "translation", active, label);
                egui::Image::new(egui::include_image!("../resources/icons/power.svg"))
                    .tint(if active || response.hovered() || response.has_focus() {
                        theme::text_strong()
                    } else {
                        theme::text_weak()
                    })
                    .paint_at(ui, response.rect.shrink(7.0));
                if response.clicked() {
                    send_event(OverlayEvent::TranslationEnabled(!active));
                }
                use components::InputIcon;
                for (icon, enabled, x, id) in [
                    (
                        InputIcon::Microphone,
                        self.controls.microphone_enabled,
                        first,
                        "microphone_input",
                    ),
                    (
                        InputIcon::SystemAudio,
                        self.controls.system_audio_enabled,
                        translation_x + step,
                        "system_audio_input",
                    ),
                    (
                        InputIcon::Text,
                        self.controls.ocr_enabled,
                        translation_x
                            + if self.controls.system_audio_enabled.is_some() {
                                step * 2.0
                            } else {
                                step
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
                        .scope_builder(egui::UiBuilder::new().id_salt(id).max_rect(slot(x)), |ui| {
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
                        if matches!(icon, InputIcon::Text) {
                            self.invalidate_region();
                        }
                        send_event(match icon {
                            InputIcon::Microphone => OverlayEvent::MicrophoneEnabled(!enabled),
                            InputIcon::SystemAudio => OverlayEvent::SystemAudioEnabled(!enabled),
                            InputIcon::Text => OverlayEvent::OcrEnabled(!enabled),
                        });
                    }
                }
                if self.controls.auto_input_available {
                    let active = self.controls.auto_input_enabled;
                    let label = tr(
                        self.language,
                        if active {
                            "Turn off automatic typing of translations into the focused input"
                        } else {
                            "Turn on automatic typing of translations into the focused input"
                        },
                    );
                    let response = icon_button(
                        ui,
                        slot(first + inputs as f32 * step),
                        "auto_input",
                        active,
                        label,
                    );
                    paint_wave_pen(
                        ui.painter(),
                        response.rect.center(),
                        if active || response.hovered() || response.has_focus() {
                            theme::text_strong()
                        } else {
                            theme::text_weak()
                        },
                    );
                    if response.clicked() {
                        send_event(OverlayEvent::AutoInputEnabled(!active));
                    }
                }
                let close = icon_button(
                    ui,
                    slot(bar.right() - components::INPUT_TOGGLE_SIZE * 0.5),
                    "close",
                    false,
                    tr(self.language, "Close"),
                );
                let center = close.rect.center();
                for y in [-1.0, 1.0] {
                    ui.painter().line_segment(
                        [
                            center + egui::vec2(-5.0, y * 5.0),
                            center + egui::vec2(5.0, -y * 5.0),
                        ],
                        Stroke::new(
                            1.6,
                            if close.hovered() {
                                theme::text_strong()
                            } else {
                                theme::text_weak()
                            },
                        ),
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
        let ocr_enabled = self.controls.ocr_enabled == Some(true);
        let ocr = self.ocr.as_ref().filter(|_| ocr_enabled);
        let waiting = self.controls.microphone_enabled == Some(true)
            || self.controls.system_audio_enabled == Some(true);
        // Keep the viewport alive between recognition passes, including empty
        // results. Recreating it also disrupts the capture exclusion region.
        if !ocr_enabled
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
                        if drag.hovered() || drag.dragged() {
                            ui.painter().rect_filled(
                                Rect::from_center_size(handle.center(), egui::vec2(32.0, 12.0)),
                                6,
                                theme::text_strong().gamma_multiply(0.08),
                            );
                        }
                        for x in [-8.0, -4.0, 0.0, 4.0, 8.0] {
                            ui.painter().circle_filled(
                                handle.center() + egui::vec2(x, 0.0),
                                1.0,
                                theme::text_weak(),
                            );
                        }
                        // Activity belongs in the existing header, so capture
                        // and translation updates cannot shift the result list.
                        if ocr.is_some_and(|ocr| ocr.busy) {
                            egui::Spinner::new().color(theme::text_weak()).paint_at(
                                ui,
                                Rect::from_center_size(
                                    egui::pos2(handle.left() + 6.0, handle.center().y),
                                    Vec2::splat(12.0),
                                ),
                            );
                        }
                        if drag.drag_started() {
                            result.moving();
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
                        }
                        ui.style_mut().spacing.scroll = egui::style::ScrollStyle::solid();
                        egui::ScrollArea::vertical()
                            .stick_to_bottom(true)
                            .min_scrolled_height(0.0)
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
                    result.stacking.update(ui.ctx());
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
                        self.invalidate_region();
                    }
                    self.controls = controls;
                }
                OverlayCommand::Subtitles(state) => self.subtitles = state,
                OverlayCommand::Ocr(state) => self.ocr = state,
                OverlayCommand::Companion(presentation) => self.companion.update(presentation),
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
        let collapsed = self.collapsed;
        let mut header_rect = Rect::NOTHING;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.inner_margin(4))
            .show(ui, |ui| {
                let header = self.header(ui);
                header_rect = header;
                regions.push((header, if collapsed { 8.0 } else { CORNER as f32 }));
                anchor = header;
                ui.add_space(GAP);
                if let Some(rect) =
                    recognition_area(ui, self.controls.ocr_enabled == Some(true), &mut regions)
                {
                    selection = Some(rect.shrink(12.0));
                    anchor = header.union(rect);
                }
            });
        let Some(window) = frame.winit_window() else {
            return;
        };
        let bubble = if collapsed {
            let pointer = self
                .input
                .pointer_position(window, ui.ctx().pixels_per_point());
            ui.ctx().request_repaint_after(Duration::from_millis(33));
            self.companion.paint(ui, header_rect, pointer)
        } else {
            None
        };
        let bubble = bubble.map(|rect| rect.intersect(ui.ctx().viewport_rect()));
        if let Some(rect) = bubble {
            regions.push((rect, 0.0));
        }
        if let Ok(position) = window.inner_position() {
            let region = bubble.map(|rect| {
                let rect = rect * ui.ctx().pixels_per_point();
                OverlayRegion {
                    x: position.x + rect.left().floor() as i32,
                    y: position.y + rect.top().floor() as i32,
                    width: (rect.right().ceil() - rect.left().floor()) as u32,
                    height: (rect.bottom().ceil() - rect.top().floor()) as u32,
                }
            });
            if self.companion_region != region {
                self.companion_region = region;
                send_event(OverlayEvent::CompanionRegionChanged(region));
            }
        }
        // Windows clips painting as well as input to this region. Include only
        // tooltips painted this pass, with their shadows, until they disappear.
        let tooltips = ui.ctx().memory(|memory| {
            memory
                .areas()
                .visible_layer_ids()
                .into_iter()
                .filter(|layer| layer.order == egui::Order::Tooltip)
                .filter_map(|layer| memory.area_rect(layer.id).map(|rect| (layer, rect)))
                .collect::<Vec<_>>()
        });
        let shadow = ui.ctx().global_style().visuals.popup_shadow.margin();
        ui.ctx().graphics(|graphics| {
            regions.extend(tooltips.into_iter().filter_map(|(layer, rect)| {
                graphics
                    .get(layer)
                    .filter(|paint| !paint.is_empty())
                    .map(|_| (rect.expand(1.0) + shadow, 0.0))
            }));
        });
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
        self.stacking.update(ui.ctx());
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

/// The OCR toggle owns both the visible frame and its resize handles. Keep the
/// middle out of the native input region so the captured application stays usable.
fn recognition_area(
    ui: &mut egui::Ui,
    enabled: bool,
    regions: &mut Vec<(Rect, f32)>,
) -> Option<Rect> {
    if !enabled {
        return None;
    }
    let rect = ui.available_rect_before_wrap().shrink(2.0);
    let painter = ui.painter();
    // Inward grey shadows stay within the native window's narrow input mask.
    // The capture inset below the caller's frame excludes all of these pixels.
    let shadow_rect = rect.shrink(1.0).translate(egui::vec2(0.0, 0.75));
    for (width, alpha) in [(4.0, 36), (2.5, 80)] {
        painter.rect_stroke(
            shadow_rect,
            0.0,
            Stroke::new(width, Color32::from_rgba_unmultiplied(96, 96, 96, alpha)),
            egui::StrokeKind::Inside,
        );
    }
    painter.rect_stroke(
        rect,
        0.0,
        Stroke::new(1.0, Color32::WHITE),
        egui::StrokeKind::Inside,
    );
    for (corner, direction) in [
        (rect.left_top(), egui::vec2(1.0, 1.0)),
        (rect.right_top(), egui::vec2(-1.0, 1.0)),
        (rect.left_bottom(), egui::vec2(1.0, -1.0)),
        (rect.right_bottom(), egui::vec2(-1.0, -1.0)),
    ] {
        let corner = corner + direction * 1.5;
        let points = [
            corner + egui::vec2(direction.x * 9.0, 0.0),
            corner,
            corner + egui::vec2(0.0, direction.y * 9.0),
        ];
        painter.add(egui::Shape::line(
            points.map(|point| point + direction * 1.0).to_vec(),
            Stroke::new(4.0, Color32::from_rgba_unmultiplied(96, 96, 96, 100)),
        ));
        painter.add(egui::Shape::line(
            points.to_vec(),
            Stroke::new(3.0, Color32::WHITE),
        ));
    }

    use egui::viewport::ResizeDirection::*;
    // Wider invisible handles keep the thin outline easy to grab. Register
    // corners last so diagonal resizing wins where handles overlap.
    for (edge, direction) in [
        (
            Rect::from_min_max(rect.left_top(), rect.right_top() + egui::vec2(0.0, GAP)),
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
            Rect::from_min_max(rect.left_top(), rect.left_bottom() + egui::vec2(GAP, 0.0)),
            West,
        ),
        (
            Rect::from_min_max(rect.right_top() - egui::vec2(GAP, 0.0), rect.right_bottom()),
            East,
        ),
        (
            Rect::from_min_size(rect.left_top(), Vec2::splat(12.0)),
            NorthWest,
        ),
        (
            Rect::from_min_size(rect.right_top() - egui::vec2(12.0, 0.0), Vec2::splat(12.0)),
            NorthEast,
        ),
        (
            Rect::from_min_size(
                rect.left_bottom() - egui::vec2(0.0, 12.0),
                Vec2::splat(12.0),
            ),
            SouthWest,
        ),
        (
            Rect::from_min_size(rect.right_bottom() - Vec2::splat(12.0), Vec2::splat(12.0)),
            SouthEast,
        ),
    ] {
        resize(ui, edge, direction);
        regions.push((edge, 0.0));
    }
    Some(rect)
}

fn result_contents(
    ui: &mut egui::Ui,
    language: UiLanguage,
    subtitles: &OverlayState,
    ocr: Option<&OcrOverlayState>,
    waiting: bool,
) {
    let size = subtitles.font_size.clamp(10, 32) as f32;
    if let Some(status) = ocr.and_then(|ocr| ocr.status.as_ref()) {
        ui.label(RichText::new(status).color(theme::text_weak()));
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

fn icon_button(
    ui: &mut egui::Ui,
    rect: Rect,
    id: &str,
    active: bool,
    label: &str,
) -> egui::Response {
    ui.scope_builder(egui::UiBuilder::new().id_salt(id).max_rect(rect), |ui| {
        components::compact_icon_button(ui, id, active, label, Some(theme::text_strong()))
    })
    .inner
}

fn paint_wave_pen(painter: &egui::Painter, center: egui::Pos2, color: Color32) {
    let stroke = Stroke::new(1.6, color);
    let tip = center + egui::vec2(-7.0, 6.0);
    let axis = egui::vec2(1.0, -1.0) * std::f32::consts::FRAC_1_SQRT_2;
    let normal = egui::vec2(1.0, 1.0) * std::f32::consts::FRAC_1_SQRT_2;
    painter.add(egui::Shape::line(
        vec![
            tip + axis * 4.0 + normal * 2.2,
            tip,
            tip + axis * 4.0 - normal * 2.2,
        ],
        stroke,
    ));
    // The diagonal audio waveform forms the shaft, without a solid pen barrel.
    let wave = (0..=36)
        .map(|index| {
            let t = index as f32 / 36.0;
            tip + axis * (4.0 + 17.0 * t) + normal * ((t * std::f32::consts::TAU * 3.0).sin() * 2.8)
        })
        .collect();
    painter.add(egui::Shape::line(wave, stroke));
    painter.add(egui::Shape::line(
        vec![
            tip,
            center + egui::vec2(-9.0, 9.0),
            center + egui::vec2(-4.0, 9.0),
            center + egui::vec2(-1.0, 7.5),
            center + egui::vec2(2.0, 9.0),
            center + egui::vec2(7.0, 9.0),
        ],
        stroke,
    ));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ocr_capture_updates_do_not_move_results_or_their_scroll_position() {
        for count in [0, 1, 8] {
            let ctx = egui::Context::default();
            let subtitles = OverlayState {
                font_size: 14,
                max_items: count,
                visible_entries: (0..count)
                    .map(|index| crate::overlay_ipc::OverlayEntry {
                        source: format!("source {index}"),
                        translated: format!("translated text {index}"),
                        live: false,
                        vad_active: false,
                    })
                    .collect(),
                partial_text: None,
                vad_active: false,
            };
            let paint = |source: &str, busy, tick| {
                let ocr = OcrOverlayState {
                    source: source.into(),
                    status: None,
                    busy,
                };
                let mut metrics = None;
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(320.0, 240.0),
                        )),
                        time: Some(tick as f64 * 0.1),
                        ..Default::default()
                    },
                    |ui| {
                        egui::CentralPanel::default().show(ui, |ui| {
                            let scroll = egui::ScrollArea::vertical()
                                .stick_to_bottom(true)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    result_contents(
                                        ui,
                                        UiLanguage::English,
                                        &subtitles,
                                        Some(&ocr),
                                        false,
                                    );
                                });
                            metrics = Some((scroll.content_size, scroll.state.offset));
                        });
                    },
                );
                output.textures_delta.clear();
                if count == 1 && tick >= 4 {
                    let texts: Vec<_> = output
                        .shapes
                        .iter()
                        .filter_map(|shape| match &shape.shape {
                            egui::Shape::Text(text) => Some(text.galley.text()),
                            _ => None,
                        })
                        .collect();
                    // A translated card keeps its original + translation;
                    // only the standalone recognition observation is hidden.
                    assert!(texts.contains(&"source 0"));
                    assert!(texts.contains(&"translated text 0"));
                    assert!(!texts.iter().any(|text| text.contains("screen text")));
                }
                metrics.unwrap()
            };
            let mut settled = paint("", false, 0);
            for tick in 1..4 {
                settled = paint("", false, tick);
            }
            for tick in 4..12 {
                // Raw OCR observations (including empty captures) never become
                // list rows. Only translated entries may change its geometry.
                let source = if tick % 3 == 0 {
                    ""
                } else {
                    "The same screen text\nwith another recognized line"
                };
                assert_eq!(paint(source, tick % 2 == 0, tick), settled);
            }
        }
    }

    fn frame(
        ctx: &egui::Context,
        enabled: bool,
        time: f64,
        events: Vec<egui::Event>,
    ) -> (Option<Rect>, Vec<(Rect, f32)>, egui::FullOutput) {
        let mut area = None;
        let mut regions = Vec::new();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(460.0, 360.0),
                )),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ui| {
                ui.add_space(60.0);
                area = recognition_area(ui, enabled, &mut regions);
            },
        );
        output.textures_delta.clear();
        (area, regions, output)
    }

    #[test]
    fn ocr_toggle_controls_frame_and_handles_without_blocking_captured_content() {
        let ctx = egui::Context::default();
        let (area, regions, _) = frame(&ctx, false, 0.0, vec![]);
        assert!(area.is_none() && regions.is_empty());
        for tick in 1..=3 {
            let (area, regions, _) = frame(&ctx, true, tick as f64, vec![]);
            let area = area.unwrap();
            assert_eq!(regions.len(), 8);
            let capture = area.shrink(12.0);
            for (hit, _) in regions {
                assert!(hit.is_positive());
                assert!(!hit.intersect(capture).is_positive());
                assert!(!hit.contains(area.center()));
            }
        }
        let (area, regions, _) = frame(&ctx, false, 4.0, vec![]);
        assert!(area.is_none() && regions.is_empty());
    }

    #[test]
    fn enabled_ocr_resizes_from_every_edge_and_corner_without_edit_mode() {
        use egui::viewport::ResizeDirection::*;
        for (index, direction) in [
            North, South, West, East, NorthWest, NorthEast, SouthWest, SouthEast,
        ]
        .into_iter()
        .enumerate()
        {
            let ctx = egui::Context::default();
            let (_, regions, _) = frame(&ctx, true, 0.0, vec![]);
            let point = regions[index].0.center();
            frame(&ctx, true, 0.1, vec![egui::Event::PointerMoved(point)]);
            let mut began_resize = false;
            for (time, events) in [
                (
                    0.2,
                    vec![egui::Event::PointerButton {
                        pos: point,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    }],
                ),
                (
                    0.3,
                    vec![egui::Event::PointerMoved(point + egui::vec2(8.0, 8.0))],
                ),
            ] {
                let (_, _, output) = frame(&ctx, true, time, events);
                began_resize |= output.viewport_output.values().any(|output| {
                    output.commands.iter().any(|command| {
                        matches!(command, egui::ViewportCommand::BeginResize(actual) if *actual == direction)
                    })
                });
            }
            assert!(began_resize, "missing resize command for {direction:?}");
        }
    }
}
