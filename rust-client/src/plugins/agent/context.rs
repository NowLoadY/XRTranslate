//! Caller-owned messages. Keep a stable prefix and compact only at the input limit.
use xrtranslate_prompt::{PromptMessage, PromptMessageRole, estimated_tokens};

#[derive(Default)]
pub(super) struct Context {
    pub messages: Vec<PromptMessage>,
    pub memory: String,
    // The progress note covers all messages through this assistant reply.
    summarized: usize,
    budget: usize,
}

impl Context {
    /// Begin another execution in the same conversation. Only the current rules
    /// and capability scope are replaced; user messages and work stay in order.
    pub fn begin(
        &mut self,
        instructions: String,
        task: String,
        budget: usize,
    ) -> Result<(), String> {
        if estimated_tokens(&instructions) + estimated_tokens(&task) + 64 > budget {
            return Err("The configured context window is too small for the task and tools".into());
        }
        if self.messages.is_empty() {
            self.push(PromptMessageRole::System, instructions);
            self.summarized = 1;
        } else {
            self.messages[0].content = instructions;
        }
        self.budget = budget;
        self.push(PromptMessageRole::User, task);
        Ok(())
    }

    pub fn push(&mut self, role: PromptMessageRole, content: String) {
        self.messages.push(PromptMessage { role, content });
    }

    pub fn remember(&mut self, note: Option<String>) {
        if let Some(note) = note.filter(|note| !note.trim().is_empty()) {
            self.memory = note.chars().take(1200).collect();
            self.summarized = self.messages.len();
        }
    }

    fn size(&self) -> usize {
        self.messages
            .iter()
            .map(|message| estimated_tokens(&message.content) + 32)
            .sum()
    }

    pub fn prepare(&mut self) -> Result<Vec<PromptMessage>, String> {
        let before = self.size();
        if before > self.budget && !self.memory.is_empty() {
            let summary = PromptMessage {
                role: PromptMessageRole::Assistant,
                content: format!("Progress note from earlier steps:\n{}", self.memory),
            };
            self.messages.splice(1..self.summarized, [summary]);
            self.summarized = 2;
            log::info!(
                "Agent context compacted: estimated_tokens={before}->{}, budget={}, retained_messages={}",
                self.size(),
                self.budget,
                self.messages.len()
            );
        }
        if self.size() > self.budget {
            // Preserve the latest result intact; silently truncating JSON or
            // source text would make a subsequent edit unreliable.
            return Err("Agent context limit reached; use smaller document pages or a larger model context window".into());
        }
        Ok(self.messages.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_across_executions_and_compact_only_summarized_history() {
        let mut context = Context::default();
        context
            .begin("rules and scope".into(), "goal".into(), 400)
            .unwrap();
        let prefix = context.prepare().unwrap();
        context.push(PromptMessageRole::Assistant, "first action".into());
        context.push(PromptMessageRole::User, "first result".repeat(15));
        context.push(PromptMessageRole::Assistant, "second action".into());
        context.remember(Some(
            "First change verified; second action awaiting result".into(),
        ));
        context.push(PromptMessageRole::User, "second result".into());
        let previous = context.prepare().unwrap();
        assert_eq!(&previous[..prefix.len()], prefix);
        assert_eq!(previous.len(), 6, "do not rewrite history below the budget");

        // A reply that omits a new note must not discard the last useful note
        // or claim that newer feedback was included in that older summary.
        context.push(PromptMessageRole::Assistant, "invalid decision".into());
        context.remember(None);
        context.push(PromptMessageRole::User, "error feedback".repeat(18));
        context.push(
            PromptMessageRole::User,
            "User follow-up instruction:\nKeep timestamps unchanged".into(),
        );
        let unsummarized = context.messages[5..].to_vec();
        assert!(context.size() > context.budget);
        let compacted = context.prepare().unwrap();
        assert_eq!(&compacted[..1], &prefix[..1]);
        assert!(compacted[1].content.contains("First change verified"));
        assert_eq!(&compacted[2..], unsummarized);
        assert_eq!(context.prepare().unwrap(), compacted);

        context
            .begin(
                "rules with the next scope".into(),
                "Continue the same document".into(),
                400,
            )
            .unwrap();
        assert_eq!(&context.messages[1..compacted.len()], &compacted[1..]);
        assert_eq!(
            context.messages.last().unwrap().content,
            "Continue the same document"
        );
        assert_eq!(context.messages[0].content, "rules with the next scope");
        let retained = context.messages.clone();
        assert!(
            context
                .begin("rules".repeat(100), "too large".into(), 1)
                .is_err()
        );
        assert_eq!(
            context.messages, retained,
            "reject before changing the conversation"
        );

        context.push(PromptMessageRole::Assistant, "read a large page".into());
        context.remember(Some("Earlier changes verified".into()));
        context.push(PromptMessageRole::User, "完整结果".repeat(100));
        let latest = context.messages.last().unwrap().clone();
        assert!(context.prepare().is_err());
        assert_eq!(context.messages.last(), Some(&latest));
    }
}
