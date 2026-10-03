//! One attention and activity policy, shared by every place the companion appears.
use crate::ui::components::avatar::Expression;
use std::collections::HashSet;

/// Facts from actual inference requests, independent of recognition and UI ticks.
#[derive(Default)]
pub(crate) struct TranslationWork {
    active: HashSet<(u64, u64)>,
    started: u64,
}

impl TranslationWork {
    pub(crate) fn observe(&mut self, stream: u64, request: u64, active: bool) {
        if active {
            if self.active.insert((stream, request)) {
                self.started = self.started.wrapping_add(1);
            }
        } else {
            self.active.remove(&(stream, request));
        }
    }

    pub(crate) fn retire_stream(&mut self, stream: u64) {
        self.active.retain(|(owner, _)| *owner != stream);
    }
}

#[derive(Clone, Default)]
struct Activity {
    completed: Option<(usize, u64)>,
    voice_busy: bool,
    voice_ready: bool,
    working: bool,
    started: u64,
    greeting: bool,
    failed: bool,
}

#[derive(Clone, Default)]
pub(super) struct Feedback {
    previous: Option<Activity>,
    expression: Expression,
    until: f64,
    attention_since: Option<f64>,
    acknowledged: bool,
    acknowledge_after: f64,
    thinking_until: f64,
}

impl Feedback {
    pub fn update(
        &mut self,
        app: &crate::XRTranslateApp,
        now: f64,
        attentive: bool,
        clicked: bool,
    ) -> Option<Expression> {
        let (working, started) = app.shared_session_state.lock().map_or((false, 0), |state| {
            (
                !state.companion_work.active.is_empty(),
                state.companion_work.started,
            )
        });
        self.observe(
            Activity {
                completed: app
                    .translations
                    .last()
                    .filter(|entry| !entry.live && !entry.translated.is_empty())
                    .map(|entry| (app.translations.len(), entry.source_end_ms.to_bits())),
                voice_busy: app.tts_center.busy(),
                voice_ready: app.tts_center.status.ready,
                working,
                started,
                greeting: app.vr_overlay_plugin.manager().status().avatar_greeting,
                failed: app.last_error.is_some() || app.tts_center.error.is_some(),
            },
            now,
            attentive,
            clicked,
        )
    }

    fn observe(
        &mut self,
        current: Activity,
        now: f64,
        attentive: bool,
        clicked: bool,
    ) -> Option<Expression> {
        let acknowledge = if attentive {
            let since = *self.attention_since.get_or_insert(now);
            !self.acknowledged && now - since >= 0.7 && now >= self.acknowledge_after
        } else {
            self.attention_since = None;
            self.acknowledged = false;
            false
        };
        if acknowledge || clicked {
            // Continuing to stare never loops the greeting; brief tracking losses
            // also cannot turn it into a repeated smile.
            self.acknowledged = true;
            self.acknowledge_after = now + 4.0;
        }
        // A fast request can start and finish between two UI frames. Preserve a
        // short entry gesture, while long requests remain Thinking until done.
        if self
            .previous
            .as_ref()
            .map_or(0, |previous| previous.started)
            != current.started
        {
            self.thinking_until = now + 0.4;
        }
        let busy = current.voice_busy || current.working || now < self.thinking_until;
        let reaction = if current.failed
            && self
                .previous
                .as_ref()
                .is_none_or(|previous| !previous.failed)
        {
            Some(Expression::Concerned)
        } else if now < self.until && self.expression == Expression::Concerned {
            None
        } else if let Some(previous) = &self.previous
            && !current.failed
            && ((current.completed.is_some() && current.completed != previous.completed)
                || (previous.voice_busy && !current.voice_busy && current.voice_ready))
        {
            Some(Expression::Happy)
        } else if !busy && !current.failed && (clicked || (acknowledge && now >= self.until)) {
            Some(Expression::Happy)
        } else {
            None
        };
        if let Some(expression) = reaction {
            self.expression = expression;
            self.until = now + 1.6;
        }
        let expression = if now < self.until && self.expression == Expression::Concerned {
            Some(self.expression)
        } else if busy {
            Some(Expression::Thinking)
        } else if current.greeting {
            Some(Expression::Happy)
        } else if now < self.until {
            Some(self.expression)
        } else if attentive && !current.failed {
            Some(Expression::Curious)
        } else {
            None
        };
        self.previous = Some(current);
        expression
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actual_translation_has_priority_and_fast_requests_remain_visible() {
        let mut work = TranslationWork::default();
        work.observe(1, 10, true);
        work.observe(1, 10, true);
        work.observe(2, 11, true);
        work.retire_stream(1);
        assert_eq!(work.active.len(), 1);
        work.observe(2, 11, false);
        assert_eq!(work.started, 2);
        assert!(work.active.is_empty());

        let mut feedback = Feedback::default();
        let mut activity = Activity::default();
        assert_eq!(feedback.observe(activity.clone(), 0.0, false, false), None);
        activity.started = work.started; // Both ended before the first UI frame.
        assert_eq!(
            feedback.observe(activity.clone(), 0.1, false, false),
            Some(Expression::Thinking)
        );
        assert_eq!(
            feedback.observe(activity.clone(), 0.3, false, false),
            Some(Expression::Thinking)
        );
        assert_eq!(feedback.observe(activity.clone(), 0.6, false, false), None);
        activity.greeting = true;
        assert_eq!(
            feedback.observe(activity.clone(), 0.7, false, false),
            Some(Expression::Happy)
        );
        activity.working = true;
        activity.started += 1;
        assert_eq!(
            feedback.observe(activity.clone(), 0.8, true, true),
            Some(Expression::Thinking)
        );
        assert_eq!(
            feedback.observe(activity.clone(), 5.0, false, false),
            Some(Expression::Thinking)
        );
        activity.working = false;
        assert_eq!(
            feedback.observe(activity.clone(), 5.1, false, false),
            Some(Expression::Happy)
        );
        activity.greeting = false;
        assert_eq!(feedback.observe(activity, 5.2, false, false), None);
    }
}
