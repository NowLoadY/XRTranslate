use serde_json::Value;
use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=assets/lessons.json");
    println!("cargo:rerun-if-changed=assets/audio");
    let assets = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("assets");
    let audio_root = assets
        .join("audio")
        .canonicalize()
        .expect("Missing guide audio");
    let lessons: Vec<Value> =
        serde_json::from_slice(&fs::read(assets.join("lessons.json")).unwrap()).unwrap();
    let manifest: Value = serde_json::from_slice(
        &fs::read(assets.join("audio/manifest.json"))
            .expect("Generate guide narration first; see guide/README.md"),
    )
    .unwrap();
    let clips = manifest["clips"].as_array().unwrap();
    let mut generated = String::from("static CLIPS: &[Clip] = &[\n");
    for page in &lessons {
        for step in page["steps"].as_array().unwrap() {
            for language in ["en", "zh", "ja", "ko", "ru"] {
                let clip = clips
                    .iter()
                    .find(|clip| {
                        clip["page"] == page["page"]
                            && clip["topic"] == step["id"]
                            && clip["language"] == language
                            && clip["text"] == step["text"][language]
                    })
                    .unwrap_or_else(|| {
                        panic!(
                            "Missing or stale narration: {} / {} / {language}",
                            page["page"], step["id"]
                        )
                    });
                let file = clip["file"].as_str().unwrap();
                let path = audio_root
                    .join(file)
                    .canonicalize()
                    .expect("Missing narration file");
                let relative = path
                    .strip_prefix(&audio_root)
                    .expect("Narration is outside its asset folder");
                let duration = clip["duration_ms"]
                    .as_u64()
                    .filter(|ms| *ms > 0)
                    .expect("Missing audio duration");
                generated.push_str(&format!(
                    "Clip {{ page: {:?}, topic: {:?}, language: {:?}, text: {:?}, file: {:?}, duration_ms: {duration}, bytes: include_bytes!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/assets/audio/\", {relative:?})) }},\n",
                    page["page"].as_str().unwrap(), step["id"].as_str().unwrap(), language,
                    step["text"][language].as_str().unwrap(), file,
                ));
            }
        }
    }
    generated.push_str("];\n");
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("audio.rs"),
        generated,
    )
    .unwrap();
}
