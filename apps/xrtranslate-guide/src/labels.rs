use std::{collections::BTreeMap, sync::OnceLock};

pub const LANGUAGES: [&str; 5] = ["en", "zh", "ja", "ko", "ru"];

fn dictionary() -> &'static BTreeMap<String, [String; 5]> {
    static LABELS: OnceLock<BTreeMap<String, [String; 5]>> = OnceLock::new();
    LABELS.get_or_init(|| {
        serde_json::from_str(include_str!(concat!(env!("OUT_DIR"), "/labels.json"))).unwrap()
    })
}

pub fn text<'a>(key: &'a str, language: &str) -> &'a str {
    let index = LANGUAGES.iter().position(|&s| s == language).unwrap_or(0);
    dictionary()
        .get(key)
        .map_or(key, |values| values[index].as_str())
}

pub fn matches(label: &str, key: &str) -> bool {
    let normalize = |s: &str| s.trim().trim_end_matches([':', '：']).to_lowercase();
    let label = normalize(label);
    label == normalize(key)
        || dictionary()
            .get(key)
            .is_some_and(|values| values.iter().any(|value| normalize(value) == label))
}

pub fn language(translation_label: &str) -> &'static str {
    dictionary()
        .get("Translation")
        .and_then(|values| values.iter().position(|s| s == translation_label))
        .map_or("en", |index| LANGUAGES[index])
}

pub fn system_language() -> &'static str {
    #[cfg(windows)]
    {
        unsafe extern "system" {
            fn GetUserDefaultLocaleName(name: *mut u16, count: i32) -> i32;
        }
        let mut buffer = [0; 85];
        let count = unsafe { GetUserDefaultLocaleName(buffer.as_mut_ptr(), 85) };
        if count > 1 {
            let locale = String::from_utf16_lossy(&buffer[..count as usize - 1]);
            return LANGUAGES
                .into_iter()
                .find(|lang| locale.starts_with(lang))
                .unwrap_or("en");
        }
    }
    "en"
}

pub fn local<'a>(
    language: &str,
    en: &'a str,
    zh: &'a str,
    ja: &'a str,
    ko: &'a str,
    ru: &'a str,
) -> &'a str {
    match language {
        "zh" => zh,
        "ja" => ja,
        "ko" => ko,
        "ru" => ru,
        _ => en,
    }
}
