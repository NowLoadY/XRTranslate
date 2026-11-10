#[cfg(windows)]
use std::env;

fn main() {
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
