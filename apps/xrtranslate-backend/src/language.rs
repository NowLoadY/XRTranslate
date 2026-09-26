pub(crate) use xrtranslate_engine::language::{
    AdaptiveLanguageRoute, AutoDecision, SupportedLanguage, is_traditional_chinese,
};

pub(crate) fn to_traditional_chinese(text: &str) -> String {
    zhconv::zhconv(text, zhconv::Variant::ZhTW)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn traditional_chinese_detection_and_conversion() {
        assert!(is_traditional_chinese("zh-tw"));
        assert!(is_traditional_chinese("zh-TW"));
        assert!(is_traditional_chinese("zh-Hant"));
        assert!(!is_traditional_chinese("zh"));
        assert!(!is_traditional_chinese("en"));

        assert_eq!(to_traditional_chinese("设置与翻译"), "設置與翻譯");
    }
}
