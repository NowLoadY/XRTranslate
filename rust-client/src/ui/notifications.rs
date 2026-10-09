//! Shared, non-blocking notices for recoverable failures and brief feedback.

use eframe::egui::{self, Color32, Id, Stroke, Vec2};
use parking_lot::Mutex;
use std::{collections::VecDeque, sync::Arc, time::Duration};

const MAX_PENDING: usize = 4;
const FADE_SECONDS: f64 = 0.18;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Notice {
    message: String,
    details: String,
}

struct ActiveNotice {
    notice: Notice,
    started: f64,
    expires: f64,
}

#[derive(Default)]
struct State {
    pending: VecDeque<Notice>,
    active: Option<ActiveNotice>,
}

/// The host and plugin UIs share one bounded queue and one presentation surface.
#[derive(Clone, Default)]
pub struct Notifications(Arc<Mutex<State>>);

impl Notifications {
    pub fn error(&self, message: impl Into<String>, details: impl Into<String>) {
        let notice = Notice {
            message: message.into(),
            details: details.into(),
        };
        if notice.message.trim().is_empty() {
            return;
        }
        let mut state = self.0.lock();
        if state
            .active
            .as_ref()
            .is_some_and(|active| active.notice == notice)
            || state.pending.contains(&notice)
        {
            return;
        }
        log::warn!("{} {}", notice.message, notice.details);
        if state.pending.len() == MAX_PENDING {
            state.pending.pop_front();
        }
        state.pending.push_back(notice);
    }

    /// Install before rendering pages so plugins can post without host access.
    pub fn install(&self, ctx: &egui::Context) {
        ctx.data_mut(|data| data.insert_temp(Id::new("notifications"), self.clone()));
    }

    pub fn show(&self, ctx: &egui::Context, language: crate::i18n::UiLanguage) {
        let now = ctx.input(|input| input.time);
        let (notice, opacity, repaint_after) = {
            let mut state = self.0.lock();
            state.advance(now);
            let Some(active) = &state.active else { return };
            let fade = ((now - active.started) / FADE_SECONDS)
                .min((active.expires - now) / FADE_SECONDS)
                .clamp(0.0, 1.0) as f32;
            let repaint_after = if fade < 1.0 {
                Duration::from_millis(16)
            } else {
                Duration::from_secs_f64((active.expires - now - FADE_SECONDS).max(0.016))
            };
            (
                active.notice.clone(),
                super::animation::AnimationSystem::ease_out_cubic(fade),
                repaint_after,
            )
        };
        let available = ctx.content_rect();
        let max_width = (available.width() - 48.0).clamp(48.0, 720.0);
        let response = egui::Area::new(Id::new("notification_pill"))
            .order(egui::Order::Tooltip)
            .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -24.0])
            .movable(false)
            .show(ctx, |ui| {
                ui.set_opacity(opacity);
                let mut job = egui::text::LayoutJob::simple(
                    notice.message.clone(),
                    egui::FontId::proportional(14.0),
                    Color32::BLACK,
                    (max_width - 36.0).max(12.0),
                );
                job.wrap.max_rows = 2;
                job.wrap.break_anywhere = true;
                let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
                let size = galley.size() + Vec2::new(36.0, 20.0);
                let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
                ui.painter().add(
                    egui::Frame::new()
                        .fill(Color32::WHITE)
                        .stroke(Stroke::new(1.0, Color32::BLACK))
                        .corner_radius((size.y / 2.0).min(28.0))
                        .shadow(egui::Shadow {
                            offset: [0, 4],
                            blur: 16,
                            spread: 0,
                            color: Color32::from_black_alpha(40),
                        })
                        .paint(rect),
                );
                ui.painter()
                    .galley(rect.min + Vec2::new(18.0, 10.0), galley, Color32::BLACK);
                super::automation::record_output(
                    ui,
                    "Notification",
                    super::automation::ElementValue::Text(notice.message.clone()),
                    rect,
                );
                super::automation::button_response(
                    ui,
                    response.id,
                    crate::i18n::tr(language, "Dismiss notification"),
                    true,
                    response,
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(if notice.details.is_empty() {
                    &notice.message
                } else {
                    &notice.details
                })
            })
            .inner;
        if response.clicked() {
            self.0.lock().active = None;
            ctx.request_repaint();
        }
        ctx.request_repaint_after(repaint_after);
    }
}

impl State {
    fn advance(&mut self, now: f64) {
        if self
            .active
            .as_ref()
            .is_some_and(|active| now >= active.expires)
        {
            self.active = None;
        }
        if self.active.is_none()
            && let Some(notice) = self.pending.pop_front()
        {
            let duration = (notice.message.chars().count() as f64 / 14.0).clamp(4.0, 8.0);
            self.active = Some(ActiveNotice {
                notice,
                started: now,
                expires: now + duration,
            });
        }
    }
}

/// Observe a persistent error once, including across page changes. Call with
/// None after recovery so the same failure on a later attempt can notify again.
pub fn observe_error(ctx: &egui::Context, id: Id, message: &str, error: Option<&str>) {
    let changed = ctx.data_mut(|data| {
        let key = id.with("notification_error");
        let previous = data.get_temp::<String>(key);
        if previous.as_deref() == error {
            return false;
        }
        if let Some(error) = error {
            data.insert_temp(key, error.to_owned());
        } else {
            data.remove::<String>(key);
        }
        true
    });
    if changed && let Some(error) = error {
        let notifications = ctx.data_mut(|data| {
            data.get_temp_mut_or_default::<Notifications>(Id::new("notifications"))
                .clone()
        });
        notifications.error(message, error);
        ctx.request_repaint();
    }
}
