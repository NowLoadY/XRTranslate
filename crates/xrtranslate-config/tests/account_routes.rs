use serde_json::json;
use xrtranslate_config::AppConfig;

fn configuration() -> serde_json::Value {
    let mut value: serde_json::Value =
        serde_json::from_str(include_str!("../../../config.json")).unwrap();
    value["asr"]["provider"] = json!("Doubao Account");
    value
}

#[test]
fn unknown_profiles_and_unsupported_capabilities_fail_validation() {
    for field in [
        json!({"model":"unverified-model"}),
        json!({"capabilities":{"asr_language_hint":true}}),
        json!({"capabilities":{"vocabulary_bias":true}}),
    ] {
        let mut value = configuration();
        value["asr"]["providers"]["Doubao Account"] = field;
        let config = AppConfig::from_value(value).unwrap();
        assert!(config.native_model_route().is_err());
    }
}
