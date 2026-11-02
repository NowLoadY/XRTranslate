#[cfg(windows)]
use std::env;
#[cfg(windows)]
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=resources/branding/xrtranslate-logo.ico");
    println!("cargo:rerun-if-changed=resources/mpv.def");
    println!("cargo:rerun-if-changed=build.rs");

    #[cfg(windows)]
    {
        if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
            return;
        }
        let mut resource = winres::WindowsResource::new();
        resource.set_icon("resources/branding/xrtranslate-logo.ico");
        resource
            .compile()
            .expect("cannot embed XRTranslate application icon");

        let out_dir = env::var("OUT_DIR").unwrap();
        let out_path = Path::new(&out_dir);
        if env::var_os("CARGO_FEATURE_MPV").is_some() {
            let def_path = Path::new("resources/mpv.def");
            let lib_path = out_path.join("mpv.lib");

            let target = env::var("TARGET").expect("Cargo target is required");
            let status = find_msvc_tools::find(&target, "lib.exe")
                .expect("Visual Studio C++ tools are required")
                .arg(format!("/def:{}", def_path.display()))
                .arg(format!("/out:{}", lib_path.display()))
                .arg("/machine:x64")
                .status()
                .expect("cannot run the MSVC librarian");
            assert!(status.success(), "cannot generate the MPV import library");

            println!("cargo:rustc-link-search=native={}", out_dir);
            println!("cargo:rustc-link-arg=/DELAYLOAD:mpv-2.dll");
            println!("cargo:rustc-link-lib=delayimp");
        }
    }
}
