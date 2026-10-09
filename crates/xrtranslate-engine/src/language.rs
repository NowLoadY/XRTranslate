//! Pure language detection and adaptive route classification.
//!
//! Deliberately owns no network, audio, or persistent state. Provides fast,
//! zero-allocation script analysis and pair-aware language auto-routing.

mod catalog;
pub use catalog::{LANGUAGE_OPTIONS, LANGUAGES, SupportedLanguage, is_traditional_chinese};
mod adaptive;
pub use adaptive::{AdaptiveLanguageRoute, AutoDecision, LanguagePair, LanguageRoute};
mod selection;
pub use selection::{LanguageCapabilities, LanguageMode, LanguageSelection, LanguageSet};

/// Unicode scripts recognized for rapid language classification.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Script {
    Latin,
    Han,
    Japanese,
    Cyrillic,
    Hangul,
    Thai,
    Devanagari,
}

/// Identifies all Unicode scripts present in the text.
pub fn observed_scripts(text: &str) -> Vec<Script> {
    let mut scripts = Vec::with_capacity(2);
    for character in text.chars() {
        let script =
            if character.is_ascii_alphabetic() || ('\u{00c0}'..='\u{024f}').contains(&character) {
                Some(Script::Latin)
            } else if ('\u{3040}'..='\u{31ff}').contains(&character) {
                Some(Script::Japanese)
            } else if ('\u{3400}'..='\u{9fff}').contains(&character) {
                Some(Script::Han)
            } else if ('\u{0400}'..='\u{04ff}').contains(&character) {
                Some(Script::Cyrillic)
            } else if ('\u{1100}'..='\u{11ff}').contains(&character)
                || ('\u{ac00}'..='\u{d7af}').contains(&character)
            {
                Some(Script::Hangul)
            } else if ('\u{0e00}'..='\u{0e7f}').contains(&character) {
                Some(Script::Thai)
            } else if ('\u{0900}'..='\u{097f}').contains(&character)
                || ('\u{a8e0}'..='\u{a8ff}').contains(&character)
            {
                Some(Script::Devanagari)
            } else {
                None
            };
        if let Some(script) = script {
            if !scripts.contains(&script) {
                scripts.push(script);
            }
        }
    }
    scripts
}

/// Determines whether text contains substantial evidence of a script.
pub fn has_substantial_script_evidence(script: Script, text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }
    match script {
        Script::Latin => {
            let words: Vec<&str> = trimmed
                .split_whitespace()
                .filter(|w| w.chars().any(|c| c.is_ascii_alphabetic()))
                .collect();
            if words.is_empty() {
                return false;
            }
            let total_alpha: usize = trimmed.chars().filter(|c| c.is_ascii_alphabetic()).count();
            if total_alpha < 3 {
                return false;
            }
            if words.len() == 1 {
                let word = words[0];
                let is_all_upper_or_digit = word.chars().all(|c| {
                    c.is_ascii_uppercase() || c.is_ascii_digit() || !c.is_ascii_alphanumeric()
                });
                if is_all_upper_or_digit && word.len() <= 3 {
                    return false;
                }
            }
            true
        }
        Script::Han => {
            let has_han = trimmed
                .chars()
                .any(|c| ('\u{3400}'..='\u{9fff}').contains(&c));
            let has_kana = trimmed
                .chars()
                .any(|c| ('\u{3040}'..='\u{31ff}').contains(&c));
            has_han && !has_kana
        }
        Script::Japanese => trimmed.chars().any(|c| {
            ('\u{3040}'..='\u{31ff}').contains(&c) || ('\u{3400}'..='\u{9fff}').contains(&c)
        }),
        Script::Hangul => trimmed.chars().any(|c| {
            ('\u{1100}'..='\u{11ff}').contains(&c) || ('\u{ac00}'..='\u{d7af}').contains(&c)
        }),
        Script::Cyrillic => trimmed
            .chars()
            .any(|c| ('\u{0400}'..='\u{04ff}').contains(&c)),
        Script::Thai => trimmed
            .chars()
            .any(|c| ('\u{0e00}'..='\u{0e7f}').contains(&c)),
        Script::Devanagari => trimmed.chars().any(|c| {
            ('\u{0900}'..='\u{097f}').contains(&c) || ('\u{a8e0}'..='\u{a8ff}').contains(&c)
        }),
    }
}

/// Determines whether text contains distinct Chinese linguistic markers
/// (unique characters, grammatical particles, or common Chinese phrases)
/// that distinguish Chinese from Japanese Kanji. Zero heap allocations.
pub fn has_distinct_chinese_markers(text: &str) -> bool {
    let has_char_marker = text.chars().any(|c| {
        matches!(
            c,
            '你' | '妳'
                | '她'
                | '它'
                | '牠'
                | '祂'
                | '们'
                | '們'
                | '这'
                | '這'
                | '哪'
                | '什'
                | '么'
                | '麼'
                | '谁'
                | '誰'
                | '吗'
                | '嗎'
                | '呢'
                | '吧'
                | '呀'
                | '很'
                | '没'
                | '說'
                | '话'
                | '話'
                | '请'
                | '請'
                | '让'
                | '進'
                | '进'
                | '门'
                | '关'
                | '开'
                | '车'
                | '钱'
                | '东'
                | '认'
                | '识'
                | '欢'
                | '变'
                | '电'
                | '风'
                | '问'
                | '买'
                | '卖'
                | '头'
                | '过'
                | '对'
                | '为'
                | '样'
                | '个'
                | '谢'
                | '謝'
        )
    });
    if has_char_marker {
        return true;
    }

    const PHRASES: &[&str] = &[
        "不用",
        "可以",
        "好的",
        "真的",
        "不知道",
        "早上好",
        "晚上好",
        "再见",
        "再見",
    ];
    PHRASES.iter().any(|phrase| text.contains(phrase))
}

/// Determines if text represents genuine, substantial English candidate speech
/// rather than isolated gaming loanwords, short acknowledgments, or noise hallucinations.
/// Single-pass, zero heap allocations.
pub fn is_substantial_english_candidate(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }

    let mut total_words = 0usize;
    let mut non_loanword_count = 0usize;
    let mut has_function_word = false;

    for raw in trimmed.split_whitespace() {
        let word = raw.trim_matches(|c: char| !c.is_alphabetic());
        if word.is_empty() {
            continue;
        }
        total_words += 1;

        if is_english_function_word(word) {
            has_function_word = true;
        }
        if !is_common_loanword(word) {
            non_loanword_count += 1;
        }
    }

    if total_words < 2 || non_loanword_count < 2 {
        return false;
    }

    let total_letters: usize = trimmed.chars().filter(|c| c.is_ascii_alphabetic()).count();
    has_function_word || (total_words >= 3 && total_letters >= 16)
}

fn is_common_loanword(word: &str) -> bool {
    // Check against common short loanwords without allocating strings
    matches!(
        word.to_ascii_lowercase().as_str(),
        "ok" | "okay"
            | "nice"
            | "gg"
            | "good"
            | "great"
            | "vrc"
            | "vrchat"
            | "hi"
            | "hello"
            | "hey"
            | "yes"
            | "yeah"
            | "yep"
            | "no"
            | "nope"
            | "sorry"
            | "thx"
            | "thanks"
            | "thank"
            | "you"
            | "lol"
            | "kusa"
            | "w"
            | "39"
            | "bye"
            | "cool"
            | "super"
            | "omg"
            | "wow"
            | "pls"
            | "please"
            | "subtitles"
            | "watching"
            | "subscribe"
            | "amara"
            | "mbc"
    )
}

fn is_english_function_word(word: &str) -> bool {
    matches!(
        word.to_ascii_lowercase().as_str(),
        "the"
            | "is"
            | "are"
            | "was"
            | "were"
            | "am"
            | "be"
            | "been"
            | "being"
            | "have"
            | "has"
            | "had"
            | "do"
            | "does"
            | "did"
            | "would"
            | "should"
            | "could"
            | "will"
            | "can"
            | "this"
            | "that"
            | "these"
            | "those"
            | "what"
            | "where"
            | "when"
            | "which"
            | "who"
            | "how"
            | "why"
            | "with"
            | "from"
            | "about"
            | "into"
            | "because"
            | "they"
            | "them"
            | "their"
            | "your"
            | "ours"
            | "we"
            | "i"
            | "it"
            | "my"
            | "me"
    )
}

/// Detects the most likely primary language code for input text.
pub fn detect_text_language(text: &str) -> Option<&'static str> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Check for Kana first because Japanese texts often contain Kanji (Han) characters.
    if trimmed
        .chars()
        .any(|c| ('\u{3040}'..='\u{31ff}').contains(&c))
    {
        return Some("ja");
    }

    // Check for Hangul (Korean).
    if trimmed
        .chars()
        .any(|c| ('\u{1100}'..='\u{11ff}').contains(&c) || ('\u{ac00}'..='\u{d7af}').contains(&c))
    {
        return Some("ko");
    }

    // Check for Thai.
    if trimmed
        .chars()
        .any(|c| ('\u{0e00}'..='\u{0e7f}').contains(&c))
    {
        return Some("th");
    }

    // Check for Devanagari (Hindi).
    if trimmed
        .chars()
        .any(|c| ('\u{0900}'..='\u{097f}').contains(&c) || ('\u{a8e0}'..='\u{a8ff}').contains(&c))
    {
        return Some("hi");
    }

    // Check for Cyrillic (Russian).
    if trimmed
        .chars()
        .any(|c| ('\u{0400}'..='\u{04ff}').contains(&c))
    {
        return Some("ru");
    }

    // Check for Han characters (Chinese).
    if trimmed
        .chars()
        .any(|c| ('\u{3400}'..='\u{9fff}').contains(&c))
    {
        return Some("zh");
    }

    // Check for Latin script (English / European).
    if has_substantial_script_evidence(Script::Latin, text) {
        return Some("en");
    }

    None
}

/// Automatically adjusts the language pair based on detected input language.
///
/// If the text clearly matches the current target language (e.g. current is `("zh", "en")`
/// and text is English), the route is inverted to `("en", "zh")`.
/// If the text matches a different recognized language (e.g. Japanese), it sets `new_source`
/// to `"ja"` while retaining the user's primary anchor language as target.
pub fn auto_route_language_pair(
    text: &str,
    current_source: &str,
    current_target: &str,
) -> Option<(&'static str, &'static str)> {
    let mut detected = detect_text_language(text)?;
    let src = current_source.trim().to_ascii_lowercase();
    let target_parts = current_target
        .split(',')
        .map(str::trim)
        .filter(|code| !code.is_empty())
        .collect::<Vec<_>>();
    // Automatic bidirectional mode stores both languages in the target field
    // (for example `zh,en`).  Route against the individual pair members;
    // passing the comma-separated value through as a language code makes the
    // text path unable to select the opposite direction.
    let (tgt, pair_target) = match target_parts.as_slice() {
        [first, second, ..] => (
            first.to_ascii_lowercase(),
            Some(second.to_ascii_lowercase()),
        ),
        [first] => (first.to_ascii_lowercase(), None),
        [] => (String::new(), None),
    };

    // If active session involves Japanese and text is Han characters without Chinese markers,
    // it's Japanese Kanji, not Chinese!
    if (src == "ja" || tgt == "ja" || pair_target.as_deref() == Some("ja"))
        && detected == "zh"
        && !has_distinct_chinese_markers(text)
    {
        detected = "ja";
    }

    // Do not route to English if the text is merely gaming loanwords / acronyms
    if detected == "en"
        && src != "en"
        && tgt != "en"
        && pair_target.as_deref() != Some("en")
        && !is_substantial_english_candidate(text)
    {
        return None;
    }

    if src == "auto" {
        if pair_target.is_none() {
            return Some((detected, static_code(&tgt)?));
        }
        if let Some(second) = pair_target.as_deref() {
            if is_language_code_match(detected, &tgt) {
                return Some((static_code(&tgt)?, static_code(second)?));
            }
            if is_language_code_match(detected, second) {
                return Some((static_code(second)?, static_code(&tgt)?));
            }
            return Some((detected, static_code(&tgt)?));
        }
    }

    // If already matching source, no swap needed.
    if is_language_code_match(detected, &src) {
        return None;
    }

    // If matches target, flip the pair!
    if is_language_code_match(detected, &tgt) {
        return Some((static_code(&tgt)?, static_code(&src)?));
    }

    // If detected is a distinct third language, route detected -> target (or source if target conflicts).
    let next_target = if is_language_code_match(detected, &tgt) {
        static_code(&src)?
    } else {
        static_code(&tgt)?
    };

    Some((detected, next_target))
}

fn is_language_code_match(detected: &str, code: &str) -> bool {
    match (
        SupportedLanguage::from_code(detected),
        SupportedLanguage::from_code(code),
    ) {
        (Some(a), Some(b)) => a.base_code() == b.base_code(),
        _ => false,
    }
}

fn static_code(code: &str) -> Option<&'static str> {
    SupportedLanguage::from_code(code).map(SupportedLanguage::code)
}
