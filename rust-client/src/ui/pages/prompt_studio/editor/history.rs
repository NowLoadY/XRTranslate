use xrtranslate_prompt::PromptTemplateProfile;

/// Manages undo and redo stacks for prompt studio graph drafts.
#[derive(Clone, Debug, PartialEq)]
pub struct PromptStudioHistory {
    history: crate::ui::graph_editor::GraphEditHistory<PromptTemplateProfile>,
}

impl Default for PromptStudioHistory {
    fn default() -> Self {
        Self::new(60)
    }
}

impl PromptStudioHistory {
    pub fn new(max_depth: usize) -> Self {
        Self {
            history: crate::ui::graph_editor::GraphEditHistory::new(max_depth),
        }
    }

    /// Pushes a snapshot of the profile before mutation.
    pub fn push(&mut self, before: PromptTemplateProfile) {
        if before.read_only {
            return;
        }
        self.history.push(before);
    }

    /// Undoes the last mutation and pushes the current state to the redo stack.
    pub fn undo(&mut self, current: PromptTemplateProfile) -> Option<PromptTemplateProfile> {
        if current.read_only {
            return None;
        }
        self.history.undo(current)
    }

    /// Redoes the undone mutation and pushes the current state to the undo stack.
    pub fn redo(&mut self, current: PromptTemplateProfile) -> Option<PromptTemplateProfile> {
        if current.read_only {
            return None;
        }
        self.history.redo(current)
    }

    pub fn can_undo(&self, read_only: bool) -> bool {
        !read_only && self.history.can_undo()
    }

    pub fn can_redo(&self, read_only: bool) -> bool {
        !read_only && self.history.can_redo()
    }

    pub fn clear(&mut self) {
        self.history.clear();
    }
}
