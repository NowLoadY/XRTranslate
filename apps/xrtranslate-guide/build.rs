// Project the existing UI dictionary at build time; the player carries only labels
// used by its lessons, without linking the client's audio/model/UI implementation.
#[path = "../../crates/xrtranslate-client-core/src/i18n/strings.rs"]
mod strings;

fn main() {
    #[cfg(windows)]
    {
        let icon = "../../rust-client/resources/branding/xrtranslate-logo.ico";
        println!("cargo:rerun-if-changed={icon}");
        winres::WindowsResource::new()
            .set_icon(icon)
            .set("ProductName", "XRTranslate Guide")
            .compile()
            .unwrap();
    }
    let source = "../../plugin-src/guide/assets/lessons.json";
    println!("cargo:rerun-if-changed={source}");
    println!("cargo:rerun-if-changed=../../crates/xrtranslate-client-core/src/i18n/strings.rs");
    let lessons: serde_json::Value =
        serde_json::from_slice(&std::fs::read(source).unwrap()).unwrap();
    let mut keys = std::collections::BTreeSet::from([
        "Translation",
        "User Guide",
        "Previous feature",
        "Next feature",
        "Exit guide",
        "Back to styles",
        "Edit prompts",
        "Source language",
        "Target language",
        "Settings",
        "Retry",
        "Audio processing",
    ]);
    for page in lessons.as_array().unwrap() {
        for step in page["steps"].as_array().unwrap() {
            for field in ["targets", "reveal"] {
                keys.extend(
                    step[field]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter_map(|s| s.as_str()),
                );
            }
        }
    }
    let labels: std::collections::BTreeMap<_, _> = keys
        .into_iter()
        .map(|key| {
            let values = strings::DICTIONARY
                .iter()
                .find(|entry| entry.0 == key)
                .map_or([key; 5], |&(en, zh, ja, ko, ru)| [en, zh, ja, ko, ru]);
            (key, values)
        })
        .collect();
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    std::fs::write(
        out.join("labels.json"),
        serde_json::to_vec(&labels).unwrap(),
    )
    .unwrap();
}
