fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var_os("CARGO_FEATURE_MPV").is_some()
    {
        // The host supplies the optional player runtime after process startup.
        println!("cargo:rustc-link-arg=/DELAYLOAD:mpv-2.dll");
        println!("cargo:rustc-link-lib=delayimp");
    }
}
