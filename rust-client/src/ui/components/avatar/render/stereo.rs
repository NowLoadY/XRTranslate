//! Two real mesh views packed left/right into one reusable GPU overlay texture.
use super::super::{Appearance, Pose};
use super::{CallbackResources, CallbackTrait, Draw, Renderer, ScreenDescriptor, egui, wgpu};
use glam::Mat4;
use std::time::{Duration, Instant};

pub(crate) const EYE_SIZE: u32 = 512;

pub(crate) struct StereoRenderer {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub texture: wgpu::Texture,
    resources: CallbackResources,
    pending: Option<wgpu::SubmissionIndex>,
    errors: std::sync::Arc<parking_lot::Mutex<Option<String>>>,
    hair: super::super::hair::motion::Motion,
    clock: Instant,
}

impl StereoRenderer {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        let errors = std::sync::Arc::new(parking_lot::Mutex::new(None));
        let recorded = std::sync::Arc::clone(&errors);
        device.on_uncaptured_error(std::sync::Arc::new(move |error| {
            *recorded.lock() = Some(error.to_string());
        }));
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("companion stereo overlay"),
            size: wgpu::Extent3d {
                width: EYE_SIZE * 2,
                height: EYE_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let mut resources = CallbackResources::default();
        resources.insert(Renderer::new(&device, wgpu::TextureFormat::Rgba8Unorm));
        Self {
            device,
            queue,
            texture,
            resources,
            pending: None,
            errors,
            hair: Default::default(),
            clock: Instant::now(),
        }
    }

    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub fn render(
        &mut self,
        mut pose: Pose,
        appearance: &Appearance,
        model_transform: Mat4,
        cameras: [Mat4; 2],
    ) -> Result<bool, String> {
        if let Some(error) = self.errors.lock().take() {
            return Err(error);
        }
        // At most one frame is in flight. A slow GPU cannot grow a queue or
        // monopolize the caption worker; complete it on a later worker tick.
        if let Some(submission) = self.pending.take() {
            match self.device.poll(wgpu::PollType::Wait {
                submission_index: Some(submission.clone()),
                timeout: Some(Duration::ZERO),
            }) {
                Ok(_) => {
                    if let Some(error) = self.errors.lock().take() {
                        return Err(error);
                    }
                    return Ok(true);
                }
                Err(wgpu::PollError::Timeout) => {
                    self.pending = Some(submission);
                    return Ok(false);
                }
                Err(error) => return Err(error.to_string()),
            }
        }
        if !model_transform.is_finite() || cameras.iter().any(|m| !m.is_finite()) {
            return Err("Invalid companion camera".into());
        }
        let (scale, rotation, position) = model_transform.to_scale_rotation_translation();
        let (yaw, pitch, roll) = rotation.to_euler(glam::EulerRot::YXZ);
        pose.hair = self.hair.sample(
            glam::Vec3::new(
                yaw + pose.gaze.yaw,
                pitch + pose.gaze.pitch,
                pose.roll - roll,
            ),
            position,
            scale.x.abs(),
            self.clock.elapsed().as_secs_f64(),
        );
        let mut model = appearance.model().assemble(pose);
        // Authored widths use model units. Convert with the uniform spatial
        // scale, keeping the requested threefold VR contour at any model size.
        let outline_scale = model_transform.x_axis.truncate().length() * 3.0;
        for part in &mut model.parts {
            part.transform = model_transform * part.transform;
            part.material.outline_width *= outline_scale;
        }
        let mut encoder = self.device.create_command_encoder(&Default::default());
        for (eye, camera) in cameras.into_iter().enumerate() {
            let mut draw = Draw::new(
                eye as u64,
                egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::splat(EYE_SIZE as f32)),
                model.clone(),
            );
            draw.projection = Some(camera);
            draw.prepare(
                &self.device,
                &self.queue,
                &ScreenDescriptor {
                    size_in_pixels: [EYE_SIZE; 2],
                    pixels_per_point: 1.0,
                },
                &mut encoder,
                &mut self.resources,
            );
            let renderer = self
                .resources
                .get::<Renderer>()
                .expect("stereo renderer is installed");
            encoder.copy_texture_to_texture(
                renderer.targets[&(eye as u64)].texture.as_image_copy(),
                wgpu::TexelCopyTextureInfo {
                    origin: wgpu::Origin3d {
                        x: eye as u32 * EYE_SIZE,
                        y: 0,
                        z: 0,
                    },
                    ..self.texture.as_image_copy()
                },
                wgpu::Extent3d {
                    width: EYE_SIZE,
                    height: EYE_SIZE,
                    depth_or_array_layers: 1,
                },
            );
        }
        // Valve requires TRANSFER_SRC_OPTIMAL and leaves it in that state.
        // Use wgpu's tracker, so the next COPY_DST transition remains correct.
        encoder.transition_resources(
            std::iter::empty(),
            std::iter::once(wgpu::TextureTransition {
                texture: &self.texture,
                selector: None,
                state: wgpu::TextureUses::COPY_SRC,
            }),
        );
        let submission = self.queue.submit([encoder.finish()]);
        match self.device.poll(wgpu::PollType::Wait {
            submission_index: Some(submission.clone()),
            timeout: Some(Duration::from_millis(4)),
        }) {
            Ok(_) => {}
            Err(wgpu::PollError::Timeout) => {
                self.pending = Some(submission);
                return Ok(false);
            }
            Err(error) => return Err(error.to_string()),
        }
        if let Some(error) = self.errors.lock().take() {
            return Err(error);
        }
        Ok(true)
    }
}
