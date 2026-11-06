//! Video stays in the host playback service. Leave a transparent aperture at its
//! original draw position, so subtitles and controls can still paint above it.
use super::FORMAT;
use eframe::{
    egui,
    egui_wgpu::{self, CallbackResources, CallbackTrait},
    wgpu,
};
use std::sync::Arc;
use xrtranslate_plugin::surface::Drawing;

struct Aperture(wgpu::RenderPipeline);
struct Clear;

pub fn install(device: &wgpu::Device, renderer: &mut egui_wgpu::Renderer) {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("video aperture"),
        source: wgpu::ShaderSource::Wgsl(
            r#"
            @vertex fn vertex(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
                let p = array(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
                return vec4(p[i], 0.0, 1.0);
            }
            @fragment fn fragment() -> @location(0) vec4<f32> { return vec4(0.0); }
        "#
            .into(),
        ),
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("video aperture"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vertex"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fragment"),
            targets: &[Some(FORMAT.into())],
            compilation_options: Default::default(),
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    });
    renderer.callback_resources.insert(Aperture(pipeline));
}
impl CallbackTrait for Clear {
    fn paint(
        &self,
        _: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &CallbackResources,
    ) {
        pass.set_pipeline(&resources.get::<Aperture>().unwrap().0);
        pass.draw(0..3, 0..1);
    }
}
pub fn extract(primitives: &mut [egui::ClippedPrimitive]) -> Vec<Drawing> {
    let mut external = Vec::new();
    for item in primitives {
        if let egui::epaint::Primitive::Mesh(mesh) = &item.primitive
            && matches!(mesh.texture_id, egui::TextureId::User(_))
        {
            let rect = mesh.calc_bounds();
            external.push(Drawing::Mesh {
                clip: item.clip_rect,
                mesh: Arc::new(mesh.clone()),
            });
            let callback = egui_wgpu::Callback::new_paint_callback(rect, Clear);
            item.primitive = egui::epaint::Primitive::Callback(callback);
        }
    }
    external
}
