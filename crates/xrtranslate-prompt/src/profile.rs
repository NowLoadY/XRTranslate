use serde::{Deserialize, Serialize};

use crate::PromptTarget;

/// Model-declared instruction contract, independent of host, connector or vendor.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranslationPromptProfile {
    #[default]
    ChatMessages,
    Contextual,
    Bilingual,
}

impl TranslationPromptProfile {
    pub const fn target(self) -> PromptTarget {
        match self {
            Self::ChatMessages => PromptTarget::ChatMessages,
            Self::Contextual | Self::Bilingual => PromptTarget::TranslationInstruction,
        }
    }

    pub const fn supports_style(self) -> bool {
        !matches!(self, Self::Bilingual)
    }
}
