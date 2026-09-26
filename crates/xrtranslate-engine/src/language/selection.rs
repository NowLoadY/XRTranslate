//! Model-independent task intent and language availability. The legacy wire
//! format is parsed once at the boundary; selection never changes user intent.

use super::{LANGUAGES, SupportedLanguage};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LanguageSet(u64);

impl LanguageSet {
    pub const ALL: Self = Self((1 << LANGUAGES.len()) - 1);
    pub const EMPTY: Self = Self(0);

    pub fn from_codes(codes: &[&str], spoken: bool) -> Self {
        Self::matching(|language| {
            codes
                .iter()
                .filter_map(|code| SupportedLanguage::parse(code))
                .any(|candidate| {
                    candidate == language
                        || (spoken && candidate.base_code() == language.base_code())
                })
        })
    }

    pub fn matching(mut predicate: impl FnMut(SupportedLanguage) -> bool) -> Self {
        Self(
            LANGUAGES
                .iter()
                .enumerate()
                .fold(0, |bits, (index, language)| {
                    bits | (u64::from(predicate(*language)) << index)
                }),
        )
    }

    pub fn contains(self, language: SupportedLanguage) -> bool {
        self.iter().any(|candidate| candidate == language)
    }

    pub fn iter(self) -> impl Iterator<Item = SupportedLanguage> {
        LANGUAGES
            .iter()
            .enumerate()
            .filter_map(move |(index, language)| (self.0 & (1 << index) != 0).then_some(*language))
    }

    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn options(self) -> Vec<(&'static str, &'static str)> {
        self.iter()
            .map(|language| (language.code(), language.name()))
            .collect()
    }
}

/// None is unspecified provider support, not a guarantee of universal support.
/// Such providers remain usable and perform their final validation at inference.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LanguageCapabilities {
    pub recognition: Option<LanguageSet>,
    pub translation: Option<LanguageSet>,
}

impl LanguageCapabilities {
    pub fn sources(self) -> LanguageSet {
        self.recognition
            .unwrap_or(LanguageSet::ALL)
            .intersection(self.targets())
    }

    pub fn targets(self) -> LanguageSet {
        self.translation.unwrap_or(LanguageSet::ALL)
    }

    pub fn for_text(self) -> Self {
        Self {
            recognition: None,
            ..self
        }
    }

    /// Choose a valid target only in response to an explicit input-mode change.
    /// Opening a task or changing model availability must never call this.
    pub fn change_input(
        self,
        source: &str,
        target: &str,
        bidirectional: bool,
    ) -> Result<LanguageSelection, String> {
        let preferred: Vec<_> = target
            .split(',')
            .filter_map(SupportedLanguage::parse)
            .collect();
        if source == "auto" && bidirectional {
            let sources = self.sources();
            let a = preferred
                .iter()
                .copied()
                .find(|language| sources.contains(*language))
                .or_else(|| sources.iter().next())
                .ok_or("No language is available for bidirectional translation")?;
            let b = preferred
                .iter()
                .copied()
                .chain(sources.iter())
                .find(|language| {
                    sources.contains(*language) && language.base_code() != a.base_code()
                })
                .ok_or("Bidirectional translation requires two available spoken languages")?;
            return Ok(LanguageSelection::Bidirectional([a, b]));
        }
        let source_language = SupportedLanguage::parse(source);
        let target = preferred
            .into_iter()
            .chain(self.targets().iter())
            .find(|language| {
                self.targets().contains(*language) && Some(*language) != source_language
            })
            .ok_or("No translation target is available")?;
        self.select(source, target.code())
    }

    pub fn select(self, source: &str, target: &str) -> Result<LanguageSelection, String> {
        let selection = LanguageSelection::parse(source, target)?;
        let require = |set: LanguageSet, language: SupportedLanguage, role: &str| {
            if set.contains(language) {
                Ok(())
            } else {
                Err(format!(
                    "{} is unavailable for {role} with the selected services",
                    language.name()
                ))
            }
        };
        match selection {
            LanguageSelection::Fixed { source, target } => {
                require(self.sources(), source, "input")?;
                require(self.targets(), target, "translation")?;
            }
            LanguageSelection::Detect { target } => {
                require(self.targets(), target, "translation")?;
                if self.sources().is_empty() {
                    return Err("The selected services have no common input language".into());
                }
            }
            LanguageSelection::Bidirectional(pair) => {
                for language in pair {
                    require(self.sources(), language, "bidirectional translation")?;
                }
            }
        }
        Ok(selection)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LanguageSelection {
    Fixed {
        source: SupportedLanguage,
        target: SupportedLanguage,
    },
    Detect {
        target: SupportedLanguage,
    },
    Bidirectional([SupportedLanguage; 2]),
}

impl LanguageSelection {
    pub fn parse(source: &str, target: &str) -> Result<Self, String> {
        let language = |code| {
            SupportedLanguage::parse(code).ok_or_else(|| format!("Unknown language: {code:?}"))
        };
        if source.trim().eq_ignore_ascii_case("auto") {
            if let Some((a, b)) = target.split_once(',') {
                let pair = [language(a)?, language(b)?];
                if pair[0].base_code() == pair[1].base_code() {
                    return Err(
                        "Bidirectional translation requires two different spoken languages".into(),
                    );
                }
                Ok(Self::Bidirectional(pair))
            } else {
                Ok(Self::Detect {
                    target: language(target)?,
                })
            }
        } else {
            Ok(Self::Fixed {
                source: language(source)?,
                target: language(target)?,
            })
        }
    }

    pub fn wire(self) -> (String, String) {
        match self {
            Self::Fixed { source, target } => (source.code().into(), target.code().into()),
            Self::Detect { target } => ("auto".into(), target.code().into()),
            Self::Bidirectional([a, b]) => ("auto".into(), format!("{},{}", a.code(), b.code())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switching_modes_uses_available_languages_without_chinese_or_english_defaults() {
        let caps = LanguageCapabilities {
            recognition: Some(LanguageSet::from_codes(&["ja", "ko"], true)),
            translation: Some(LanguageSet::from_codes(&["ja", "ko", "fr"], false)),
        };
        assert_eq!(
            caps.change_input("auto", "fr", true).unwrap().wire(),
            ("auto".into(), "ja,ko".into())
        );
        assert_eq!(
            caps.change_input("ko", "ja,ko", false).unwrap().wire(),
            ("ko".into(), "ja".into())
        );
        assert_eq!(
            caps.change_input("auto", "fr", false).unwrap().wire(),
            ("auto".into(), "fr".into())
        );
        let one = LanguageCapabilities {
            recognition: Some(LanguageSet::from_codes(&["ja"], true)),
            ..caps
        };
        assert!(one.change_input("auto", "fr", true).is_err());
    }

    #[test]
    fn legacy_requests_keep_their_direction_and_normalize_aliases() {
        for (source, target, expected) in [
            ("Japanese", "en-US", ("ja", "en")),
            ("auto", "zh-TW", ("auto", "zh-TW")),
            ("AUTO", "Japanese,English", ("auto", "ja,en")),
        ] {
            let (source, target) = LanguageSelection::parse(source, target).unwrap().wire();
            assert_eq!((source.as_str(), target.as_str()), expected);
        }
        for (source, target) in [
            ("auto", "zh,zh-TW"),
            ("auto", "en,ja,ko"),
            ("ja", "en,ko"),
            ("", "en"),
        ] {
            assert!(LanguageSelection::parse(source, target).is_err());
        }
    }

    #[test]
    fn empty_or_insufficient_capabilities_never_fall_back_to_other_languages() {
        let caps = LanguageCapabilities {
            recognition: Some(LanguageSet::from_codes(&["ja", "ko"], true)),
            translation: Some(LanguageSet::from_codes(&["en", "ja"], false)),
        };
        assert!(caps.select("ja", "en").is_ok());
        assert!(caps.select("auto", "en").is_ok());
        assert!(caps.select("auto", "ja,en").is_err());
        assert!(caps.select("ko", "en").is_err());
        assert!(caps.select("ja", "ko").is_err());
        assert!(caps.for_text().select("en", "ja").is_ok());
        let empty = LanguageCapabilities {
            recognition: Some(LanguageSet::EMPTY),
            ..caps
        };
        assert!(empty.select("auto", "en").is_err());
        assert!(empty.sources().options().is_empty());
    }

    #[test]
    fn script_variants_expand_only_for_speech_support() {
        let traditional = SupportedLanguage::parse("zh-TW").unwrap();
        assert!(LanguageSet::from_codes(&["zh"], true).contains(traditional));
        assert!(!LanguageSet::from_codes(&["zh"], false).contains(traditional));
        assert_eq!(LanguageSet::ALL.iter().count(), LANGUAGES.len());
        assert!(LANGUAGES.len() < 64);
    }
}
