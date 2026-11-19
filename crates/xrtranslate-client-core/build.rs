use std::{env, fmt::Write, fs, path::PathBuf};

fn index_icons() {
    let dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("resources/icons");
    println!("cargo:rerun-if-changed=resources/icons");
    let mut icons: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "svg"))
        .collect();
    icons.sort();
    let mut source = String::from(
        "#[doc(hidden)] pub const fn icon_bytes(name: &str) -> Option<&'static [u8]> { match name.as_bytes() {\n",
    );
    for path in icons {
        let name = path.file_name().unwrap().to_str().unwrap();
        writeln!(
            source,
            "b{name:?} => Some(include_bytes!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/resources/icons/\", {name:?}))),"
        )
        .unwrap();
    }
    source.push_str("_ => None, } }");
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("sdk_icons.rs"),
        source,
    )
    .unwrap();
}

fn main() {
    index_icons();
    println!("cargo:rerun-if-changed=resources/mpv.def");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var_os("CARGO_FEATURE_MPV").is_some()
    {
        let out_dir = std::env::var("OUT_DIR").unwrap();
        let library = std::path::Path::new(&out_dir).join("mpv.lib");
        let target = std::env::var("TARGET").expect("Cargo target is required");
        let status = find_msvc_tools::find(&target, "lib.exe")
            .expect("Visual Studio C++ tools are required")
            .arg("/def:resources/mpv.def")
            .arg(format!("/out:{}", library.display()))
            .arg("/machine:x64")
            .status()
            .expect("cannot run the MSVC librarian");
        assert!(status.success(), "cannot generate the MPV import library");
        // Link search paths propagate to every consumer of this shared backend.
        println!("cargo:rustc-link-search=native={out_dir}");
    }
}
