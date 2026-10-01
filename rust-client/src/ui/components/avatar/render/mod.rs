//! Instanced 3D solids rendered with depth into reusable transparent MSAA targets.
#[cfg(debug_assertions)]
pub(crate) mod export;
pub(crate) mod stereo;
use super::{
    geometry,
    model::{Geometry, Model, Part},
};
use bytemuck::{Pod, Zeroable};
use eframe::{
    egui::{self, Id, Rect},
    egui_wgpu::{self, CallbackResources, CallbackTrait, ScreenDescriptor},
    wgpu::{self, util::DeviceExt},
};
use glam::{Mat3, Vec3};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

const COLOR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const SAMPLES: u32 = 4;

#[derive(Clone, Copy, PartialEq, Pod, Zeroable)]
#[repr(C)]
struct Instance {
    model: [[f32; 4]; 4],
    normal: [[f32; 4]; 3],
    color: [f32; 4],
    shape: [f32; 4],
}
impl From<&Part> for Instance {
    fn from(part: &Part) -> Self {
        Self {
            model: part.transform.to_cols_array_2d(),
            normal: Mat3::from_mat4(part.transform)
                .inverse()
                .transpose()
                .to_cols_array_2d()
                .map(|v| [v[0], v[1], v[2], 0.0]),
            color: egui::Rgba::from(part.color).to_array(),
            shape: [
                part.morph[0],
                part.morph[1],
                part.material.shade_contrast,
                part.material.outline_width,
            ],
        }
    }
}

struct Target {
    size: u32,
    capacity: usize,
    parts: wgpu::Buffer,
    color: wgpu::TextureView,
    texture: wgpu::Texture,
    msaa: wgpu::TextureView,
    depth: wgpu::TextureView,
    shadow: wgpu::TextureView,
    lighting: wgpu::BindGroup,
    composite: wgpu::BindGroup,
    composite_uv: wgpu::Buffer,
    last_uv: [f32; 4],
    last_seen: Instant,
    last_parts: Vec<Instance>,
    last_camera: Option<glam::Mat4>,
    camera: Option<(wgpu::Buffer, wgpu::BindGroup)>,
    last_batches: Vec<Batch>,
}
impl Target {
    fn new(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        shadow_layout: &wgpu::BindGroupLayout,
        shadow_sampler: &wgpu::Sampler,
        size: u32,
        count: usize,
    ) -> Self {
        let texture = |format, samples, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("avatar target"),
                size: wgpu::Extent3d {
                    width: size,
                    height: size,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let color_texture = texture(
            COLOR,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
        );
        let color = color_texture.create_view(&Default::default());
        let msaa = texture(COLOR, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT)
            .create_view(&Default::default());
        let depth = texture(DEPTH, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT)
            .create_view(&Default::default());
        let composite = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("avatar composite"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&color),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        });
        let shadow_size = (size * 2).clamp(256, 768);
        let shadow = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("avatar shadow"),
                size: wgpu::Extent3d {
                    width: shadow_size,
                    height: shadow_size,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        let lighting = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("avatar lighting"),
            layout: shadow_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&shadow),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(shadow_sampler),
                },
            ],
        });
        let capacity = count.max(1).next_power_of_two();
        let parts = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("avatar parts"),
            size: (capacity * size_of::<Instance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let composite_uv = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("avatar composite UV"),
            size: 16,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            size,
            capacity,
            parts,
            color,
            texture: color_texture,
            msaa,
            depth,
            shadow,
            lighting,
            composite,
            composite_uv,
            last_uv: [0.0; 4],
            last_seen: Instant::now(),
            last_parts: Vec::new(),
            last_camera: None,
            camera: None,
            last_batches: Vec::new(),
        }
    }
}

struct Renderer {
    model_pipeline: wgpu::RenderPipeline,
    outline_pipeline: wgpu::RenderPipeline,
    shadow_pipeline: wgpu::RenderPipeline,
    shadow_layout: wgpu::BindGroupLayout,
    shadow_sampler: wgpu::Sampler,
    composite_pipeline: wgpu::RenderPipeline,
    camera: wgpu::BindGroup,
    camera_layout: wgpu::BindGroupLayout,
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    ranges: Vec<std::ops::Range<u32>>,
    targets: HashMap<u64, Target>,
}

impl Renderer {
    fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let model_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("avatar 3D shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("avatar.wgsl").into()),
        });
        let composite_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("avatar composite shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("composite.wgsl").into()),
        });
        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("avatar camera layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("avatar texture layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let shadow_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("avatar shadow layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
            ],
        });
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("avatar soft shadow sampler"),
            compare: Some(wgpu::CompareFunction::LessEqual),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let mesh_attributes = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 11 => Float32x3, 12 => Float32x3, 13 => Float32x3, 14 => Float32x3];
        let instance_attributes = wgpu::vertex_attr_array![2 => Float32x4, 3 => Float32x4, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4, 7 => Float32x4, 8 => Float32x4, 9 => Float32x4, 10 => Float32x4];
        let buffers = [
            Some(wgpu::VertexBufferLayout {
                array_stride: size_of::<geometry::Vertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &mesh_attributes,
            }),
            Some(wgpu::VertexBufferLayout {
                array_stride: size_of::<Instance>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &instance_attributes,
            }),
        ];
        let pipeline = |shader: &wgpu::ShaderModule,
                        layouts: &[Option<&wgpu::BindGroupLayout>],
                        vertex,
                        fragment: Option<&str>,
                        buffers: &[Option<wgpu::VertexBufferLayout>],
                        format,
                        depth,
                        samples,
                        blend,
                        cull_mode| {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("avatar pipeline layout"),
                bind_group_layouts: layouts,
                immediate_size: 0,
            });
            let targets = [Some(wgpu::ColorTargetState {
                format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })];
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("avatar pipeline"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some(vertex),
                    buffers,
                    compilation_options: Default::default(),
                },
                primitive: wgpu::PrimitiveState {
                    cull_mode,
                    ..Default::default()
                },
                depth_stencil: depth,
                multisample: wgpu::MultisampleState {
                    count: samples,
                    ..Default::default()
                },
                fragment: fragment.map(|entry| wgpu::FragmentState {
                    module: shader,
                    entry_point: Some(entry),
                    targets: &targets,
                    compilation_options: Default::default(),
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let model_depth = wgpu::DepthStencilState {
            format: DEPTH,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: Default::default(),
            bias: Default::default(),
        };
        let model_pipeline = pipeline(
            &model_shader,
            &[Some(&camera_layout), Some(&shadow_layout)],
            "model_vertex",
            Some("model_fragment"),
            &buffers,
            COLOR,
            Some(model_depth.clone()),
            SAMPLES,
            None,
            None,
        );
        let outline_pipeline = pipeline(
            &model_shader,
            &[Some(&camera_layout)],
            "outline_vertex",
            Some("outline_fragment"),
            &buffers,
            COLOR,
            Some(model_depth),
            SAMPLES,
            None,
            Some(wgpu::Face::Front),
        );
        let shadow_pipeline = pipeline(
            &model_shader,
            &[Some(&camera_layout)],
            "shadow_vertex",
            None,
            &buffers,
            COLOR,
            Some(wgpu::DepthStencilState {
                format: DEPTH,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: wgpu::DepthBiasState {
                    constant: 1,
                    slope_scale: 1.0,
                    clamp: 0.0,
                },
            }),
            1,
            None,
            None,
        );
        let composite_pipeline = pipeline(
            &composite_shader,
            &[Some(&texture_layout)],
            "composite_vertex",
            Some(if target_format.is_srgb() {
                "composite_linear"
            } else {
                "composite_gamma"
            }),
            &[Some(wgpu::VertexBufferLayout {
                array_stride: 16,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &wgpu::vertex_attr_array![0 => Float32x4],
            })],
            target_format,
            None,
            1,
            Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
            None,
        );
        let matrix = glam::camera::rh::proj::directx::orthographic(-2.2, 2.2, -2.2, 2.2, 0.1, 20.0)
            * glam::camera::rh::view::look_at_mat4(Vec3::new(0.0, 0.0, 6.0), Vec3::ZERO, Vec3::Y);
        let light_matrix =
            glam::camera::rh::proj::directx::orthographic(-2.3, 2.3, -2.3, 2.3, 0.1, 16.0)
                * glam::camera::rh::view::look_at_mat4(
                    Vec3::new(-3.0, 5.0, 7.0),
                    Vec3::ZERO,
                    Vec3::Y,
                );
        let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("avatar camera"),
            contents: bytemuck::cast_slice(&[matrix.to_cols_array(), light_matrix.to_cols_array()]),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let camera = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("avatar camera"),
            layout: &camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });
        let (vertices, indices, ranges) = geometry::atlas();
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("avatar vertices"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("avatar indices"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            model_pipeline,
            outline_pipeline,
            shadow_pipeline,
            shadow_layout,
            shadow_sampler,
            composite_pipeline,
            camera,
            camera_layout,
            texture_layout,
            sampler,
            vertices,
            indices,
            ranges,
            targets: HashMap::new(),
        }
    }
}

pub fn install(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    renderer: &mut egui_wgpu::Renderer,
) {
    renderer
        .callback_resources
        .insert(Renderer::new(device, format));
}

pub(super) fn paint(painter: &egui::Painter, id: Id, rect: Rect, model: Model) {
    let callback = Draw::new(id.value(), rect, model);
    painter.add(egui_wgpu::Callback::new_paint_callback(rect, callback));
}

struct Draw {
    id: u64,
    rect: Rect,
    batches: Vec<Batch>,
    parts: Vec<Instance>,
    projection: Option<glam::Mat4>,
}

#[derive(Clone, PartialEq)]
struct Batch {
    geometry: Geometry,
    instances: std::ops::Range<u32>,
}
impl CallbackTrait for Draw {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen: &ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(renderer) = resources.get_mut::<Renderer>() else {
            return Vec::new();
        };
        let size = (self.rect.width() * screen.pixels_per_point)
            .ceil()
            .clamp(32.0, 1024.0) as u32;
        renderer.targets.retain(|id, target| {
            *id == self.id || target.last_seen.elapsed() < Duration::from_secs(30)
        });
        if renderer
            .targets
            .get(&self.id)
            // Keep the larger target while the character recedes; composition
            // handles its on-screen size without reallocating textures per frame.
            .is_none_or(|target| target.size < size || target.capacity < self.parts.len())
        {
            renderer.targets.insert(
                self.id,
                Target::new(
                    device,
                    &renderer.texture_layout,
                    &renderer.sampler,
                    &renderer.shadow_layout,
                    &renderer.shadow_sampler,
                    size,
                    self.parts.len(),
                ),
            );
        }
        let target = renderer.targets.get_mut(&self.id).unwrap();
        target.last_seen = Instant::now();
        // egui clamps callback viewports to the framebuffer. Sample only that
        // part of the original image instead of squeezing the whole image into it.
        let viewport = egui::PaintCallbackInfo {
            viewport: self.rect,
            clip_rect: self.rect,
            pixels_per_point: screen.pixels_per_point,
            screen_size_px: screen.size_in_pixels,
        }
        .viewport_in_pixels();
        let rect_px = self.rect * screen.pixels_per_point;
        let uv = [
            (viewport.left_px as f32 - rect_px.left()) / rect_px.width(),
            (viewport.top_px as f32 - rect_px.top()) / rect_px.height(),
            viewport.width_px as f32 / rect_px.width(),
            viewport.height_px as f32 / rect_px.height(),
        ];
        if target.last_uv != uv {
            queue.write_buffer(&target.composite_uv, 0, bytemuck::cast_slice(&uv));
            target.last_uv = uv;
        }
        if target.last_camera != self.projection {
            if let Some(matrix) = self.projection {
                let light = glam::camera::rh::proj::directx::orthographic(
                    -0.45, 0.45, -0.45, 0.45, 0.01, 4.0,
                ) * glam::camera::rh::view::look_at_mat4(
                    Vec3::new(-0.6, 1.0, 1.4),
                    Vec3::ZERO,
                    Vec3::Y,
                );
                let values = [matrix.to_cols_array(), light.to_cols_array()];
                if target.camera.is_none() {
                    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("avatar eye camera"),
                        size: 128,
                        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    });
                    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("avatar eye camera"),
                        layout: &renderer.camera_layout,
                        entries: &[wgpu::BindGroupEntry {
                            binding: 0,
                            resource: buffer.as_entire_binding(),
                        }],
                    });
                    target.camera = Some((buffer, bind));
                }
                queue.write_buffer(
                    &target.camera.as_ref().unwrap().0,
                    0,
                    bytemuck::cast_slice(&values),
                );
            } else {
                target.camera = None;
            }
        }
        let camera_changed = target.last_camera != self.projection;
        target.last_camera = self.projection;
        if !camera_changed && target.last_parts == self.parts && target.last_batches == self.batches
        {
            return Vec::new();
        }
        target.last_parts.clear();
        target.last_parts.extend_from_slice(&self.parts);
        target.last_batches.clone_from(&self.batches);
        queue.write_buffer(&target.parts, 0, bytemuck::cast_slice(&self.parts));
        let target = &renderer.targets[&self.id];
        {
            let mut shadow_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("avatar shadow pass"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &target.shadow,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            shadow_pass.set_pipeline(&renderer.shadow_pipeline);
            self.draw_parts(&mut shadow_pass, renderer, target);
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("avatar 3D pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target.msaa,
                depth_slice: None,
                resolve_target: Some(&target.color),
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Discard,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &target.depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&renderer.outline_pipeline);
        self.draw_parts(&mut pass, renderer, target);
        pass.set_pipeline(&renderer.model_pipeline);
        pass.set_bind_group(1, &target.lighting, &[]);
        self.draw_parts(&mut pass, renderer, target);
        Vec::new()
    }
    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &CallbackResources,
    ) {
        let Some(renderer) = resources.get::<Renderer>() else {
            return;
        };
        if let Some(target) = renderer.targets.get(&self.id) {
            pass.set_pipeline(&renderer.composite_pipeline);
            pass.set_bind_group(0, &target.composite, &[]);
            pass.set_vertex_buffer(0, target.composite_uv.slice(..));
            pass.draw(0..3, 0..1);
        }
    }
}

impl Draw {
    fn new(id: u64, rect: Rect, mut model: Model) -> Self {
        model.parts.sort_by_key(|part| part.geometry as u8);
        let mut batches: Vec<Batch> = Vec::new();
        for (index, part) in model.parts.iter().enumerate() {
            if let Some(batch) = batches
                .last_mut()
                .filter(|batch| batch.geometry == part.geometry)
            {
                batch.instances.end += 1;
            } else {
                batches.push(Batch {
                    geometry: part.geometry,
                    instances: index as u32..index as u32 + 1,
                });
            }
        }
        Self {
            id,
            rect,
            batches,
            parts: model.parts.iter().map(Instance::from).collect(),
            projection: None,
        }
    }

    fn draw_parts(&self, pass: &mut wgpu::RenderPass<'_>, renderer: &Renderer, target: &Target) {
        pass.set_bind_group(
            0,
            target
                .camera
                .as_ref()
                .map_or(&renderer.camera, |camera| &camera.1),
            &[],
        );
        pass.set_vertex_buffer(0, renderer.vertices.slice(..));
        pass.set_vertex_buffer(1, target.parts.slice(..));
        pass.set_index_buffer(renderer.indices.slice(..), wgpu::IndexFormat::Uint32);
        for batch in &self.batches {
            pass.draw_indexed(
                renderer.ranges[batch.geometry as usize].clone(),
                0,
                batch.instances.clone(),
            );
        }
    }
}
