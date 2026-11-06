fn main() {
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
