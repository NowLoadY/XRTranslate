#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() -> eframe::Result<()> {
    #[cfg(windows)]
    if std::env::var_os("XRTRANSLATE_BACKEND_CONSOLE_LOG").is_some() {
        use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};

        // The development launcher also needs console access in GUI release builds.
        unsafe {
            let _ = AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
    rust_client::run()
}
