//! Centralized UI copy and translations.
//!
//! UI code always uses the English source text as a key. Adding a language is
//! a single consolidated table addition rather than separate arrays per language.
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UiLanguage {
    #[default]
    English,
    Chinese,
    Japanese,
    Korean,
    Russian,
}

impl UiLanguage {
    pub const ALL: [Self; 5] = [
        Self::English,
        Self::Chinese,
        Self::Japanese,
        Self::Korean,
        Self::Russian,
    ];

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::Chinese => "中文",
            Self::Japanese => "日本語",
            Self::Korean => "한국어",
            Self::Russian => "Русский",
        }
    }

    /// Parses a standard locale string (e.g. "zh-CN", "zh_Hans", "ja-JP", "ko_KR", "ru-RU", "en-US")
    /// into a supported `UiLanguage`. Returns `None` if the language prefix is not supported.
    pub fn from_locale_str(locale: &str) -> Option<Self> {
        let trimmed = locale.trim().to_ascii_lowercase().replace('_', "-");
        let prefix = trimmed.split(['-', '.']).next().unwrap_or("");
        match prefix {
            "zh" => Some(Self::Chinese),
            "ja" => Some(Self::Japanese),
            "ko" => Some(Self::Korean),
            "ru" => Some(Self::Russian),
            "en" => Some(Self::English),
            _ => None,
        }
    }

    /// Detects the host operating system language. If the system language is not supported
    /// by the UI, falls back to `UiLanguage::English`.
    pub fn detect_system_language() -> Self {
        #[cfg(windows)]
        {
            unsafe extern "system" {
                fn GetUserDefaultLocaleName(lpLocaleName: *mut u16, cchLocaleName: i32) -> i32;
            }
            let mut buffer = [0u16; 85];
            let len = unsafe { GetUserDefaultLocaleName(buffer.as_mut_ptr(), buffer.len() as i32) };
            if len > 1 {
                let name = String::from_utf16_lossy(&buffer[..(len as usize - 1)]);
                if let Some(lang) = Self::from_locale_str(&name) {
                    return lang;
                }
            }
        }

        for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
            if let Ok(val) = std::env::var(var) {
                if let Some(lang) = Self::from_locale_str(&val) {
                    return lang;
                }
            }
        }

        Self::English
    }
}

/// Looks up a fixed UI string from the consolidated multi-language dictionary.
/// Missing translations intentionally fall back to the English source key.
pub fn tr(language: UiLanguage, english: &'static str) -> &'static str {
    translation(language, english).unwrap_or(english)
}

pub fn usage_notice_items(language: UiLanguage) -> &'static [&'static str] {
    crate::usage_guidelines::notice_summary_items(language)
}

/// Dynamic counterpart for status text that originates outside the UI layer.
pub fn tr_dynamic<'a>(language: UiLanguage, english: &'a str) -> Cow<'a, str> {
    Cow::Borrowed(translation(language, english).unwrap_or(english))
}

pub(crate) fn translation(language: UiLanguage, english: &str) -> Option<&'static str> {
    if language == UiLanguage::English {
        return None;
    }
    // Natural-case toolbar labels reuse the translations of earlier uppercase
    // labels. Exact keys still win when copy intentionally differs by context.
    type Entry = (
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        &'static str,
    );
    type Index = (
        std::collections::HashMap<&'static str, &'static Entry>,
        std::collections::HashMap<String, &'static Entry>,
    );
    static INDEX: std::sync::OnceLock<Index> = std::sync::OnceLock::new();
    let (exact, folded) = INDEX.get_or_init(|| {
        let mut exact = std::collections::HashMap::with_capacity(DICTIONARY.len());
        let mut folded = std::collections::HashMap::with_capacity(DICTIONARY.len());
        for entry in DICTIONARY {
            exact.entry(entry.0).or_insert(entry);
            folded.entry(entry.0.to_ascii_lowercase()).or_insert(entry);
        }
        (exact, folded)
    });
    let (_, zh, ja, ko, ru) = exact
        .get(english)
        .or_else(|| folded.get(&english.to_ascii_lowercase()))?;
    Some(match language {
        UiLanguage::Chinese => zh,
        UiLanguage::Japanese => ja,
        UiLanguage::Korean => ko,
        UiLanguage::Russian => ru,
        UiLanguage::English => unreachable!(),
    })
}

mod strings;
use strings::DICTIONARY;
