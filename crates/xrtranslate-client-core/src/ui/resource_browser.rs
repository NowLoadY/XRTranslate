use eframe::egui;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use crate::{
    i18n::{UiLanguage, tr},
    media_import::discovery::{self, MediaKind, MediaResource},
    ui::{components, theme},
};

#[derive(Default)]
pub struct ResourceBrowser {
    pub url: String,
    resources: Vec<MediaResource>,
    selected: Option<usize>,
    pub choosing: bool,
    query: String,
    input_snapshot: String,
    resolve_after: Option<Instant>,
    pending: Option<Lookup>,
}

struct Lookup {
    stop: Arc<AtomicBool>,
    result: crossbeam_channel::Receiver<Result<Vec<MediaResource>, String>>,
}

impl Drop for Lookup {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

impl ResourceBrowser {
    pub fn render_selection(
        &mut self,
        ui: &mut egui::Ui,
        language: UiLanguage,
        proxy: &str,
    ) -> Result<bool, String> {
        if self.input_snapshot != self.url.trim() {
            self.invalidate();
            self.input_snapshot = self.url.trim().to_owned();
            self.resolve_after = Some(Instant::now() + Duration::from_millis(350));
        }
        if let Some(when) = self.resolve_after {
            let remaining = when.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                self.resolve_after = None;
                if !self.url.trim().is_empty() {
                    self.load(proxy.to_owned(), ui.ctx().clone())?;
                }
            } else {
                ui.ctx().request_repaint_after(remaining);
            }
        }
        let mut changed = self.poll().map_err(|error| {
            crate::i18n::translation(language, &error)
                .unwrap_or(&error)
                .to_owned()
        })?;
        if self.choosing {
            ui.ctx().request_repaint();
            return Ok(changed);
        }
        let mut refresh = false;
        ui.horizontal(|ui| {
            if self.pending.is_some() {
                ui.spinner();
            }
            if let Some(resource) = self.selected_resource() {
                let width = (ui.available_width() - 80.0).max(1.0);
                ui.allocate_ui(
                    egui::vec2(width.min(480.0), ui.spacing().interact_size.y),
                    |ui| {
                        ui.add(egui::Label::new(&resource.title).truncate());
                    },
                );
            }
            if self.resources.len() > 1
                && components::icon_button(
                    ui,
                    tr(language, "Choose a resource"),
                    egui::include_image!("../../resources/icons/view-all.svg"),
                )
                .clicked()
            {
                self.choosing = true;
                ui.ctx().request_repaint();
            }
            if self.pending.is_none() && self.resolve_after.is_none() && !self.url.trim().is_empty()
            {
                refresh = components::refresh_button(ui, tr(language, "Refresh")).clicked();
            }
        });
        if refresh {
            self.load(proxy.to_owned(), ui.ctx().clone())?;
            changed = true;
        }
        Ok(changed)
    }

    /// Shared full-page picker for every discovery source, including feeds and HTML players.
    pub fn render_picker(
        &mut self,
        ui: &mut egui::Ui,
        language: UiLanguage,
        expected: MediaKind,
    ) -> bool {
        components::page_header(ui, tr(language, "Choose a resource"), |ui| {
            if components::back_button(ui, tr(language, "Back")).clicked() {
                self.choosing = false;
                ui.ctx().request_repaint();
            }
        });
        components::search_bar(ui, &mut self.query, tr(language, "Search resources"));
        let query = self.query.trim().to_lowercase();
        let visible: Vec<_> = self
            .resources
            .iter()
            .enumerate()
            .filter(|(_, resource)| {
                query.is_empty() || resource.title.to_lowercase().contains(&query)
            })
            .map(|(index, _)| index)
            .collect();
        let mut picked = None;
        crate::ui::layout::selection_grid(
            ui,
            "media_resource_grid",
            visible.len(),
            208.0,
            220.0,
            |ui, index, width| {
                let index = visible[index];
                if resource_card(
                    ui,
                    &self.resources[index],
                    index,
                    width,
                    self.selected == Some(index),
                    expected,
                    language,
                ) {
                    picked = Some(index);
                }
            },
        );
        if let Some(index) = picked {
            self.selected = Some(index);
            self.choosing = false;
            self.query.clear();
            ui.ctx().request_repaint();
            return true;
        }
        false
    }

    pub fn selected_resource(&self) -> Option<&MediaResource> {
        if self.choosing || self.input_snapshot != self.url.trim() {
            return None;
        }
        self.selected.and_then(|index| self.resources.get(index))
    }

    pub fn invalidate(&mut self) {
        self.pending = None;
        self.resources.clear();
        self.input_snapshot.clear();
        self.resolve_after = None;
        self.selected = None;
        self.choosing = false;
        self.query.clear();
    }

    fn load(&mut self, proxy: String, ctx: egui::Context) -> Result<(), String> {
        self.invalidate();
        let url = self.url.trim().to_owned();
        self.input_snapshot = url.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let (tx, result) = crossbeam_channel::bounded(1);
        std::thread::Builder::new()
            .name("media-discovery".into())
            .spawn(move || {
                let _ = tx.send(discovery::resolve(&url, &proxy, worker_stop));
                ctx.request_repaint();
            })
            .map_err(|e| e.to_string())?;
        self.pending = Some(Lookup { stop, result });
        Ok(())
    }

    fn poll(&mut self) -> Result<bool, String> {
        let Some(pending) = &self.pending else {
            return Ok(false);
        };
        let result = match pending.result.try_recv() {
            Ok(result) => result,
            Err(crossbeam_channel::TryRecvError::Empty) => return Ok(false),
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                Err("Media lookup stopped unexpectedly".into())
            }
        };
        self.pending = None;
        self.resources = result?;
        self.selected = (self.resources.len() == 1).then_some(0);
        self.choosing = self.resources.len() > 1;
        Ok(true)
    }
}

fn resource_card(
    ui: &mut egui::Ui,
    resource: &MediaResource,
    index: usize,
    width: f32,
    selected: bool,
    expected: MediaKind,
    language: UiLanguage,
) -> bool {
    let enabled = resource.kind == expected;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 208.0), egui::Sense::click());
    let id = ui.make_persistent_id(("media_resource", index));
    if !ui.is_rect_visible(rect) {
        return false;
    }
    if enabled && (response.hovered() || selected) {
        ui.painter()
            .rect_filled(rect, 14.0, theme::surface_control_hover());
    }
    let preview = egui::Rect::from_min_size(
        rect.min + egui::vec2(6.0, 6.0),
        egui::vec2(width - 12.0, (width * 9.0 / 16.0).min(164.0)),
    );
    ui.painter()
        .rect_filled(preview, 12.0, theme::surface_subtle());
    let icon = if resource.video {
        egui::include_image!("../../resources/icons/file-video.svg")
    } else {
        egui::include_image!("../../resources/icons/file-audio.svg")
    };
    egui::Image::new(icon).tint(theme::text_weak()).paint_at(
        ui,
        egui::Rect::from_center_size(preview.center(), egui::vec2(38.0, 38.0)),
    );
    if let Some(url) = &resource.thumbnail {
        let image = egui::Image::from_uri(url).corner_radius(12.0);
        if let Ok(egui::load::TexturePoll::Ready { texture }) =
            image.load_for_size(ui.ctx(), preview.size())
        {
            let aspect = texture.size.x / texture.size.y;
            let target = preview.width() / preview.height();
            let crop = egui::vec2((target / aspect).min(1.0), (aspect / target).min(1.0));
            egui::Image::from_texture(texture)
                .uv(egui::Rect::from_center_size(egui::pos2(0.5, 0.5), crop))
                .corner_radius(12.0)
                .tint(if enabled {
                    egui::Color32::WHITE
                } else {
                    egui::Color32::from_white_alpha(100)
                })
                .paint_at(ui, preview);
        }
    }
    let color = if enabled {
        theme::text_strong()
    } else {
        theme::text_weak()
    };
    let mut title = egui::text::LayoutJob::simple(
        resource.title.clone(),
        egui::FontId::proportional(14.0),
        color,
        preview.width(),
    );
    title.wrap.max_rows = 2;
    ui.painter().galley(
        preview.left_bottom() + egui::vec2(0.0, 9.0),
        ui.painter().layout_job(title),
        color,
    );
    if resource.kind != MediaKind::Recorded {
        let label = tr(
            language,
            match resource.kind {
                MediaKind::Live => "Live",
                MediaKind::Pending => "Upcoming",
                MediaKind::Ended => "Ended",
                _ => "Unknown",
            },
        );
        ui.painter().text(
            preview.left_bottom() - egui::vec2(-8.0, 8.0),
            egui::Align2::LEFT_BOTTOM,
            label,
            egui::FontId::proportional(12.0),
            theme::primary(),
        );
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Button, enabled, selected, &resource.title)
    });
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect,
            14.0,
            egui::Stroke::new(1.0, theme::primary()),
            egui::StrokeKind::Inside,
        );
    }
    let automated = ui_director::egui::record_button(ui, id, &resource.title, enabled, rect);
    let clicked = enabled && (response.clicked() || automated);
    if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand);
    }
    clicked
}
