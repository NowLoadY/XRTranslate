//! Shared, non-blocking notices for recoverable failures and brief feedback.

use eframe::egui::{self, Color32, Id, Stroke, Vec2};
use parking_lot::Mutex;
use std::{borrow::Cow, collections::VecDeque, sync::Arc, time::Duration};

const MAX_PENDING: usize = 4;
const FADE_SECONDS: f64 = 0.18;

/// Add a common diagnostic hint without replacing the service's own message.
pub(crate) fn error_text(error: &str, language: crate::i18n::UiLanguage) -> Cow<'_, str> {
    match error_hint(error) {
        Some(hint) => Cow::Owned(format!("{}\n{error}", crate::i18n::tr(language, hint))),
        None => crate::i18n::tr_dynamic(language, error),
    }
}

fn error_hint(error: &str) -> Option<&'static str> {
    let text = error.to_ascii_lowercase();
    let contains = |patterns: &[&str]| patterns.iter().any(|pattern| text.contains(pattern));
    // Explicit diagnostics take precedence: HTTP 429 can mean either exhausted
    // quota or a temporary rate limit. Keep this independent of provider names.
    if contains(&[
        "insufficient_quota",
        "insufficient balance",
        "credit balance",
        "quota exceeded",
        "quota_exceeded",
    ]) {
        return Some("API quota or balance is insufficient.");
    }
    if contains(&[
        "invalid_api_key",
        "invalid api key",
        "incorrect api key",
        "unauthenticated",
    ]) {
        return Some("API authentication failed. Check the API key.");
    }
    if contains(&[
        "context_length_exceeded",
        "exceed_context_size",
        "maximum context length",
        "exceeds the available context size",
    ]) {
        return Some("The request exceeds the model's context limit.");
    }
    if contains(&["out of memory", "bad allocation", "std::bad_alloc"]) {
        return Some("The model ran out of memory.");
    }
    if contains(&["timed out", "timeout:", "deadline_exceeded"]) {
        return Some("The request timed out. Try again later.");
    }
    if contains(&[
        "connection refused",
        "dns error",
        "connect:",
        "failed to lookup address",
    ]) {
        return Some("Could not connect to the service.");
    }
    if contains(&["rate_limit", "rate limit", "too many requests"]) {
        return Some("Too many requests. Try again later.");
    }
    let status = ["http ", "status: ", "ask failed ("]
        .into_iter()
        .find_map(|prefix| text.split_once(prefix)?.1.get(..3)?.parse::<u16>().ok());
    match status {
        Some(401) => Some("API authentication failed. Check the API key."),
        Some(403) => Some("Access denied. Check the service permissions."),
        Some(404) => Some("The model or API endpoint was not found."),
        Some(408 | 504) => Some("The request timed out. Try again later."),
        Some(429) => Some("Too many requests. Try again later."),
        Some(400 | 422) => Some("The service rejected the request parameters."),
        Some(500..=599) => Some("The service is unavailable. Try again later."),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Notice {
    message: String,
    details: String,
}

impl Notice {
    fn text(&self) -> &str {
        if self.details.trim().is_empty() {
            &self.message
        } else {
            &self.details
        }
    }
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
        if notice.text().trim().is_empty() {
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
        ctx.data_mut(|data| data.insert_temp(Id::unique("notifications"), self.clone()));
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
        let text = error_text(notice.text(), language);
        let available = ctx.content_rect();
        let max_width = (available.width() - 48.0).clamp(48.0, 720.0);
        let response = egui::Area::new(Id::unique("notification_pill"))
            .order(egui::Order::Tooltip)
            .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -24.0])
            .movable(false)
            .show(ctx, |ui| {
                ui.set_opacity(opacity);
                let mut job = egui::text::LayoutJob::simple(
                    text.to_string(),
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
                    super::automation::ElementValue::Text(text.to_string()),
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
                .on_hover_text(text.as_ref())
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
            let duration = (notice.text().chars().count() as f64 / 14.0).clamp(4.0, 8.0);
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
            data.get_temp_mut_or_default::<Notifications>(Id::unique("notifications"))
                .clone()
        });
        notifications.error(message, error);
        ctx.request_repaint();
    }
}
