//! Prepare only the view named by an operation: open a disclosure/tab and scroll
//! to its control through Director's normal, cancellable input queue.
use crate::i18n::{UiLanguage, translation};
use eframe::egui::{self, Rect};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use ui_director::egui::{
    ElementKind, ElementValue,
    driver::{Command, Request},
};
use xrtranslate_plugin::guide::Step;

#[derive(Clone)]
struct Pending {
    reply: crossbeam_channel::Receiver<ui_director::Response>,
    cancelled: Arc<AtomicBool>,
}

#[derive(Clone, Default)]
pub(super) struct Reveal {
    done: bool,
    started: Option<Instant>,
    retry_at: Option<Instant>,
    tried: u16,
    scrolls: u8,
    pending: Option<Pending>,
}

pub(super) fn matches(label: &str, keys: &[String], language: UiLanguage) -> bool {
    keys.iter()
        .any(|key| label.eq_ignore_ascii_case(key) || translation(language, key) == Some(label))
}

impl Reveal {
    pub fn finished(&self) -> bool {
        self.done
    }
    pub fn cancel(&mut self) {
        if let Some(pending) = self.pending.take() {
            pending.cancelled.store(true, Ordering::Release);
        }
        self.done = true;
    }

    pub fn update(&mut self, ctx: &egui::Context, step: &Step, route: &str, language: UiLanguage) {
        if step.targets.is_empty() {
            self.done = true;
        }
        if self.done {
            return;
        }
        let now = Instant::now();
        if now.duration_since(*self.started.get_or_insert(now)) > Duration::from_secs(3) {
            self.cancel();
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(60));
        if self.retry_at.is_some_and(|at| now < at) {
            return;
        }
        if let Some(pending) = &self.pending {
            match pending.reply.try_recv() {
                Err(crossbeam_channel::TryRecvError::Empty) => return,
                _ => {
                    self.pending = None;
                    self.retry_at = Some(now + Duration::from_millis(240));
                    return;
                }
            }
        }
        let driver = ui_director::egui::driver();
        let snapshot = driver.snapshot();
        if snapshot.location.page != route {
            return;
        }
        let mask = (1u16 << step.reveal.len()) - 1;
        let target = snapshot
            .elements
            .iter()
            .filter(|control| matches(&control.label, &step.targets, language))
            .min_by_key(|control| matches!(control.kind, ElementKind::Button | ElementKind::Tab));
        if let Some(target) = target.filter(|target| {
            !matches!(target.kind, ElementKind::Button | ElementKind::Tab)
                || self.tried & mask == mask
        }) {
            let center = egui::pos2(
                target.rect[0] + target.rect[2] * 0.5,
                target.rect[1] + target.rect[3] * 0.5,
            );
            if target
                .visible_rect()
                .map(|rect| rect.intersect(ctx.content_rect()))
                .is_some_and(|rect| rect.contains(center))
            {
                self.done = true;
                return;
            }
            if self.scrolls < 4 {
                let [x, y, width, height] = target.rect;
                let clip = target.clip_rect.map_or(ctx.content_rect(), |[x, y, w, h]| {
                    Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, h))
                });
                let delta = (clip.center().y - (y + height * 0.5)).clamp(-600.0, 600.0);
                if clip.width() > 2.0 && clip.height() > 2.0 && delta.abs() > 8.0 {
                    self.scrolls += 1;
                    self.send(
                        ctx,
                        Command::Scroll {
                            x: (x + width * 0.5).clamp(clip.left() + 1.0, clip.right() - 1.0),
                            y: clip.center().y,
                            delta,
                        },
                    );
                    return;
                }
            }
            self.done = true;
            return;
        }
        // Try the innermost available disclosure first, preserving already open views.
        for (index, key) in step.reveal.iter().enumerate().rev() {
            if self.tried & (1 << index) != 0 {
                continue;
            }
            let mut controls: Vec<_> = snapshot
                .elements
                .iter()
                .filter(|control| {
                    control.enabled
                        && matches(&control.label, std::slice::from_ref(key), language)
                        && control.actions().contains(&"click")
                })
                .collect();
            controls.sort_by(|a, b| {
                b.value
                    .as_bool()
                    .is_some()
                    .cmp(&a.value.as_bool().is_some())
                    .then_with(|| a.rect[1].total_cmp(&b.rect[1]))
            });
            let Some(control) = controls.first() else {
                continue;
            };
            self.tried |= 1 << index;
            if control.value == ElementValue::Bool(true) {
                continue;
            }
            self.send(ctx, Command::Click(control.id_hex.clone()));
            return;
        }
    }

    fn send(&mut self, ctx: &egui::Context, command: Command) {
        let (responder, reply) = crossbeam_channel::bounded(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        let request = Request {
            command,
            responder,
            expires: Instant::now() + Duration::from_secs(2),
            cancelled: cancelled.clone(),
        };
        if ui_director::egui::driver()
            .channel()
            .try_send(request)
            .is_ok()
        {
            self.pending = Some(Pending { reply, cancelled });
            ctx.request_repaint();
        }
    }
}
