//! Brief reactions to application activity, without changing the companion's placement.
use crate::ui::components::avatar::Expression;

#[derive(Clone, Default)]
struct Activity {
    translating: bool,
    completed: Option<(usize, u64)>,
    voice_busy: bool,
    failed: bool,
}

#[derive(Clone, Default)]
pub(super) struct Feedback {
    previous: Option<Activity>,
    expression: Expression,
    until: f64,
}

impl Feedback {
    pub fn update(&mut self, app: &crate::XRTranslateApp, now: f64) -> Option<Expression> {
        let current = Activity {
            translating: app.is_translating,
            completed: app
                .translations
                .last()
                .filter(|entry| !entry.live && !entry.translated.is_empty())
                .map(|entry| (app.translations.len(), entry.source_end_ms.to_bits())),
            voice_busy: app.tts_center.busy(),
            failed: app.last_error.is_some() || app.tts_center.error.is_some(),
        };
        if let Some(previous) = &self.previous {
            let expression = if (current.failed && !previous.failed)
                || (current.translating && !previous.translating)
                || (current.voice_busy && !previous.voice_busy)
            {
                Some(Expression::Curious)
            } else if !current.failed
                && ((current.completed.is_some() && current.completed != previous.completed)
                    || (previous.voice_busy && !current.voice_busy && app.tts_center.status.ready))
            {
                Some(Expression::Happy)
            } else {
                None
            };
            if let Some(expression) = expression {
                self.expression = expression;
                self.until = now + 1.6;
            }
        }
        self.previous = Some(current);
        (now < self.until).then_some(self.expression)
    }
}
