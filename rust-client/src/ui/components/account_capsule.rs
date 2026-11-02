//! Account identity is the control; secondary actions emerge from the stack.
use crate::{
    account_connections::{AccountAction, AccountConnections},
    i18n::{self, UiLanguage},
    ui::{animation::AnimationSystem, automation, theme},
};
use eframe::egui::{self, Color32, Rect, Sense, Stroke, Vec2};
use xrtranslate_connections::{AccountProvider, AccountSnapshot, AccountStatus};

const PLUS: egui::ImageSource<'static> = egui::include_image!("../../../resources/icons/plus.svg");
const SWITCH: egui::ImageSource<'static> =
    egui::include_image!("../../../resources/icons/account-switch.svg");
const CLOSE: egui::ImageSource<'static> = egui::include_image!("../../../resources/icons/x.svg");

pub(crate) fn account_capsule(
    ui: &mut egui::Ui,
    provider: AccountProvider,
    connections: &AccountConnections,
    language: UiLanguage,
) -> Option<AccountAction> {
    let empty = AccountSnapshot::default();
    let snapshot = connections.snapshot(provider).unwrap_or(&empty);
    let connected = snapshot.status == AccountStatus::Connected;
    let busy = connections.is_busy_for(provider);
    let enabled = !connections.is_busy();
    let label = snapshot
        .label
        .as_deref()
        .unwrap_or(i18n::tr(language, provider.label()));
    let profiles = &snapshot.profiles;
    let has_rear = connected || !profiles.is_empty();
    let reveal = 42.0;
    let reserved = if has_rear { reveal } else { 0.0 };
    let id = ui.make_persistent_id((provider.id(), "account_capsule"));
    let colors = theme::account_capsule_colors();
    let measured = ui
        .painter()
        .layout_no_wrap(
            label.to_owned(),
            egui::FontId::proportional(13.0),
            colors.identity,
        )
        .size()
        .x;
    let width = (if connected {
        (measured + 44.0).clamp(164.0, 280.0)
    } else {
        128.0
    })
    .min((ui.available_width() - reserved - 8.0).max(72.0));
    let (_, area) = ui.allocate_space(egui::vec2(width + reserved + 8.0, 60.0));
    let front = Rect::from_min_size(area.min + egui::vec2(0.0, 12.0), egui::vec2(width, 38.0));
    let mut managing =
        ui.data_mut(|data| data.get_temp::<bool>(id.with("managing")).unwrap_or(false));
    if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
        managing = false;
    }
    let near = ui.rect_contains_pointer(area);
    let inside_manager = ui
        .data(|data| data.get_temp::<Rect>(id.with("manager_bounds")))
        .is_some_and(|bounds| ui.rect_contains_pointer(bounds));
    if !near && !inside_manager && ui.input(|input| input.pointer.any_pressed()) {
        managing = false;
    }
    let focused = ui.data(|data| data.get_temp::<bool>(id.with("focused")).unwrap_or(false));
    let expand = AnimationSystem::hover(ui.ctx(), id.with("reveal"), near || focused || managing);
    let offset = 12.0 + (reveal - 12.0) * expand;
    let rear = front.translate(egui::vec2(offset, 0.0));
    let mut action = None;
    let mut focus = false;

    if has_rear {
        ui.painter().rect(
            rear,
            19.0,
            colors.rear,
            Stroke::NONE,
            egui::StrokeKind::Inside,
        );
        // The covered part never receives a click intended for the front account.
        let exposed = Rect::from_min_max(egui::pos2(front.right() + 2.0, rear.top()), rear.max);
        let response = hit(
            ui,
            id.with("manage"),
            exposed,
            i18n::tr(language, "Switch account"),
            enabled,
        );
        focus |= response.has_focus();
        if expand > 0.35 {
            icon(
                ui,
                SWITCH,
                exposed.center(),
                theme::text_normal().gamma_multiply(expand),
            );
        }
        if response.clicked() {
            managing = !managing;
        }
    }

    let hint = if busy {
        i18n::tr(language, connections.working_label()).to_owned()
    } else if connected {
        label.to_owned()
    } else {
        let status = match snapshot.status {
            AccountStatus::Expired => "Session expired. Sign in again.",
            AccountStatus::Unavailable => "Cannot read account connection. Sign in again.",
            _ => provider.sign_in_label(),
        };
        let mut hint = i18n::tr(language, status).to_owned();
        if let Some(identity) = &snapshot.label {
            hint.push('\n');
            hint.push_str(identity);
        }
        if let Some(requirement) = provider.requirement() {
            hint.push('\n');
            hint.push_str(i18n::tr(language, requirement));
        }
        hint
    };
    let response = hit(ui, id.with("front"), front, &hint, enabled);
    focus |= response.has_focus();
    let hover = AnimationSystem::hover(
        ui.ctx(),
        id.with("border"),
        response.hovered() || response.has_focus(),
    );
    ui.painter().rect(
        front,
        19.0,
        if connected { colors.fill } else { colors.empty },
        if connected {
            Stroke::NONE
        } else {
            Stroke::new(1.1 + hover, colors.idle_border)
        },
        egui::StrokeKind::Inside,
    );
    if busy {
        egui::Spinner::new().color(theme::text_normal()).paint_at(
            ui,
            Rect::from_center_size(front.center(), Vec2::splat(18.0)),
        );
    } else if connected {
        let galley =
            egui::WidgetText::from(egui::RichText::new(label).size(13.0).color(colors.identity))
                .into_galley(
                    ui,
                    Some(egui::TextWrapMode::Truncate),
                    width - 32.0,
                    egui::TextStyle::Body,
                );
        ui.painter().galley(
            front.center() - galley.size() * 0.5,
            galley,
            colors.identity,
        );
    } else {
        icon(ui, PLUS, front.center(), theme::text_normal());
    }
    if response.clicked() {
        if connected {
            managing = !managing;
        } else {
            action = Some(AccountAction::Connect(
                provider,
                snapshot.active_profile.clone(),
            ));
        }
    }

    if ((connected && !busy) || (busy && connections.can_cancel())) && (expand > 0.4 || busy) {
        let center = egui::pos2(front.right() - 3.0, front.top() + 2.0);
        let rect = Rect::from_center_size(center, Vec2::splat(26.0));
        let hint = i18n::tr(language, if busy { "Cancel" } else { "Sign out" });
        let response = hit(
            ui,
            id.with("close"),
            rect,
            hint,
            if busy {
                connections.can_cancel()
            } else {
                enabled
            },
        );
        focus |= response.has_focus();
        ui.painter().circle(
            center,
            10.0,
            if response.hovered() {
                colors.rear
            } else {
                colors.fill
            },
            Stroke::NONE,
        );
        egui::Image::new(CLOSE)
            .tint(theme::text_normal())
            .paint_at(ui, Rect::from_center_size(center, Vec2::splat(12.0)));
        if response.clicked() {
            action = Some(if busy {
                AccountAction::Cancel
            } else {
                AccountAction::SignOut(
                    provider,
                    snapshot.active_profile.clone().unwrap_or_default(),
                )
            });
            managing = false;
        }
    }
    if managing {
        let selected = snapshot.active_profile.as_deref();
        let max_width = ui.available_width().min(260.0);
        let response = ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 10.0);
            for profile in profiles {
                let selected = selected == Some(profile.id.as_str());
                let measured = ui
                    .painter()
                    .layout_no_wrap(
                        profile.label.clone(),
                        egui::FontId::proportional(12.0),
                        colors.identity,
                    )
                    .size()
                    .x;
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2((measured + 48.0).clamp(120.0, max_width.max(120.0)), 34.0),
                    Sense::hover(),
                );
                let hint = if profile.connected {
                    profile.label.clone()
                } else {
                    format!("{} · {}", i18n::tr(language, "Sign in"), profile.label)
                };
                let response = hit(ui, id.with((&profile.id, "select")), rect, &hint, enabled);
                ui.painter().rect_filled(
                    rect,
                    17.0,
                    if selected { colors.fill } else { colors.rear },
                );
                let color = if profile.connected {
                    colors.identity
                } else {
                    theme::text_weak()
                };
                let galley = egui::WidgetText::from(
                    egui::RichText::new(&profile.label).size(12.0).color(color),
                )
                .into_galley(
                    ui,
                    Some(egui::TextWrapMode::Truncate),
                    rect.width() - 48.0,
                    egui::TextStyle::Body,
                );
                ui.painter().galley(
                    rect.center() - galley.size() * 0.5 + egui::vec2(5.0, 0.0),
                    galley,
                    color,
                );
                if selected {
                    icon(
                        ui,
                        egui::include_image!("../../../resources/icons/check.svg"),
                        egui::pos2(rect.left() + 15.0, rect.center().y),
                        color,
                    );
                }
                if response.clicked() {
                    if !selected || !profile.connected {
                        action = Some(AccountAction::SelectProfile(provider, profile.id.clone()));
                    }
                    managing = false;
                }
                let close_rect = Rect::from_center_size(
                    egui::pos2(rect.right() - 2.0, rect.top() + 2.0),
                    Vec2::splat(24.0),
                );
                let close_id = id.with((&profile.id, "sign_out"));
                if profile.connected
                    && (ui.rect_contains_pointer(rect.union(close_rect))
                        || response.has_focus()
                        || ui.memory(|memory| memory.has_focus(close_id)))
                {
                    let hint = format!("{} · {}", i18n::tr(language, "Sign out"), profile.label);
                    let close = hit(ui, close_id, close_rect, &hint, enabled);
                    ui.painter()
                        .circle_filled(close_rect.center(), 9.0, colors.fill);
                    egui::Image::new(CLOSE).tint(theme::text_normal()).paint_at(
                        ui,
                        Rect::from_center_size(close_rect.center(), Vec2::splat(12.0)),
                    );
                    if close.clicked() {
                        action = Some(AccountAction::SignOut(provider, profile.id.clone()));
                    }
                }
            }
            let (rect, _) = ui.allocate_exact_size(egui::vec2(48.0, 34.0), Sense::hover());
            let response = hit(
                ui,
                id.with("add"),
                rect,
                i18n::tr(language, "Sign in to another account"),
                enabled,
            );
            ui.painter().rect_filled(rect, 17.0, colors.rear);
            icon(ui, PLUS, rect.center(), theme::text_normal());
            if response.clicked() {
                action = Some(AccountAction::Connect(provider, None));
                managing = false;
            }
        });
        ui.data_mut(|data| {
            data.insert_temp(
                id.with("manager_bounds"),
                area.union(response.response.rect),
            )
        });
        ui.add_space(8.0);
    } else {
        ui.data_mut(|data| data.remove::<Rect>(id.with("manager_bounds")));
    }
    ui.data_mut(|data| {
        data.insert_temp(id.with("managing"), managing);
        data.insert_temp(id.with("focused"), focus);
    });
    action
}

fn hit(ui: &mut egui::Ui, id: egui::Id, rect: Rect, label: &str, enabled: bool) -> egui::Response {
    let response = ui.interact(
        rect,
        id,
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let response = automation::button_response(ui, id, label, enabled && ui.is_enabled(), response);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::Button, enabled && ui.is_enabled(), label)
    });
    response
        .on_hover_cursor(if enabled {
            egui::CursorIcon::PointingHand
        } else {
            egui::CursorIcon::Default
        })
        .on_hover_text(label)
}

fn icon(ui: &egui::Ui, source: egui::ImageSource<'static>, center: egui::Pos2, color: Color32) {
    egui::Image::new(source)
        .tint(color)
        .paint_at(ui, Rect::from_center_size(center, Vec2::splat(18.0)));
}
