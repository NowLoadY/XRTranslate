use std::{borrow::Cow, collections::HashMap, num::NonZeroU64};

use bytemuck::{Pod, Zeroable};
use eframe::{
    egui::{self, Color32, LayerId, Margin, Rect},
    egui_wgpu::{self, CallbackResources, CallbackTrait, ScreenDescriptor},
    wgpu,
};

const CALLBACK_MARGIN: f32 = 5.0;
const ORGANIC_BORDER_WIDTH: f32 = 1.5;
const LAYOUT_GUTTER: i8 = 3;

#[derive(Clone, Copy, Debug)]
pub struct OrganicBorderStyle {
    pub radius: f32,
    pub half_width: f32,
    pub displacement: f32,
    pub noise_scale: f32,
    pub seed: f32,
    pub color: Color32,
}

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct OrganicBorderUniform {
    size_px: [f32; 2],
    radius_px: f32,
    half_width_px: f32,
    displacement_px: f32,
    noise_scale_px: f32,
    inset_px: f32,
    seed: f32,
    color: [f32; 4],
}

struct OrganicBorderInstance {
    uniform_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

struct OrganicBorderRenderer {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    instances: HashMap<u64, OrganicBorderInstance>,
}

impl OrganicBorderRenderer {
    fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("organic_border_shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!(
                "shaders/organic_border.wgsl"
            ))),
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("organic_border_bind_group_layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(
                        std::mem::size_of::<OrganicBorderUniform>() as u64
                    ),
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("organic_border_pipeline_layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let fragment_entry = if target_format.is_srgb() {
            "fs_main_linear_framebuffer"
        } else {
            "fs_main_gamma_framebuffer"
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("organic_border_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some(fragment_entry),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::OneMinusDstAlpha,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview_mask: None,
            cache: None,
        });

        Self {
            pipeline,
            bind_group_layout,
            instances: HashMap::new(),
        }
    }

    fn ensure_instance(&mut self, device: &wgpu::Device, id: u64) {
        if self.instances.contains_key(&id) {
            return;
        }
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("organic_border_uniform_buffer"),
            size: std::mem::size_of::<OrganicBorderUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("organic_border_bind_group"),
            layout: &self.bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });
        self.instances.insert(
            id,
            OrganicBorderInstance {
                uniform_buffer,
                bind_group,
            },
        );
    }
}

pub fn install(
    device: &wgpu::Device,
    target_format: wgpu::TextureFormat,
    renderer: &mut egui_wgpu::Renderer,
) {
    if !renderer
        .callback_resources
        .contains::<OrganicBorderRenderer>()
    {
        renderer
            .callback_resources
            .insert(OrganicBorderRenderer::new(device, target_format));
    }
}

pub fn paint_with_id(
    ctx: &egui::Context,
    layer_id: LayerId,
    id: egui::Id,
    rect: Rect,
    style: OrganicBorderStyle,
) {
    if !crate::ui::theme::is_hand_drawn(ctx) {
        if style.half_width > 0.0 {
            ctx.layer_painter(layer_id).rect_stroke(
                rect,
                egui::CornerRadius::same(style.radius.round().clamp(0.0, 255.0) as u8),
                crate::ui::theme::border_stroke(style.color),
                egui::StrokeKind::Inside,
            );
        } else {
            ctx.layer_painter(layer_id).rect_filled(
                rect,
                egui::CornerRadius::same(style.radius.round().clamp(0.0, 255.0) as u8),
                style.color,
            );
        }
        return;
    }
    paint_with_painter(&ctx.layer_painter(layer_id), id, rect, style);
}

/// Shows a frame while reserving enough layout space for the SDF outline.
///
/// Organic displacement and antialiasing extend beyond the nominal outline.
/// Keeping that visual overflow inside the allocated rectangle prevents parent
/// containers, especially scroll areas, from clipping the border at an edge.
pub fn show<R>(
    ui: &mut egui::Ui,
    id: egui::Id,
    frame: egui::Frame,
    radius: f32,
    color: Color32,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    if !crate::ui::theme::is_hand_drawn(ui.ctx()) {
        return frame
            .stroke(crate::ui::theme::border_stroke(color))
            .show(ui, add_contents);
    }
    let response = frame
        .outer_margin(Margin::same(LAYOUT_GUTTER))
        .show(ui, add_contents);
    paint_subtle(
        ui,
        id,
        response.response.rect.shrink(LAYOUT_GUTTER as f32),
        radius,
        color,
    );
    response
}

fn paint_with_painter(
    painter: &egui::Painter,
    id: egui::Id,
    rect: Rect,
    style: OrganicBorderStyle,
) {
    let callback_rect = rect.expand(CALLBACK_MARGIN);
    painter.add(egui_wgpu::Callback::new_paint_callback(
        callback_rect,
        OrganicBorderCallback {
            id: id.value(),
            size_points: callback_rect.size().into(),
            style,
        },
    ));
}

fn paint_subtle(ui: &egui::Ui, id: egui::Id, rect: Rect, radius: f32, color: Color32) {
    let id_value = id.value();
    let seed = ((id_value ^ (id_value >> 32)) & 0xffff) as f32 / 257.0;
    paint_with_painter(
        ui.painter(),
        id,
        rect,
        OrganicBorderStyle {
            radius,
            half_width: ORGANIC_BORDER_WIDTH * 0.5,
            displacement: 0.65,
            noise_scale: 0.028,
            seed,
            color,
        },
    );
}

struct OrganicBorderCallback {
    id: u64,
    size_points: [f32; 2],
    style: OrganicBorderStyle,
}

impl CallbackTrait for OrganicBorderCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen_descriptor: &ScreenDescriptor,
        _egui_encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(renderer) = callback_resources.get_mut::<OrganicBorderRenderer>() else {
            log::warn!("organic border renderer was not installed");
            return Vec::new();
        };
        renderer.ensure_instance(device, self.id);

        let pixels_per_point = screen_descriptor.pixels_per_point;
        let [r, g, b, a] = self.style.color.to_srgba_unmultiplied();
        // SDF antialiasing reduces edge coverage once more than an egui line.
        // Compensate for strokes so organic and straight borders have comparable weight.
        let alpha = if self.style.half_width <= 0.0 {
            a as f32 / 255.0
        } else {
            (a as f32 / 255.0 * 1.25).min(1.0)
        };
        let uniform = OrganicBorderUniform {
            size_px: self.size_points.map(|value| value * pixels_per_point),
            radius_px: self.style.radius * pixels_per_point,
            half_width_px: self.style.half_width * pixels_per_point,
            displacement_px: self.style.displacement * pixels_per_point,
            noise_scale_px: self.style.noise_scale / pixels_per_point,
            inset_px: CALLBACK_MARGIN * pixels_per_point,
            seed: self.style.seed,
            color: [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, alpha],
        };
        let instance = renderer
            .instances
            .get(&self.id)
            .expect("organic border instance must exist after initialization");
        queue.write_buffer(&instance.uniform_buffer, 0, bytemuck::bytes_of(&uniform));
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        callback_resources: &CallbackResources,
    ) {
        let Some(renderer) = callback_resources.get::<OrganicBorderRenderer>() else {
            return;
        };
        let Some(instance) = renderer.instances.get(&self.id) else {
            return;
        };
        render_pass.set_pipeline(&renderer.pipeline);
        render_pass.set_bind_group(0, &instance.bind_group, &[]);
        render_pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniform_layout_matches_wgsl_alignment() {
        assert_eq!(std::mem::size_of::<OrganicBorderUniform>(), 48);
    }

    #[test]
    fn layout_gutter_contains_the_organic_outline() {
        let minimum_visual_reach = ORGANIC_BORDER_WIDTH * 0.5 + 0.65 + 0.75;
        assert!(LAYOUT_GUTTER as f32 >= minimum_visual_reach);
    }
}
