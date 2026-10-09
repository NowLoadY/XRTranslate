use std::collections::VecDeque;

use super::{LanguageSelection, LanguageSet, Script, SupportedLanguage};

const RECENT_OBSERVATIONS_LIMIT: usize = 5;
const SWITCH_CANDIDATE_THRESHOLD: usize = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LanguagePair(pub [SupportedLanguage; 2]);

impl LanguagePair {
    pub fn target_lang(self) -> String {
        format!("{},{}", self.0[0].code(), self.0[1].code())
    }

    fn parse(source: &str, targets: &str) -> Option<Self> {
        match LanguageSelection::parse(source, targets).ok()?.mode {
            super::LanguageMode::Bidirectional(pair) => Some(Self(pair)),
            _ => None,
        }
    }

    fn find_matching(self, language: SupportedLanguage) -> Option<SupportedLanguage> {
        if self.0[0] == language
            || (self.0[0].base_code() == language.base_code()
                && self.0[0].script() == language.script())
        {
            Some(self.0[0])
        } else if self.0[1] == language
            || (self.0[1].base_code() == language.base_code()
                && self.0[1].script() == language.script())
        {
            Some(self.0[1])
        } else {
            None
        }
    }

    fn contains(self, language: SupportedLanguage) -> bool {
        self.find_matching(language).is_some()
    }

    fn other(self, language: SupportedLanguage) -> SupportedLanguage {
        let matched = self.find_matching(language).unwrap_or(language);
        if matched == self.0[0] {
            self.0[1]
        } else {
            self.0[0]
        }
    }

    fn route(self, source: SupportedLanguage) -> LanguageRoute {
        let matched = self.find_matching(source).unwrap_or(source);
        LanguageRoute {
            source: matched,
            target: self.other(matched),
        }
    }

    fn language_for_text(self, text: &str) -> Option<SupportedLanguage> {
        let first = script_evidence(self.0[0], text);
        let second = script_evidence(self.0[1], text);
        match (first, second) {
            (Evidence::Compatible, Evidence::Incompatible) => Some(self.0[0]),
            (Evidence::Incompatible, Evidence::Compatible) => Some(self.0[1]),
            (Evidence::Compatible, Evidence::Unknown)
                if has_substantial_language_evidence(self.0[0], text) =>
            {
                Some(self.0[0])
            }
            (Evidence::Unknown, Evidence::Compatible)
                if has_substantial_language_evidence(self.0[1], text) =>
            {
                Some(self.0[1])
            }
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LanguageRoute {
    pub source: SupportedLanguage,
    pub target: SupportedLanguage,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AutoDecision {
    Accept(LanguageRoute),
    Switched {
        route: LanguageRoute,
        active: LanguagePair,
    },
    Retry {
        language: SupportedLanguage,
        candidate: Option<SupportedLanguage>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Evidence {
    Compatible,
    Unknown,
    Incompatible,
}

/// Per-worker adaptive state. The saved user pair remains immutable; only the
/// effective pair used by this live session can change.
#[derive(Debug, Default)]
pub struct AdaptiveLanguageRoute {
    allowed: Option<LanguageSet>,
    configured: Option<LanguagePair>,
    active: Option<LanguagePair>,
    anchor: Option<SupportedLanguage>,
    recent_observations: VecDeque<Option<SupportedLanguage>>,
}

impl AdaptiveLanguageRoute {
    pub fn configure(&mut self, source: &str, targets: &str) {
        let configured = LanguagePair::parse(source, targets);
        if self.configured != configured {
            self.configured = configured;
            self.active = configured;
            self.anchor = None;
            self.recent_observations.clear();
        }
    }

    pub fn active_targets(&self, fallback: &str) -> String {
        self.active.map_or_else(
            || fallback.to_owned(),
            |pair| format!("{},{}", pair.0[0].code(), pair.0[1].code()),
        )
    }

    pub fn is_configured(&self) -> bool {
        self.active.is_some()
    }

    pub fn constrain(&mut self, allowed: LanguageSet) {
        if self.allowed != Some(allowed) {
            self.allowed = Some(allowed);
            self.recent_observations.clear();
            if self
                .active
                .is_some_and(|pair| pair.0.iter().any(|language| !allowed.contains(*language)))
            {
                self.active = self
                    .configured
                    .filter(|pair| pair.0.iter().all(|language| allowed.contains(*language)));
                self.anchor = None;
            }
        }
    }

    pub fn classify(&mut self, detected: Option<&str>, text: &str) -> AutoDecision {
        let pair = self
            .active
            .expect("classify is only used for an automatic pair");
        let allowed = self.allowed.unwrap_or(LanguageSet::ALL);
        let languages = || {
            detected
                .into_iter()
                .flat_map(|label| label.split(','))
                .filter_map(SupportedLanguage::parse)
                .filter(|language| allowed.contains(*language))
        };
        // A model may return an ordered candidate list such as
        // `Chinese,English`. Prefer a candidate whose script is positively
        // supported by the transcript instead of accepting the first merely
        // non-conflicting candidate.
        if let Some(language) = languages().find_map(|language| {
            let matched = pair.find_matching(language)?;
            (script_evidence(matched, text) == Evidence::Compatible).then_some(matched)
        }) {
            self.confirm(language);
            return AutoDecision::Accept(pair.route(language));
        }

        // If the active pair contains Japanese, pure Kanji text without distinct Chinese
        // markers (e.g. "了解", "大丈夫", "乾杯") is Japanese speech. Accept directly as Japanese.
        if let Some(ja) = SupportedLanguage::from_code("ja") {
            if pair.contains(ja)
                && crate::has_substantial_script_evidence(Script::Han, text)
                && !crate::has_distinct_chinese_markers(text)
            {
                self.confirm(ja);
                return AutoDecision::Accept(pair.route(ja));
            }
        }

        let candidate = languages().find(|language| {
            !pair.contains(*language)
                && script_evidence(*language, text) != Evidence::Incompatible
                && has_substantial_candidate_evidence(*language, text)
        });

        if let Some(candidate) = candidate {
            self.push_observation(Some(candidate));
            let count = self
                .recent_observations
                .iter()
                .filter(|observation| **observation == Some(candidate))
                .count();
            if count >= SWITCH_CANDIDATE_THRESHOLD {
                let fallback = if candidate == pair.0[1] {
                    pair.0[0]
                } else {
                    pair.0[1]
                };
                let partner = self
                    .anchor
                    .filter(|language| pair.contains(*language) && *language != candidate)
                    .unwrap_or(fallback);
                let active = LanguagePair([candidate, partner]);
                self.active = Some(active);
                self.anchor = Some(candidate);
                self.recent_observations.clear();
                return AutoDecision::Switched {
                    route: active.route(candidate),
                    active,
                };
            }
        }

        if let Some(language) = pair.language_for_text(text) {
            return AutoDecision::Retry {
                language,
                candidate,
            };
        }
        if let Some(language) = languages().find_map(|language| {
            let matched = pair.find_matching(language)?;
            (script_evidence(matched, text) == Evidence::Unknown).then_some(matched)
        }) {
            self.confirm(language);
            return AutoDecision::Accept(pair.route(language));
        }
        let language = self
            .anchor
            .and_then(|language| pair.find_matching(language))
            .unwrap_or(pair.0[0]);
        AutoDecision::Retry {
            language,
            candidate,
        }
    }

    pub fn recovery(&mut self, forced: SupportedLanguage) -> LanguageRoute {
        let pair = self
            .active
            .expect("recovery is only used for an automatic pair");
        pair.route(forced)
    }

    pub fn evidence(&self, language: SupportedLanguage, text: &str) -> bool {
        script_evidence(language, text) != Evidence::Incompatible
    }

    pub fn alternate(&self, language: SupportedLanguage) -> SupportedLanguage {
        self.active
            .expect("alternate requires an automatic pair")
            .other(language)
    }

    fn push_observation(&mut self, observation: Option<SupportedLanguage>) {
        if self.recent_observations.len() >= RECENT_OBSERVATIONS_LIMIT {
            self.recent_observations.pop_front();
        }
        self.recent_observations.push_back(observation);
    }

    fn confirm(&mut self, language: SupportedLanguage) {
        self.anchor = Some(language);
        self.push_observation(None);
    }
}

fn script_evidence(language: SupportedLanguage, text: &str) -> Evidence {
    let Some(script) = language.script() else {
        return Evidence::Unknown;
    };
    let observed = crate::observed_scripts(text);
    if observed.is_empty() {
        return Evidence::Unknown;
    }
    if script == Script::Han && observed.contains(&Script::Japanese) {
        return Evidence::Incompatible;
    }
    if observed.iter().any(|observed_script| {
        *observed_script == script
            || (script == Script::Japanese && *observed_script == Script::Han)
    }) {
        Evidence::Compatible
    } else if observed.iter().all(|script| *script == Script::Latin) {
        // Latin letters, acronyms, and loanwords (e.g. "S1", "OK", "BGM") are common in Japanese, Chinese, etc.
        // If Latin is the only alphabetic script present, do not treat it as strictly incompatible.
        Evidence::Unknown
    } else {
        Evidence::Incompatible
    }
}

fn has_substantial_language_evidence(language: SupportedLanguage, text: &str) -> bool {
    language
        .script()
        .is_some_and(|script| crate::has_substantial_script_evidence(script, text))
}

fn has_substantial_candidate_evidence(language: SupportedLanguage, text: &str) -> bool {
    let Some(script) = language.script() else {
        return false;
    };
    if script == Script::Latin {
        crate::is_substantial_english_candidate(text)
    } else if script == Script::Han {
        crate::has_distinct_chinese_markers(text)
    } else {
        crate::has_substantial_script_evidence(script, text)
    }
}
