use std::collections::VecDeque;

pub(crate) use xrtranslate_engine::language::Script;

const RECENT_OBSERVATIONS_LIMIT: usize = 5;
const SWITCH_CANDIDATE_THRESHOLD: usize = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LanguageSpec {
    code: &'static str,
    model_name: &'static str,
    script: Script,
}

macro_rules! language {
    ($code:literal, $name:literal, $script:ident) => {
        LanguageSpec {
            code: $code,
            model_name: $name,
            script: Script::$script,
        }
    };
}

const LANGUAGES: &[LanguageSpec] = &[
    language!("af", "Afrikaans", Latin),
    language!("zh", "Chinese", Han),
    language!("zh-TW", "Chinese", Han),
    language!("en", "English", Latin),
    language!("fr", "French", Latin),
    language!("pt", "Portuguese", Latin),
    language!("es", "Spanish", Latin),
    language!("ja", "Japanese", Japanese),
    language!("ru", "Russian", Cyrillic),
    language!("ko", "Korean", Hangul),
    language!("th", "Thai", Thai),
    language!("hi", "Hindi", Devanagari),
    language!("it", "Italian", Latin),
    language!("de", "German", Latin),
    language!("vi", "Vietnamese", Latin),
    language!("id", "Indonesian", Latin),
    language!("pl", "Polish", Latin),
    language!("cs", "Czech", Latin),
    language!("nl", "Dutch", Latin),
    language!("bg", "Bulgarian", Cyrillic),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SupportedLanguage(&'static LanguageSpec);

impl SupportedLanguage {
    pub(crate) fn from_code(code: &str) -> Option<Self> {
        let code = code.trim();
        LANGUAGES
            .iter()
            .find(|language| language.code.eq_ignore_ascii_case(code))
            .map(Self)
            .or_else(|| {
                if code.eq_ignore_ascii_case("zh-hant")
                    || code.eq_ignore_ascii_case("zh-hk")
                    || code.eq_ignore_ascii_case("zh-mo")
                {
                    LANGUAGES.iter().find(|l| l.code == "zh-TW").map(Self)
                } else if code.eq_ignore_ascii_case("zh-hans") || code.eq_ignore_ascii_case("zh-cn")
                {
                    LANGUAGES.iter().find(|l| l.code == "zh").map(Self)
                } else if code.eq_ignore_ascii_case("hin")
                    || code.eq_ignore_ascii_case("hi-in")
                    || code.eq_ignore_ascii_case("hi_in")
                {
                    LANGUAGES.iter().find(|l| l.code == "hi").map(Self)
                } else {
                    let primary = code.split(['-', '_']).next().unwrap_or(code);
                    LANGUAGES
                        .iter()
                        .find(|language| language.code.eq_ignore_ascii_case(primary))
                        .map(Self)
                }
            })
    }

    pub(crate) fn from_model_label(label: &str) -> Option<Self> {
        let label = label.trim();
        if label.eq_ignore_ascii_case("Traditional Chinese")
            || label.eq_ignore_ascii_case("TraditionalChinese")
            || label.eq_ignore_ascii_case("Chinese (Traditional)")
        {
            return LANGUAGES.iter().find(|l| l.code == "zh-TW").map(Self);
        }
        Self::from_code(label).or_else(|| {
            LANGUAGES
                .iter()
                .find(|language| language.model_name.eq_ignore_ascii_case(label))
                .map(Self)
                .or_else(|| {
                    label
                        .eq_ignore_ascii_case("Mandarin")
                        .then(|| Self(&LANGUAGES[1]))
                })
        })
    }

    pub(crate) const fn code(self) -> &'static str {
        self.0.code
    }

    pub(crate) const fn model_name(self) -> &'static str {
        self.0.model_name
    }

    pub(crate) const fn script(self) -> Script {
        self.0.script
    }
}

pub(crate) fn is_traditional_chinese(code: &str) -> bool {
    let code = code.trim();
    code.eq_ignore_ascii_case("zh-tw")
        || code.eq_ignore_ascii_case("zh-hant")
        || code.eq_ignore_ascii_case("zh-hk")
        || code.eq_ignore_ascii_case("zh-mo")
}

pub(crate) fn to_traditional_chinese(text: &str) -> String {
    zhconv::zhconv(text, zhconv::Variant::ZhTW)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LanguagePair(pub(crate) [SupportedLanguage; 2]);

impl LanguagePair {
    pub(crate) fn target_lang(self) -> String {
        format!("{},{}", self.0[0].code(), self.0[1].code())
    }

    fn parse(source: &str, targets: &str) -> Option<Self> {
        if !source.trim().eq_ignore_ascii_case("auto") {
            return None;
        }
        let mut languages = targets.split(',').map(SupportedLanguage::from_code);
        let pair = Self([languages.next()??, languages.next()??]);
        (languages.next().is_none() && pair.0[0] != pair.0[1]).then_some(pair)
    }

    fn find_matching(self, language: SupportedLanguage) -> Option<SupportedLanguage> {
        if self.0[0] == language
            || (self.0[0].model_name() == language.model_name()
                && self.0[0].script() == language.script())
        {
            Some(self.0[0])
        } else if self.0[1] == language
            || (self.0[1].model_name() == language.model_name()
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
pub(crate) struct LanguageRoute {
    pub(crate) source: SupportedLanguage,
    pub(crate) target: SupportedLanguage,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AutoDecision {
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
pub(crate) struct AdaptiveLanguageRoute {
    configured: Option<LanguagePair>,
    active: Option<LanguagePair>,
    anchor: Option<SupportedLanguage>,
    recent_observations: VecDeque<Option<SupportedLanguage>>,
}

impl AdaptiveLanguageRoute {
    pub(crate) fn configure(&mut self, source: &str, targets: &str) {
        let configured = LanguagePair::parse(source, targets);
        if self.configured != configured {
            self.configured = configured;
            self.active = configured;
            self.anchor = None;
            self.recent_observations.clear();
        }
    }

    pub(crate) fn active_targets(&self, fallback: &str) -> String {
        self.active.map_or_else(
            || fallback.to_owned(),
            |pair| format!("{},{}", pair.0[0].code(), pair.0[1].code()),
        )
    }

    pub(crate) fn is_configured(&self) -> bool {
        self.active.is_some()
    }

    pub(crate) fn classify(&mut self, detected: Option<&str>, text: &str) -> AutoDecision {
        let pair = self
            .active
            .expect("classify is only used for an automatic pair");
        let languages = || {
            detected
                .into_iter()
                .flat_map(|label| label.split(','))
                .filter_map(SupportedLanguage::from_model_label)
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

        let candidate = languages().find(|language| {
            !pair.contains(*language)
                && script_evidence(*language, text) != Evidence::Incompatible
                && has_substantial_language_evidence(*language, text)
        });

        if let Some(candidate) = candidate {
            self.push_observation(Some(candidate));
            let count = self
                .recent_observations
                .iter()
                .filter(|observation| **observation == Some(candidate))
                .count();
            if count >= SWITCH_CANDIDATE_THRESHOLD {
                let partner = self
                    .anchor
                    .filter(|language| pair.contains(*language))
                    .unwrap_or(pair.0[1]);
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

    pub(crate) fn recovery(&mut self, forced: SupportedLanguage) -> LanguageRoute {
        let pair = self
            .active
            .expect("recovery is only used for an automatic pair");
        pair.route(forced)
    }

    pub(crate) fn evidence(&self, language: SupportedLanguage, text: &str) -> bool {
        script_evidence(language, text) != Evidence::Incompatible
    }

    pub(crate) fn alternate(&self, language: SupportedLanguage) -> SupportedLanguage {
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
    let observed = xrtranslate_engine::observed_scripts(text);
    if observed.is_empty() {
        return Evidence::Unknown;
    }
    if language.0.script == Script::Han && observed.contains(&Script::Japanese) {
        return Evidence::Incompatible;
    }
    if observed.iter().any(|script| {
        *script == language.0.script
            || (language.0.script == Script::Japanese && *script == Script::Han)
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
    xrtranslate_engine::has_substantial_script_evidence(language.0.script, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn language(code: &str) -> SupportedLanguage {
        SupportedLanguage::from_code(code).unwrap()
    }

    #[test]
    fn all_configured_languages_round_trip() {
        for spec in LANGUAGES {
            assert_eq!(
                SupportedLanguage::from_code(spec.code),
                Some(SupportedLanguage(spec))
            );
            assert!(SupportedLanguage::from_model_label(spec.model_name).is_some());
        }
        assert_eq!(
            SupportedLanguage::from_model_label("Traditional Chinese"),
            SupportedLanguage::from_code("zh-TW")
        );
        assert_eq!(
            SupportedLanguage::from_code("hi-IN"),
            SupportedLanguage::from_code("hi")
        );
        assert_eq!(
            SupportedLanguage::from_code("hin"),
            SupportedLanguage::from_code("hi")
        );
        assert_eq!(
            SupportedLanguage::from_code("vi-VN"),
            SupportedLanguage::from_code("vi")
        );
    }

    #[test]
    fn traditional_chinese_detection_and_conversion() {
        assert!(is_traditional_chinese("zh-tw"));
        assert!(is_traditional_chinese("zh-TW"));
        assert!(is_traditional_chinese("zh-Hant"));
        assert!(!is_traditional_chinese("zh"));
        assert!(!is_traditional_chinese("en"));

        assert_eq!(to_traditional_chinese("设置与翻译"), "設置與翻譯");
    }

    #[test]
    fn one_outside_detection_never_switches() {
        let mut route = AdaptiveLanguageRoute::default();
        route.configure("auto", "ja,en");
        assert!(matches!(
            route.classify(Some("Chinese,English"), "hello"),
            AutoDecision::Accept(LanguageRoute { source, .. }) if source == language("en")
        ));
        assert!(matches!(
            route.classify(Some("Chinese"), "再会"),
            AutoDecision::Retry {
                candidate: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn switches_after_two_outside_observations_within_five_turns() {
        let mut route = AdaptiveLanguageRoute::default();
        route.configure("auto", "ja,en");
        assert!(matches!(
            route.classify(Some("English"), "hello"),
            AutoDecision::Accept(_)
        ));

        // 1. First outside detection -> Retry (forced in-pair for this 1st turn)
        let decision1 = route.classify(Some("Korean"), "안녕하세요");
        assert!(matches!(decision1, AutoDecision::Retry { .. }));
        assert_eq!(route.active_targets(""), "ja,en");

        // 2. Second outside detection in 5 turns -> Directly Accepts & switches to ko,en!
        let decision2 = route.classify(Some("Korean"), "안녕하세요");
        assert!(matches!(
            decision2,
            AutoDecision::Switched { route: LanguageRoute { source, .. }, .. } if source == language("ko")
        ));
        assert_eq!(route.active_targets(""), "ko,en");
    }

    #[test]
    fn hindi_is_accepted_and_can_adapt_an_automatic_pair() {
        let mut configured = AdaptiveLanguageRoute::default();
        configured.configure("auto", "hi,en");
        assert!(matches!(
            configured.classify(Some("Hindi"), "नमस्ते दुनिया"),
            AutoDecision::Accept(LanguageRoute { source, target })
                if source == language("hi") && target == language("en")
        ));

        let mut adaptive = AdaptiveLanguageRoute::default();
        adaptive.configure("auto", "ja,en");
        assert!(matches!(
            adaptive.classify(Some("Hindi"), "नमस्ते दुनिया"),
            AutoDecision::Retry { candidate: Some(candidate), .. }
                if candidate == language("hi")
        ));
        assert!(matches!(
            adaptive.classify(Some("Hindi"), "आप कैसे हैं"),
            AutoDecision::Switched { route: LanguageRoute { source, .. }, .. }
                if source == language("hi")
        ));
        assert_eq!(adaptive.active_targets(""), "hi,en");
    }

    #[test]
    fn interleaved_in_pair_still_switches_if_two_outside_in_five_turns() {
        let mut route = AdaptiveLanguageRoute::default();
        route.configure("auto", "ja,en");

        // 1. Outside candidate: Korean (count = 1) -> Retry
        let decision1 = route.classify(Some("Korean"), "안녕하세요");
        assert!(matches!(decision1, AutoDecision::Retry { .. }));

        // 2. In-pair utterance: English (observation = None)
        assert!(matches!(
            route.classify(Some("English"), "hello world"),
            AutoDecision::Accept(_)
        ));

        // 3. Outside candidate: Korean (count = 2 within recent 3 messages <= 5) -> Accept & Switch!
        let decision2 = route.classify(Some("Korean"), "안녕하세요");
        assert!(matches!(
            decision2,
            AutoDecision::Switched { route: LanguageRoute { source, .. }, .. } if source == language("ko")
        ));
        assert_eq!(route.active_targets(""), "ko,en");
    }

    #[test]
    fn shared_script_also_switches_on_two_observations() {
        let mut route = AdaptiveLanguageRoute::default();
        route.configure("auto", "ja,en");

        // Turn 1: Chinese -> Retry
        let decision1 = route.classify(Some("Chinese"), "再会");
        assert!(matches!(decision1, AutoDecision::Retry { .. }));

        // Turn 2: Chinese -> Accept & Switch
        let decision2 = route.classify(Some("Chinese"), "再会");
        assert!(matches!(
            decision2,
            AutoDecision::Switched { route: LanguageRoute { source, .. }, .. } if source == language("zh")
        ));
        assert_eq!(route.active_targets(""), "zh,en");
    }

    #[test]
    fn outside_observation_expires_after_five_turns() {
        let mut route = AdaptiveLanguageRoute::default();
        route.configure("auto", "ja,en");

        // Turn 1: Korean candidate (1st) -> Retry
        let decision1 = route.classify(Some("Korean"), "안녕하세요");
        assert!(matches!(decision1, AutoDecision::Retry { .. }));

        // Turn 2..=6 (5 in-pair turns): pushes 5 `None`s, expiring the first Korean observation
        for _ in 0..5 {
            assert!(matches!(
                route.classify(Some("English"), "hello"),
                AutoDecision::Accept(_)
            ));
        }

        // Turn 7: Korean candidate again (only 1 in recent 5 turns) -> Retry (does not switch)
        let decision2 = route.classify(Some("Korean"), "안녕하세요");
        assert!(matches!(decision2, AutoDecision::Retry { .. }));
        assert_eq!(route.active_targets(""), "ja,en");
    }

    #[test]
    fn in_pair_detections_keep_configured_pair() {
        let mut route = AdaptiveLanguageRoute::default();
        route.configure("auto", "ja,en");
        for _ in 0..4 {
            assert!(matches!(
                route.classify(Some("Japanese"), "これは日本語です"),
                AutoDecision::Accept(LanguageRoute { source, .. }) if source == language("ja")
            ));
        }
        assert_eq!(route.active_targets(""), "ja,en");
    }

    #[test]
    fn chinese_english_pair_uses_transcript_evidence_over_ambiguous_label_order() {
        let mut route = AdaptiveLanguageRoute::default();
        route.configure("auto", "zh,en");

        assert!(matches!(
            route.classify(Some("Chinese,English"), "Do you play Overwatch?"),
            AutoDecision::Accept(LanguageRoute { source, target })
                if source == language("en") && target == language("zh")
        ));
    }

    #[test]
    fn substantial_english_recovers_without_defaulting_to_first_pair_language() {
        let mut route = AdaptiveLanguageRoute::default();
        route.configure("auto", "zh,en");

        assert!(matches!(
            route.classify(Some("Chinese"), "What's your name?"),
            AutoDecision::Retry { language: source, candidate: None }
                if source == language("en")
        ));

        let mut short = AdaptiveLanguageRoute::default();
        short.configure("auto", "zh,en");
        assert!(matches!(
            short.classify(Some("Chinese"), "OK"),
            AutoDecision::Accept(LanguageRoute { source, target })
                if source == language("zh") && target == language("en")
        ));
    }

    #[test]
    fn reconfiguration_resets_adaptation() {
        let mut route = AdaptiveLanguageRoute::default();
        route.configure("auto", "ja,en");
        route.push_observation(Some(language("zh")));
        assert_eq!(route.recent_observations.len(), 1);
        route.configure("auto", "fr,de");
        assert_eq!(route.active_targets(""), "fr,de");
        assert!(route.recent_observations.is_empty());
    }

    #[test]
    fn latin_noise_and_acronyms_never_switch_configured_pair() {
        let mut route = AdaptiveLanguageRoute::default();
        route.configure("auto", "ja,zh");

        // 1. In-pair Japanese speech
        assert!(matches!(
            route.classify(Some("Japanese"), "おはようございます"),
            AutoDecision::Accept(LanguageRoute { source, target })
                if source == language("ja") && target == language("zh")
        ));

        // 2. Short noise / acronym "S1" detected as English -> should NOT count as candidate
        let decision1 = route.classify(Some("English"), "S1");
        assert!(matches!(
            decision1,
            AutoDecision::Retry {
                candidate: None,
                ..
            }
        ));
        assert_eq!(route.active_targets(""), "ja,zh");

        // 3. Repeated short tokens "OK", "BGM" detected as English -> still NO switch
        let decision2 = route.classify(Some("English"), "OK");
        assert!(matches!(
            decision2,
            AutoDecision::Retry {
                candidate: None,
                ..
            }
        ));
        assert_eq!(route.active_targets(""), "ja,zh");

        let decision3 = route.classify(Some("English"), "BGM");
        assert!(matches!(
            decision3,
            AutoDecision::Retry {
                candidate: None,
                ..
            }
        ));
        assert_eq!(route.active_targets(""), "ja,zh");

        // 4. In-pair Japanese speech containing Latin word "これはS1です"
        assert!(matches!(
            route.classify(Some("Japanese"), "これはS1です"),
            AutoDecision::Accept(LanguageRoute { source, target })
                if source == language("ja") && target == language("zh")
        ));
        assert_eq!(route.active_targets(""), "ja,zh");

        // 5. Pure Latin token "S1" with Japanese detection -> should be accepted via Unknown evidence
        assert!(matches!(
            route.classify(Some("Japanese"), "S1"),
            AutoDecision::Accept(LanguageRoute { source, target })
                if source == language("ja") && target == language("zh")
        ));
        assert_eq!(route.active_targets(""), "ja,zh");
    }

    #[test]
    fn genuine_english_speech_switches_after_two_turns() {
        let mut route = AdaptiveLanguageRoute::default();
        route.configure("auto", "ja,zh");

        // First genuine English utterance -> Retry
        let decision1 = route.classify(Some("English"), "Hello everyone, can you hear me?");
        assert!(matches!(
            decision1,
            AutoDecision::Retry {
                candidate: Some(c),
                ..
            } if c == language("en")
        ));
        assert_eq!(route.active_targets(""), "ja,zh");

        // Second genuine English utterance -> Switched!
        let decision2 = route.classify(Some("English"), "Yes, I am speaking English now.");
        assert!(matches!(
            decision2,
            AutoDecision::Switched {
                route: LanguageRoute { source, .. },
                ..
            } if source == language("en")
        ));
        assert!(route.active_targets("").contains("en"));
    }

    #[test]
    fn invalid_automatic_pair_is_not_configured() {
        let mut route = AdaptiveLanguageRoute::default();
        route.configure("auto", "en,en");
        assert!(!route.is_configured());
    }
}
