use crate::CaptureSource;
use crate::i18n::UiLanguage;

/// Plugin-provided identity and presentation metadata for an active session.
///
/// The host treats these values as opaque. In particular, this type never
/// enumerates concrete plugins, so adding a plugin does not change the
/// recognition/translation infrastructure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginSessionOwner {
    plugin_id: &'static str,
    operation_id: String,
    conversation_id: Option<String>,
    display_name_key: &'static str,
    open_label_key: &'static str,
    active_message_key: &'static str,
}

impl PluginSessionOwner {
    pub fn new(
        plugin_id: &'static str,
        operation_id: impl Into<String>,
        display_name_key: &'static str,
        open_label_key: &'static str,
        active_message_key: &'static str,
    ) -> Self {
        Self {
            plugin_id,
            operation_id: operation_id.into(),
            conversation_id: None,
            display_name_key,
            open_label_key,
            active_message_key,
        }
    }

    pub const fn plugin_id(&self) -> &'static str {
        self.plugin_id
    }

    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }

    /// Groups successive requests into one dialogue while keeping their results
    /// and cancellation identities independent.
    pub fn in_conversation(mut self, conversation_id: impl Into<String>) -> Self {
        self.conversation_id = Some(conversation_id.into());
        self
    }

    fn conversation_id(&self) -> &str {
        self.conversation_id
            .as_deref()
            .unwrap_or(&self.operation_id)
    }

    pub fn display_name(&self, language: UiLanguage) -> &'static str {
        crate::i18n::tr(language, self.display_name_key)
    }

    pub fn open_label(&self, language: UiLanguage) -> &'static str {
        crate::i18n::tr(language, self.open_label_key)
    }

    pub fn active_message(&self, language: UiLanguage) -> &'static str {
        crate::i18n::tr(language, self.active_message_key)
    }
}

/// Identifies who currently owns the translation task without
/// teaching the session infrastructure about individual plugins.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TranslationSessionOwner {
    #[default]
    None,
    Host {
        capture_source: CaptureSource,
    },
    Plugin(PluginSessionOwner),
}

impl TranslationSessionOwner {
    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }

    pub fn is_active(&self) -> bool {
        !self.is_none()
    }

    pub fn is_host(&self) -> bool {
        matches!(self, Self::Host { .. })
    }

    pub fn plugin(&self) -> Option<&PluginSessionOwner> {
        match self {
            Self::Plugin(owner) => Some(owner),
            _ => None,
        }
    }

    pub fn is_plugin(&self, plugin_id: &str) -> bool {
        self.plugin()
            .is_some_and(|owner| owner.plugin_id() == plugin_id)
    }

    pub fn operation_id(&self) -> Option<&str> {
        self.plugin().map(PluginSessionOwner::operation_id)
    }

    pub(crate) fn same_conversation(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Plugin(left), Self::Plugin(right)) => {
                left.plugin_id == right.plugin_id
                    && left.conversation_id() == right.conversation_id()
            }
            _ => self == other,
        }
    }

    pub fn display_name(&self, language: UiLanguage) -> &'static str {
        match self {
            Self::None => match language {
                UiLanguage::Chinese => "空闲",
                UiLanguage::Japanese => "アイドル",
                UiLanguage::Korean => "대기 중",
                UiLanguage::Russian => "Свободно",
                UiLanguage::English => "Idle",
            },
            Self::Host { .. } => match language {
                UiLanguage::Chinese => "实时翻译",
                UiLanguage::Japanese => "リアルタイム翻訳",
                UiLanguage::Korean => "실시간 번역",
                UiLanguage::Russian => "Прямой перевод",
                UiLanguage::English => "Live Translation",
            },
            Self::Plugin(owner) => owner.display_name(language),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_identity_is_opaque_to_the_session_owner() {
        let owner = TranslationSessionOwner::Plugin(PluginSessionOwner::new(
            "example",
            "operation-1",
            "Live Translation",
            "Start Translation",
            "Idle",
        ));

        assert!(owner.is_active());
        assert!(owner.is_plugin("example"));
        assert_eq!(owner.operation_id(), Some("operation-1"));
        assert_eq!(owner.display_name(UiLanguage::English), "Live Translation");
    }

    #[test]
    fn host_sessions_remain_distinct_from_plugin_sessions() {
        let owner = TranslationSessionOwner::Host {
            capture_source: CaptureSource::Microphone,
        };
        assert!(owner.is_host());
        assert!(!owner.is_plugin("example"));
        assert_eq!(owner.display_name(UiLanguage::Chinese), "实时翻译");
    }
}
