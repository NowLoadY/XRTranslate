//! Shared language identities, display names and aliases. Model capabilities
//! and wire formats belong to model manifests and provider adapters.

use super::Script;
use Script::{Cyrillic, Devanagari, Han, Hangul, Japanese, Latin, Thai};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SupportedLanguage {
    code: &'static str,
    name: &'static str,
    script: Option<Script>,
}

macro_rules! language_catalog {
    ($(($code:literal, $name:literal, $script:expr)),+ $(,)?) => {
        pub const LANGUAGES: &[SupportedLanguage] = &[
            $(SupportedLanguage { code: $code, name: $name, script: $script }),+
        ];
        /// UI choices generated from the same catalogue, in presentation order.
        pub const LANGUAGE_OPTIONS: &[(&str, &str)] = &[$(($code, $name)),+];
    };
}

language_catalog! {
    ("zh", "Chinese", Some(Han)),
    ("zh-TW", "Traditional Chinese", Some(Han)),
    ("en", "English", Some(Latin)),
    ("fr", "French", Some(Latin)),
    ("pt", "Portuguese", Some(Latin)),
    ("es", "Spanish", Some(Latin)),
    ("ja", "Japanese", Some(Japanese)),
    ("ru", "Russian", Some(Cyrillic)),
    ("ko", "Korean", Some(Hangul)),
    ("th", "Thai", Some(Thai)),
    ("hi", "Hindi", Some(Devanagari)),
    ("it", "Italian", Some(Latin)),
    ("de", "German", Some(Latin)),
    ("vi", "Vietnamese", Some(Latin)),
    ("id", "Indonesian", Some(Latin)),
    ("pl", "Polish", Some(Latin)),
    ("cs", "Czech", Some(Latin)),
    ("nl", "Dutch", Some(Latin)),
    ("bg", "Bulgarian", Some(Cyrillic)),
    ("yue", "Cantonese", Some(Han)),
    ("ar", "Arabic", None),
    ("tr", "Turkish", Some(Latin)),
    ("ms", "Malay", Some(Latin)),
    ("sv", "Swedish", Some(Latin)),
    ("da", "Danish", Some(Latin)),
    ("fi", "Finnish", Some(Latin)),
    ("fil", "Filipino", Some(Latin)),
    ("tl", "Tagalog", Some(Latin)),
    ("fa", "Persian", None),
    ("el", "Greek", None),
    ("hu", "Hungarian", Some(Latin)),
    ("mk", "Macedonian", Some(Cyrillic)),
    ("ro", "Romanian", Some(Latin)),
    ("km", "Khmer", None),
    ("my", "Burmese", None),
    ("gu", "Gujarati", None),
    ("ur", "Urdu", None),
    ("te", "Telugu", None),
    ("mr", "Marathi", Some(Devanagari)),
    ("he", "Hebrew", None),
    ("bn", "Bengali", None),
    ("ta", "Tamil", None),
    ("uk", "Ukrainian", Some(Cyrillic)),
    ("bo", "Tibetan", None),
    ("kk", "Kazakh", Some(Cyrillic)),
    ("mn", "Mongolian", Some(Cyrillic)),
    ("ug", "Uyghur", None),
    ("af", "Afrikaans", Some(Latin)),
    ("no", "Norwegian", Some(Latin)),
    ("hr", "Croatian", Some(Latin)),
    ("sk", "Slovak", Some(Latin)),
}

impl SupportedLanguage {
    /// Resolves a code or locale without allocating; preserves Chinese script
    /// variants for translation instead of collapsing them at the shared layer.
    pub fn from_code(code: &str) -> Option<Self> {
        let mut parts = code.trim().split(['-', '_']);
        let primary = parts.next()?;
        let canonical = if primary.eq_ignore_ascii_case("zh") {
            if parts.clone().any(|tag| tag.eq_ignore_ascii_case("Hant"))
                || (!parts.clone().any(|tag| tag.eq_ignore_ascii_case("Hans"))
                    && parts.any(|tag| {
                        ["TW", "HK", "MO"]
                            .iter()
                            .any(|region| tag.eq_ignore_ascii_case(region))
                    }))
            {
                "zh-TW"
            } else {
                "zh"
            }
        } else if primary.eq_ignore_ascii_case("hin") {
            "hi"
        } else {
            primary
        };
        LANGUAGES
            .iter()
            .copied()
            .find(|language| language.code.eq_ignore_ascii_case(canonical))
    }

    /// Accepts canonical codes, locale tags, English names and shared aliases.
    pub fn parse(label: &str) -> Option<Self> {
        let label = label.trim();
        Self::from_code(label)
            .or_else(|| {
                LANGUAGES
                    .iter()
                    .copied()
                    .find(|language| language.name.eq_ignore_ascii_case(label))
            })
            .or_else(|| {
                [
                    ("Mandarin", "zh"),
                    ("Simplified Chinese", "zh"),
                    ("TraditionalChinese", "zh-TW"),
                    ("Chinese (Traditional)", "zh-TW"),
                ]
                .into_iter()
                .find_map(|(alias, code)| {
                    alias
                        .eq_ignore_ascii_case(label)
                        .then(|| Self::from_code(code))
                        .flatten()
                })
            })
    }

    pub const fn code(self) -> &'static str {
        self.code
    }
    pub const fn name(self) -> &'static str {
        self.name
    }

    /// None means script-only inference is unavailable, not that the language
    /// is unsupported. Explicit model language metadata may still be used.
    pub const fn script(self) -> Option<Script> {
        self.script
    }

    pub fn base_code(self) -> &'static str {
        self.code.split('-').next().unwrap_or(self.code)
    }
}

pub fn is_traditional_chinese(code: &str) -> bool {
    SupportedLanguage::from_code(code).is_some_and(|language| language.code() == "zh-TW")
}
