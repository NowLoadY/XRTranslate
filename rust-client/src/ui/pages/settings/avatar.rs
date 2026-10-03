//! Appearance editor. Geometry, accessory choices and fitting stay in the avatar module.
use crate::{
    i18n::{UiLanguage, tr},
    ui::{
        animation::AnimationSystem,
        automation,
        components::{
            self,
            avatar::{Accessory, Appearance, Expression, Gaze, Pose, wardrobe::Socket},
        },
        theme,
    },
};
use eframe::egui::{self, Align2, Color32, FontId, Id, Rect, Sense, Stroke, Vec2};
use std::f32::consts::{FRAC_PI_2, PI, TAU};

#[derive(Clone)]
struct Orbit {
    angles: Vec2,
    velocity: Vec2,
    last_time: f64,
}

impl Default for Orbit {
    fn default() -> Self {
        Self {
            angles: egui::vec2(-0.25, 0.05),
            velocity: Vec2::ZERO,
            last_time: 0.0,
        }
    }
}

impl Orbit {
    fn update(&mut self, response: &egui::Response) {
        let now = response.ctx.input(|input| input.time);
        let elapsed = (now - self.last_time).max(0.0) as f32;
        self.last_time = now;
        if elapsed > 0.25 {
            self.velocity = Vec2::ZERO;
        }
        let dt = elapsed.min(0.05);
        if response.dragged() {
            let turn = response.drag_delta() * (TAU / response.rect.width().max(160.0));
            self.angles += turn;
            self.velocity = turn / dt.max(0.001);
        } else {
            self.velocity *= (-dt / 0.12).exp();
            self.angles += self.velocity * dt;
        }
        self.angles.x = (self.angles.x + PI).rem_euclid(TAU) - PI;
        let pitch = self.angles.y.clamp(-FRAC_PI_2, FRAC_PI_2);
        if pitch != self.angles.y {
            self.velocity.y = 0.0;
        }
        self.angles.y = pitch;
        if self.velocity.length_sq() > 0.001 {
            response.ctx.request_repaint();
        } else {
            self.velocity = Vec2::ZERO;
        }
    }
}

pub(super) fn render(ui: &mut egui::Ui, appearance: &mut Appearance, language: UiLanguage) -> bool {
    let id = ui.make_persistent_id("avatar_appearance");
    let width = ui.available_width();
    let height = (width * 0.85).clamp(280.0, 520.0);
    let (rect, drag) = ui.allocate_exact_size(egui::vec2(width, height), Sense::click_and_drag());
    let cursor = if drag.dragged() {
        egui::CursorIcon::Grabbing
    } else {
        egui::CursorIcon::Grab
    };
    let drag = drag.on_hover_cursor(cursor);
    let mut orbit = ui
        .ctx()
        .data(|data| data.get_temp::<Orbit>(id))
        .unwrap_or_default();
    orbit.update(&drag);
    ui.painter().rect_filled(
        rect,
        22.0,
        theme::panel_fill(ui.ctx(), Color32::from_white_alpha(130)),
    );

    ui.scope_builder(
        egui::UiBuilder::new().max_rect(Rect::from_min_size(
            rect.right_top() + egui::vec2(-42.0, 12.0),
            Vec2::splat(28.0),
        )),
        |ui| {
            if components::reset_button(ui, "avatar_rotation")
                .on_hover_text(tr(language, "Reset view"))
                .clicked()
            {
                orbit = Orbit {
                    last_time: ui.input(|input| input.time),
                    ..Default::default()
                };
            }
        },
    );
    let avatar = appearance.model();
    let radius = rect.width().min(rect.height()) * 0.28;
    let center = rect.center() + egui::vec2(0.0, 8.0);
    let mut pose = Pose::interactive(
        ui.ctx(),
        id.with("face"),
        Expression::Calm,
        Gaze::default(),
        Some(&drag),
    );
    pose.gaze = Gaze {
        yaw: orbit.angles.x,
        pitch: orbit.angles.y,
    };
    avatar.paint(
        &ui.painter().with_clip_rect(rect.intersect(ui.clip_rect())),
        id.with("model"),
        center,
        radius,
        pose,
    );
    ui.painter().text(
        rect.center_bottom() - egui::vec2(0.0, 20.0),
        Align2::CENTER_CENTER,
        tr(language, "Drag to rotate"),
        FontId::proportional(12.0),
        theme::text_weak(),
    );

    let mut changed = false;
    for socket in Socket::AVAILABLE {
        let point = avatar.socket_position(socket, pose);
        // The existing orthographic renderer maps one model unit to `radius` points.
        let marker = rect
            .shrink(22.0)
            .clamp(center + egui::vec2(point.x, -point.y) * radius);
        let label = tr(
            language,
            match socket {
                Socket::Hat => "Hat",
                _ => "Scarf",
            },
        );
        let response = ui
            .put(
                Rect::from_center_size(marker, Vec2::splat(32.0)),
                egui::Button::new(egui::RichText::new("+").size(23.0))
                    .fill(Color32::from_white_alpha(235))
                    .stroke(Stroke::new(
                        1.0,
                        if appearance.selected(socket).is_some() {
                            theme::primary()
                        } else {
                            theme::border()
                        },
                    ))
                    .corner_radius(16.0),
            )
            .on_hover_text(label);
        let response = automation::button_response(ui, response.id, label, true, response);
        egui::Popup::from_toggle_button_response(&response)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .width((rect.width() - 24.0).clamp(140.0, 420.0))
            .show(|ui| {
                ui.label(egui::RichText::new(label).strong());
                ui.add_space(6.0);
                egui::ScrollArea::horizontal()
                    .id_salt((id, socket))
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            for item in std::iter::once(None).chain(
                                Accessory::ALL
                                    .iter()
                                    .filter(|item| item.socket == socket)
                                    .map(Some),
                            ) {
                                changed |= accessory_card(
                                    ui,
                                    id.with(socket),
                                    appearance,
                                    socket,
                                    item,
                                    language,
                                );
                            }
                        });
                    });
            });
    }
    ui.ctx().data_mut(|data| data.insert_temp(id, orbit));
    if changed {
        ui.ctx().request_repaint();
    }
    changed
}

fn accessory_card(
    ui: &mut egui::Ui,
    id: Id,
    appearance: &mut Appearance,
    socket: Socket,
    item: Option<&Accessory>,
    language: UiLanguage,
) -> bool {
    let id = id.with(item.map_or("none", |item| item.id));
    let (rect, response) = ui.allocate_exact_size(egui::vec2(108.0, 118.0), Sense::click());
    let selected = appearance.selected(socket).map(|item| item.id) == item.map(|item| item.id);
    let name = tr(language, item.map_or("No accessory", |item| item.name));
    let label = format!(
        "{}: {name}",
        tr(
            language,
            if socket == Socket::Hat {
                "Hat"
            } else {
                "Scarf"
            }
        )
    );
    let response = automation::button_response(ui, id, &label, true, response)
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, &label)
    });
    let hover = AnimationSystem::hover(ui.ctx(), id.with("hover"), response.hovered());
    let chosen = AnimationSystem::selection(ui.ctx(), id.with("chosen"), selected);
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.rect_filled(
        rect,
        16.0,
        Color32::from_gray(248).lerp_to_gamma(Color32::from_gray(240), hover),
    );
    painter.rect_stroke(
        rect.shrink(0.5),
        16.0,
        Stroke::new(1.0, theme::border().lerp_to_gamma(theme::primary(), chosen)),
        egui::StrokeKind::Inside,
    );
    let center = rect.center_top() + egui::vec2(0.0, 47.0);
    if let Some(item) = item {
        if ui.is_rect_visible(rect) {
            item.paint(&painter, id, center, 35.0, -0.45 + hover * 0.3);
        }
    } else {
        painter.circle_stroke(center, 16.0, Stroke::new(1.5, theme::text_weak()));
        painter.line_segment(
            [center - Vec2::splat(11.0), center + Vec2::splat(11.0)],
            Stroke::new(1.5, theme::text_weak()),
        );
    }
    painter.text(
        rect.center_bottom() - egui::vec2(0.0, 17.0),
        Align2::CENTER_CENTER,
        name,
        FontId::proportional(13.0),
        theme::text_normal(),
    );
    if response.clicked() && !selected {
        appearance.select(socket, item);
        true
    } else {
        false
    }
}
