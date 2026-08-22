//! Pure language detection and adaptive route classification.
//!
//! Deliberately owns no network, audio, or persistent state. Provides fast,
//! zero-allocation script analysis and pair-aware language auto-routing.

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
    let detected = detect_text_language(text)?;
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

    if src == "auto" {
        if let Some(second) = pair_target.as_deref() {
            if is_language_code_match(detected, &tgt) {
                return Some((static_code(&tgt), static_code(second)));
            }
            if is_language_code_match(detected, second) {
                return Some((static_code(second), static_code(&tgt)));
            }
            return Some((detected, static_code(&tgt)));
        }
    }

    // If already matching source, no swap needed.
    if is_language_code_match(detected, &src) {
        return None;
    }

    // If matches target, flip the pair!
    if is_language_code_match(detected, &tgt) {
        return Some((static_code(&tgt), static_code(&src)));
    }

    // If detected is a distinct third language, route detected -> target (or source if target conflicts).
    let next_target = if is_language_code_match(detected, &tgt) {
        static_code(&src)
    } else {
        static_code(&tgt)
    };

    Some((detected, next_target))
}

fn is_language_code_match(detected: &str, code: &str) -> bool {
    if detected.eq_ignore_ascii_case(code) {
        return true;
    }
    let primary = code.split(['-', '_']).next().unwrap_or(code);
    if detected.eq_ignore_ascii_case(primary)
        || (detected.eq_ignore_ascii_case("hi") && primary.eq_ignore_ascii_case("hin"))
    {
        return true;
    }
    if detected == "zh" && (code.starts_with("zh") || code == "zh-tw" || code == "zh-cn") {
        return true;
    }
    false
}

fn static_code(code: &str) -> &'static str {
    let normalized = code.trim().to_ascii_lowercase().replace('_', "-");
    match normalized.as_str() {
        "zh-tw" | "zh-hant" | "zh-hk" | "zh-mo" => return "zh-TW",
        "zh" | "zh-cn" | "zh-hans" => return "zh",
        _ => {}
    }
    let primary = normalized.split('-').next().unwrap_or_default();
    match primary {
        "en" => "en",
        "ja" => "ja",
        "ko" => "ko",
        "ru" => "ru",
        "th" => "th",
        "hi" | "hin" => "hi",
        "fr" => "fr",
        "de" => "de",
        "es" => "es",
        "pt" => "pt",
        "it" => "it",
        "vi" => "vi",
        "id" => "id",
        "pl" => "pl",
        "cs" => "cs",
        "nl" => "nl",
        "af" => "af",
        _ => "en",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_text_language() {
        assert_eq!(detect_text_language("你好，世界！"), Some("zh"));
        assert_eq!(detect_text_language("Hello, world!"), Some("en"));
        assert_eq!(detect_text_language("こんにちは世界"), Some("ja"));
        assert_eq!(detect_text_language("안녕하세요"), Some("ko"));
        assert_eq!(detect_text_language("Привет мир"), Some("ru"));
        assert_eq!(detect_text_language("สวัสดีชาวโลก"), Some("th"));
        assert_eq!(detect_text_language("स्वागत है"), Some("hi"));
        assert_eq!(detect_text_language("12345"), None);
        assert_eq!(detect_text_language("   "), None);
        assert_eq!(detect_text_language("ok"), None); // Short acronym ignored as substantial
    }

    #[test]
    fn test_auto_route_language_pair() {
        assert_eq!(static_code("vi-VN"), "vi");
        assert_eq!(static_code("hi_IN"), "hi");

        // When typing English on a zh -> en pair, it should flip to en -> zh
        assert_eq!(
            auto_route_language_pair("How are you today?", "zh", "en"),
            Some(("en", "zh"))
        );

        // Automatic bidirectional routes carry both languages in the target
        // field, but the selected route must contain only the opposite
        // language as its target.
        assert_eq!(
            auto_route_language_pair("How are you today?", "auto", "zh,en"),
            Some(("en", "zh"))
        );
        assert_eq!(
            auto_route_language_pair("今天过得怎么样？", "auto", "zh,en"),
            Some(("zh", "en"))
        );

        // When typing Chinese on an en -> zh pair, it should flip to zh -> en
        assert_eq!(
            auto_route_language_pair("今天过得怎么样？", "en", "zh"),
            Some(("zh", "en"))
        );

        // When typing Chinese on a zh -> en pair, no change is needed
        assert_eq!(auto_route_language_pair("你好世界", "zh", "en"), None);

        // Devanagari provides an unambiguous route for Hindi.
        assert_eq!(
            auto_route_language_pair("नमस्ते दुनिया", "en", "hi-IN"),
            Some(("hi", "en"))
        );
        assert_eq!(
            auto_route_language_pair("नमस्ते दुनिया", "en", "hin"),
            Some(("hi", "en"))
        );

        // When typing Japanese on a zh -> en pair, routes ja -> en
        assert_eq!(
            auto_route_language_pair("こんにちは", "zh", "en"),
            Some(("ja", "en"))
        );
    }
}
