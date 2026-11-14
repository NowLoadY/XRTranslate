use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use xrtranslate_prompt::{PromptTurn, TranslationPromptContext, estimated_tokens};

/// A session-local allowance for appended history, shared by adapter clones.
/// The prompt graph, current input and the caller's stored history stay intact.
#[derive(Debug, Clone)]
pub(super) struct HistoryBudget(Arc<AtomicUsize>);

impl Default for HistoryBudget {
    fn default() -> Self {
        Self(Arc::new(AtomicUsize::new(usize::MAX)))
    }
}

impl HistoryBudget {
    pub(super) fn apply(&self, context: &mut TranslationPromptContext) -> usize {
        let limit = self.0.load(Ordering::Relaxed);
        let mut remaining = limit;
        let keep = context
            .recent_turns
            .iter()
            .rev()
            .take_while(|turn| {
                let tokens = turn_tokens(turn);
                if tokens > remaining {
                    return false;
                }
                remaining -= tokens;
                true
            })
            .count();
        context
            .recent_turns
            .drain(..context.recent_turns.len() - keep);
        limit
    }

    pub(super) fn reject(&self, context: &TranslationPromptContext) {
        let used = history_tokens(context);
        if used > 0 {
            let limit = used / 2;
            if self.0.fetch_min(limit, Ordering::Relaxed) > limit {
                tracing::warn!(
                    history_tokens = used,
                    history_limit = limit,
                    "Reducing appended translation history after rejected output"
                );
            }
        }
    }

    pub(super) fn recover(&self, limit: usize, requested: usize) {
        if requested > limit {
            // A late success must not undo a newer request's reduction.
            let _ = self.0.compare_exchange(
                limit,
                limit.saturating_add((limit / 16).max(1)),
                Ordering::Relaxed,
                Ordering::Relaxed,
            );
        }
    }
}

pub(super) fn history_tokens(context: &TranslationPromptContext) -> usize {
    context.recent_turns.iter().map(turn_tokens).sum()
}

fn turn_tokens(turn: &PromptTurn) -> usize {
    estimated_tokens(&turn.source_text) + estimated_tokens(&turn.translated_text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> TranslationPromptContext {
        TranslationPromptContext {
            recent_turns: (0..4)
                .map(|index| PromptTurn {
                    turn_id: Some(index.to_string()),
                    speaker_id: String::new(),
                    source_language: "en".into(),
                    target_language: "zh".into(),
                    source_text: "previous words ".repeat(4),
                    translated_text: "此前的对话".repeat(4),
                })
                .collect(),
            terminology_rows: vec!["XRTranslate,XRTranslate".into()],
            ..Default::default()
        }
    }

    #[test]
    fn retains_recent_whole_turns_in_order_and_recovers_gradually() {
        let budget = HistoryBudget::default();
        let original = context();
        budget.reject(&original);
        let mut selected = original.clone();
        let limit = budget.apply(&mut selected);
        assert_eq!(limit, history_tokens(&original) / 2);
        assert_eq!(selected.recent_turns, original.recent_turns[2..]);
        assert_eq!(selected.terminology_rows, original.terminology_rows);
        assert_eq!(original.recent_turns.len(), 4);

        budget.recover(limit, history_tokens(&original));
        let recovered = budget.apply(&mut original.clone());
        assert!(recovered > limit && recovered < history_tokens(&original));
        for _ in 0..32 {
            let limit = budget.apply(&mut original.clone());
            budget.recover(limit, history_tokens(&original));
        }
        let mut selected = original.clone();
        budget.apply(&mut selected);
        assert_eq!(selected, original);
    }

    #[test]
    fn late_success_cannot_undo_reduction_shared_by_clones() {
        let budget = HistoryBudget::default();
        let original = context();
        budget.reject(&original);
        let mut selected = original.clone();
        let previous = budget.apply(&mut selected);
        let worker = budget.clone();
        worker.reject(&selected);
        budget.recover(previous, history_tokens(&original));
        assert_eq!(budget.apply(&mut original.clone()), previous / 2);
        assert_eq!(
            HistoryBudget::default().apply(&mut original.clone()),
            usize::MAX
        );
    }

    #[test]
    fn history_free_failure_does_not_penalize_future_requests() {
        let budget = HistoryBudget::default();
        budget.reject(&TranslationPromptContext::default());
        assert_eq!(budget.apply(&mut context()), usize::MAX);
    }

    #[test]
    fn one_success_does_not_immediately_restore_a_short_failed_history() {
        let budget = HistoryBudget::default();
        let mut original = context();
        original.recent_turns.truncate(1);
        original.recent_turns[0].source_text = "hello".into();
        original.recent_turns[0].translated_text = "你好".into();
        budget.reject(&original);
        let limit = budget.apply(&mut original.clone());
        budget.recover(limit, history_tokens(&original));
        let mut selected = original;
        budget.apply(&mut selected);
        assert!(selected.recent_turns.is_empty());
    }
}
