#[cfg(windows)]
use std::env;
#[cfg(windows)]
use std::fs;
#[cfg(windows)]
use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;
#[cfg(windows)]
use std::process::Command;

#[cfg(windows)]
fn find_lib_exe() -> Option<PathBuf> {
    let candidates = [
        r"C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Tools\MSVC",
        r"C:\Program Files\Microsoft Visual Studio\2022\Professional\VC\Tools\MSVC",
        r"C:\Program Files\Microsoft Visual Studio\2022\Enterprise\VC\Tools\MSVC",
        r"C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC",
        r"C:\Program Files (x86)\Microsoft Visual Studio\2019\Community\VC\Tools\MSVC",
        r"C:\Program Files (x86)\Microsoft Visual Studio\2019\Professional\VC\Tools\MSVC",
        r"C:\Program Files (x86)\Microsoft Visual Studio\2019\Enterprise\VC\Tools\MSVC",
        r"C:\Program Files (x86)\Microsoft Visual Studio\2019\BuildTools\VC\Tools\MSVC",
    ];
    for base in &candidates {
        let base_path = Path::new(base);
        if base_path.exists() {
            if let Ok(entries) = fs::read_dir(base_path) {
                for entry in entries.flatten() {
                    let host_x64 = entry
                        .path()
                        .join("bin")
                        .join("Hostx64")
                        .join("x64")
                        .join("lib.exe");
                    if host_x64.exists() {
                        return Some(host_x64);
                    }
                    let host_x86 = entry
                        .path()
                        .join("bin")
                        .join("Hostx86")
                        .join("x64")
                        .join("lib.exe");
                    if host_x86.exists() {
                        return Some(host_x86);
                    }
                }
            }
        }
    }
    None
}

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

            if let Some(lib_exe) = find_lib_exe() {
                let _ = Command::new(lib_exe)
                    .arg(format!("/def:{}", def_path.display()))
                    .arg(format!("/out:{}", lib_path.display()))
                    .arg("/machine:x64")
                    .status();
            }

            println!("cargo:rustc-link-search=native={}", out_dir);
            println!("cargo:rustc-link-arg=/DELAYLOAD:mpv-2.dll");
            println!("cargo:rustc-link-lib=delayimp");
        }
    }
}
