use std::env;

fn companion_audio() {
    use std::{collections::BTreeSet, fs, path::PathBuf};
    println!("cargo:rerun-if-changed=resources/companion");
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("resources/companion")
        .canonicalize()
        .expect("Missing companion audio");
    let manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(root.join("manifest.json")).expect("Missing companion audio manifest"),
    )
    .unwrap();
    let clips = manifest["clips"].as_array().unwrap();
    let mut generated = String::from("static CLIPS: &[Clip] = &[\n");
    let mut seen = BTreeSet::new();
    for clip in clips {
        let language = match clip["language"].as_str().unwrap() {
            "en" => "English",
            "zh" => "Chinese",
            "ja" => "Japanese",
            "ko" => "Korean",
            "ru" => "Russian",
            _ => panic!("Unknown companion language"),
        };
        let key = clips
            .iter()
            .find(|entry| {
                entry["page"] == clip["page"]
                    && entry["topic"] == clip["topic"]
                    && entry["language"] == "en"
            })
            .expect("Companion audio is missing its source text")["text"]
            .as_str()
            .unwrap();
        assert!(seen.insert((key, language)), "Duplicate companion audio");
        let path = root
            .join(clip["file"].as_str().unwrap())
            .canonicalize()
            .expect("Missing companion clip");
        assert!(
            path.starts_with(&root),
            "Companion clip is outside its asset folder"
        );
        let duration = clip["duration_ms"]
            .as_u64()
            .filter(|ms| *ms > 0)
            .expect("Missing companion duration");
        generated.push_str(&format!(
            "Clip {{ key: {key:?}, language: UiLanguage::{language}, text: {:?}, duration_ms: {duration}, bytes: include_bytes!({:?}) }},\n",
            clip["text"].as_str().unwrap(), path.to_str().unwrap()
        ));
    }
    generated.push_str("];\n");
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("companion_audio.rs"),
        generated,
    )
    .unwrap();
}

fn main() {
    companion_audio();
    println!("cargo:rerun-if-changed=resources/branding/xrtranslate-logo.ico");
    println!("cargo:rerun-if-changed=build.rs");

    #[cfg(windows)]
    {
        if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
            return;
        }
        let mut resource = winres::WindowsResource::new();
        resource.set_icon("resources/branding/xrtranslate-logo.ico");
        resource.set("FileDescription", "XRTranslate");
        resource.set("ProductName", "XRTranslate");
        resource
            .compile()
            .expect("cannot embed XRTranslate application icon");

        if env::var_os("CARGO_FEATURE_MPV").is_some() {
            println!("cargo:rustc-link-arg=/DELAYLOAD:mpv-2.dll");
            println!("cargo:rustc-link-lib=delayimp");
        }
    }
}
