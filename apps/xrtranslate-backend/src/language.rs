pub(crate) use xrtranslate_engine::language::{
    AdaptiveLanguageRoute, AutoDecision, SupportedLanguage, is_traditional_chinese,
};

pub(crate) fn to_traditional_chinese(text: &str) -> String {
    zhconv::zhconv(text, zhconv::Variant::ZhTW)
}
