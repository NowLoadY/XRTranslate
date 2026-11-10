//! Embedded pages share the host GPU on every platform. Plugins own layout and
//! interaction; named callbacks carry parameters for the host's shared shaders.
pub use xrtranslate_plugin::surface::serve;

pub fn paint(
    plugin: &str,
    paint: &xrtranslate_plugin::surface::Paint,
) -> Option<eframe::egui::Shape> {
    super::organic_border::surface_paint(plugin, paint)
        .or_else(|| super::components::avatar::surface_paint(plugin, paint))
}
