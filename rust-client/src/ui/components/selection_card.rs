use crate::ui::{
    animation::AnimationSystem,
    components::avatar::{self, Expression, Gaze, Pose},
    theme,
};
use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, Vec2};

pub fn grid(
    ui: &mut egui::Ui,
    id: &str,
    count: usize,
    height: f32,
    mut card: impl FnMut(&mut egui::Ui, usize, f32),
) {
    let width = (ui.available_width() - 32.0).max(1.0);
    let gap = 24.0;
    let columns = (((width + gap) / 280.0).floor() as usize).clamp(1, 4);
    let card_width = ((width - gap * (columns - 1) as f32) / columns as f32).min(320.0);
    let row_width =
        card_width * columns.min(count) as f32 + gap * columns.min(count).saturating_sub(1) as f32;
    let left = ((ui.available_width() - row_width) * 0.5).max(0.0);
    let inner_spacing = ui.spacing().item_spacing.y;
    ui.add_space(24.0);
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = gap;
        egui::ScrollArea::vertical().id_salt(id).show_rows(
            ui,
            height,
            count.div_ceil(columns),
            |ui, rows| {
                for row in rows {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = gap;
                        ui.spacing_mut().item_spacing.y = inner_spacing;
                        ui.add_space(left);
                        for index in row * columns..((row + 1) * columns).min(count) {
                            card(ui, index, card_width);
                        }
                    });
                }
            },
        );
    });
}

pub struct SelectionCard<'a> {
    pub id: egui::Id,
    pub title: &'a str,
    pub avatar: &'a str,
    pub width: f32,
    pub height: f32,
    pub selected: bool,
    pub enabled: bool,
    pub action: &'a str,
}

pub fn show(
    ui: &mut egui::Ui,
    card: SelectionCard<'_>,
    add_contents: impl FnOnce(&mut egui::Ui),
) -> bool {
    let SelectionCard {
        id,
        title: name,
        avatar: character,
        width,
        height,
        selected,
        enabled,
        action,
    } = card;
    let (space, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    if !ui.is_rect_visible(space) {
        return false;
    }
    let hovered = ui.rect_contains_pointer(space);
    let hover = AnimationSystem::hover(ui.ctx(), id.with("hover"), hovered);
    let chosen = AnimationSystem::selection(ui.ctx(), id.with("selected"), selected);
    let rect = space
        .shrink2(Vec2::new(0.0, 8.0))
        .translate(Vec2::new(0.0, -4.0 * hover));
    ui.painter().add(
        egui::Frame::new()
            .fill(Color32::from_rgba_unmultiplied(242, 242, 242, 225))
            .stroke(Stroke::new(
                1.0,
                Color32::from_gray(207).lerp_to_gamma(Color32::from_gray(129), chosen),
            ))
            .corner_radius(CornerRadius::same(22))
            .shadow(egui::Shadow {
                offset: [0, 7 + (hover * 2.0) as i8],
                blur: 24,
                spread: 0,
                color: Color32::from_black_alpha(18 + (hover * 4.0) as u8),
            })
            .paint(rect),
    );

    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    let center = Pos2::new(rect.left() + 43.0, rect.top() + 39.0);
    if avatar::visible(ui, center, 20.0) {
        let character = avatar::companion(character);
        let gaze = ui
            .input(|input| input.pointer.hover_pos())
            .filter(|_| hovered)
            .map_or(Gaze::default(), |pointer| {
                Gaze::toward(pointer - center, 260.0)
            });
        let expression = if selected {
            Expression::Happy
        } else if hovered {
            Expression::Curious
        } else {
            Expression::Calm
        };
        character.paint(
            &painter,
            id.with("model"),
            center,
            20.0,
            Pose::animated(ui.ctx(), id.with("avatar"), expression, gaze),
        );
    }
    if selected {
        let badge = Pos2::new(rect.right() - 28.0, center.y);
        painter.circle_filled(badge, 10.0, Color32::from_gray(103));
        painter.text(
            badge,
            Align2::CENTER_CENTER,
            "✓",
            FontId::proportional(12.0),
            Color32::WHITE,
        );
    }
    let mut title = egui::text::LayoutJob::simple(
        name.to_string(),
        FontId::proportional(19.0),
        theme::text_strong(),
        (width - 126.0).max(1.0),
    );
    title.wrap.max_rows = 1;
    let title = painter.layout_job(title);
    painter.galley(
        Pos2::new(rect.left() + 78.0, center.y - title.size().y * 0.5),
        title,
        theme::text_strong(),
    );

    let text_rect = Rect::from_min_max(
        Pos2::new(rect.left() + 22.0, rect.top() + 82.0),
        Pos2::new(rect.right() - 22.0, rect.bottom() - 57.0),
    );
    let mut body = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(id.with("text"))
            .max_rect(text_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    body.set_clip_rect(text_rect.intersect(ui.clip_rect()));
    add_contents(&mut body);

    // Keep the reading area independent from selection, including touch/drag scrolling.
    let footer = Rect::from_min_max(
        Pos2::new(rect.left() + 12.0, rect.bottom() - 46.0),
        rect.right_bottom() - Vec2::new(12.0, 8.0),
    );
    let response = ui.interact(
        footer,
        id,
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, enabled, selected, &name)
    });
    painter.text(
        Pos2::new(rect.left() + 22.0, footer.center().y),
        Align2::LEFT_CENTER,
        action,
        FontId::proportional(12.0),
        Color32::from_gray(84),
    );
    if !selected {
        painter.text(
            Pos2::new(rect.right() - 26.0 + hover * 2.0, footer.center().y),
            Align2::CENTER_CENTER,
            "→",
            FontId::proportional(17.0),
            Color32::from_gray(103),
        );
    }
    if response.has_focus() {
        painter.rect_stroke(
            footer,
            10.0,
            Stroke::new(1.0, Color32::from_gray(129)),
            egui::StrokeKind::Inside,
        );
    }
    let automated = crate::ui::automation::record_button(ui, id, name, enabled, footer);
    let clicked = enabled && (response.clicked() || automated);
    if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand);
    }
    clicked
}
