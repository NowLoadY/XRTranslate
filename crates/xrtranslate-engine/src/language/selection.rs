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
    pub fn recognition_sources(self) -> LanguageSet {
        self.recognition.unwrap_or(LanguageSet::ALL)
    }

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
            return Ok(LanguageSelection {
                mode: LanguageMode::Bidirectional([a, b]),
                additional_target: None,
            });
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
        self.select_with_options(source, target, None, false)
    }

    pub fn select_with_options(
        self,
        source: &str,
        target: &str,
        additional_target: Option<&str>,
        asr_only: bool,
    ) -> Result<LanguageSelection, String> {
        let selection =
            LanguageSelection::parse_with_options(source, target, additional_target, asr_only)?;
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
        match selection.mode {
            LanguageMode::AsrOnly { source } => {
                if let Some(source) = source {
                    require(self.recognition_sources(), source, "recognition")?;
                }
                if self.recognition_sources().is_empty() {
                    return Err("No recognition language is available".into());
                }
            }
            LanguageMode::Fixed { source, target } => {
                require(self.sources(), source, "input")?;
                require(self.targets(), target, "translation")?;
            }
            LanguageMode::Detect { target } => {
                require(self.targets(), target, "translation")?;
                if self.sources().is_empty() {
                    return Err("The selected services have no common input language".into());
                }
            }
            LanguageMode::Bidirectional(pair) => {
                for language in pair {
                    require(self.sources(), language, "bidirectional translation")?;
                }
            }
        }
        if let Some(additional) = selection.additional_target {
            require(self.targets(), additional, "additional translation")?;
        }
        Ok(selection)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LanguageSelection {
    pub mode: LanguageMode,
    pub additional_target: Option<SupportedLanguage>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LanguageMode {
    AsrOnly {
        source: Option<SupportedLanguage>,
    },
    Fixed {
        source: SupportedLanguage,
        target: SupportedLanguage,
    },
    Detect {
        target: SupportedLanguage,
    },
    Bidirectional([SupportedLanguage; 2]),
}

impl LanguageMode {
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
            Self::AsrOnly { source } => {
                let source = source.map_or("auto", SupportedLanguage::code).to_owned();
                (source.clone(), source)
            }
            Self::Fixed { source, target } => (source.code().into(), target.code().into()),
            Self::Detect { target } => ("auto".into(), target.code().into()),
            Self::Bidirectional([a, b]) => ("auto".into(), format!("{},{}", a.code(), b.code())),
        }
    }
}

impl LanguageSelection {
    pub fn parse(source: &str, target: &str) -> Result<Self, String> {
        Self::parse_with_options(source, target, None, false)
    }

    pub fn parse_with_options(
        source: &str,
        target: &str,
        additional_target: Option<&str>,
        asr_only: bool,
    ) -> Result<Self, String> {
        if asr_only {
            let source = if source.trim().eq_ignore_ascii_case("auto") {
                None
            } else {
                Some(
                    SupportedLanguage::parse(source)
                        .ok_or_else(|| format!("Unknown language: {source:?}"))?,
                )
            };
            return Ok(Self {
                mode: LanguageMode::AsrOnly { source },
                additional_target: None,
            });
        }
        let mode = LanguageMode::parse(source, target)?;
        let additional_target = additional_target
            .filter(|target| !target.trim().is_empty())
            .map(|code| {
                SupportedLanguage::parse(code).ok_or_else(|| format!("Unknown language: {code:?}"))
            })
            .transpose()?;
        if let Some(additional) = additional_target {
            let duplicate = match mode {
                LanguageMode::Fixed { target, .. } => additional == target,
                LanguageMode::Detect { target } => additional == target,
                LanguageMode::Bidirectional(pair) => pair.contains(&additional),
                LanguageMode::AsrOnly { .. } => false,
            };
            if duplicate {
                return Err("The additional target must differ from the main languages".into());
            }
        }
        Ok(Self {
            mode,
            additional_target,
        })
    }

    pub fn wire(self) -> (String, String) {
        self.mode.wire()
    }
    pub fn asr_only(self) -> bool {
        matches!(self.mode, LanguageMode::AsrOnly { .. })
    }
    pub fn additional_target(self) -> Option<SupportedLanguage> {
        self.additional_target
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_additional_target_survives_primary_mode_and_direction_changes() {
        let caps = LanguageCapabilities::default();
        for (source, target) in [
            ("auto", "en,zh"),
            ("en", "zh"),
            ("zh", "en"),
            ("auto", "en"),
        ] {
            let selection = caps
                .select_with_options(source, target, Some("ja"), false)
                .unwrap();
            assert_eq!(selection.additional_target().unwrap().code(), "ja");
            assert_eq!(selection.wire(), (source.into(), target.into()));
            assert!(!selection.asr_only());
        }
        // A newly detected source can equal the independent target. Runtime
        // preserves that route and produces its corrected source directly.
        assert!(
            caps.select_with_options("ja", "en", Some("ja"), false)
                .is_ok()
        );
        assert!(
            caps.select_with_options("en", "ja", Some("ja"), false)
                .is_err()
        );
        assert!(
            caps.select_with_options("auto", "en,ja", Some("ja"), false)
                .is_err()
        );
    }

    #[test]
    fn asr_only_uses_recognition_capabilities_and_discards_inactive_extra_intent() {
        let caps = LanguageCapabilities {
            recognition: Some(LanguageSet::from_codes(&["ja"], true)),
            translation: Some(LanguageSet::EMPTY),
        };
        let selection = caps
            .select_with_options("ja", "en", Some("fr"), true)
            .unwrap();
        assert!(selection.asr_only());
        assert_eq!(selection.additional_target(), None);
        assert_eq!(selection.wire(), ("ja".into(), "ja".into()));
        assert!(caps.select_with_options("auto", "", None, true).is_ok());
        assert!(caps.select_with_options("en", "", None, true).is_err());
        assert!(caps.select("ja", "en").is_err());
        let caps = LanguageCapabilities {
            recognition: None,
            translation: Some(LanguageSet::from_codes(&["en", "ja"], false)),
        };
        assert!(
            caps.select_with_options("en", "ja", Some("fr"), false)
                .is_err()
        );
    }

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
