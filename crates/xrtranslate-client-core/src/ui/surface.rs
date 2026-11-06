//! Independent plugin rendering, with a negotiated legacy fallback.
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::Host;

pub fn serve(plugin: impl xrtranslate_plugin::surface::UiPlugin) -> Result<(), String> {
    #[cfg(windows)]
    let renderer = Some(
        Box::new(windows::Page::default()) as Box<dyn xrtranslate_plugin::surface::GpuRenderer>
    );
    #[cfg(not(windows))]
    let renderer = None;
    xrtranslate_plugin::surface::serve_with_renderer(plugin, renderer)
}
