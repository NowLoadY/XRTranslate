//! Debug-only 3D → transparent PNG output, using the application's actual meshes and render passes.
use super::super::{Classic, Gaze, Pose, wardrobe::Socket};
use super::{CallbackResources, CallbackTrait, Draw, Renderer, ScreenDescriptor, egui, wgpu};
use eframe::icon_data::IconDataExt;
use glam::{Mat4, Vec3};
use std::{error::Error, path::Path, time::Duration};

pub(crate) fn render_views(directory: &Path) -> Result<(), Box<dyn Error + Send + Sync>> {
    futures::executor::block_on(render(directory))
}

async fn render(directory: &Path) -> Result<(), Box<dyn Error + Send + Sync>> {
    const SIZE: u32 = 1024; // Four bytes per pixel is already aligned to WGPU's copy-row requirement.
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = instance.request_adapter(&Default::default()).await?;
    let (device, queue) = adapter.request_device(&Default::default()).await?;
    let mut resources = CallbackResources::default();
    resources.insert(Renderer::new(&device, wgpu::TextureFormat::Rgba8UnormSrgb));
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("avatar PNG readback"),
        size: (SIZE * SIZE * 4) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    std::fs::create_dir_all(directory)?;
    let mut preview = vec![0; (SIZE * SIZE * 3 * 4) as usize];
    let paper = egui::Rgba::from(egui::Color32::from_gray(240));
    for (view, (name, yaw, pitch, joy, blink, exploded)) in [
        ("front", 0.0, 0.0, 0.0, 0.0, false),
        ("three-quarter", -0.55, 0.10, 0.0, 0.0, false),
        ("side", -std::f32::consts::FRAC_PI_2, 0.0, 0.0, 0.0, false),
        ("back", std::f32::consts::PI, 0.0, 0.0, 0.0, false),
        ("happy", -0.20, 0.10, 1.0, 0.0, false),
        ("blink", 0.0, 0.0, 0.0, 1.0, false),
        ("curious", 0.0, 0.0, 0.0, 0.0, false),
        ("exploded", -0.35, -0.30, 0.0, 0.0, true),
        ("underside", -0.35, -0.75, 0.0, 0.0, true),
    ]
    .into_iter()
    .enumerate()
    {
        let mut avatar = Classic::model().clone();
        if exploded {
            for attachment in &mut avatar.attachments {
                let offset = if attachment.socket == Socket::Hat {
                    0.75
                } else {
                    -0.65
                };
                attachment.transform = Mat4::from_translation(Vec3::Y * offset);
            }
        }
        let mut model = avatar.assemble(Pose {
            gaze: Gaze { yaw, pitch },
            joy,
            blink,
            curiosity: if name == "curious" { 1.0 } else { 0.0 },
            ..Default::default()
        });
        // Fit the sheet into the same orthographic camera used by the floating UI avatar.
        let scale = if exploded { 0.79 } else { 1.27 };
        for part in &mut model.parts {
            part.transform = Mat4::from_translation(Vec3::Y * -0.24)
                * Mat4::from_scale(Vec3::splat(scale))
                * part.transform;
        }
        let draw = Draw::new(
            0,
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::splat(SIZE as f32)),
            model,
        );
        let mut encoder = device.create_command_encoder(&Default::default());
        draw.prepare(
            &device,
            &queue,
            &ScreenDescriptor {
                size_in_pixels: [SIZE; 2],
                pixels_per_point: 1.0,
            },
            &mut encoder,
            &mut resources,
        );
        let renderer = resources.get::<Renderer>().unwrap();
        encoder.copy_texture_to_buffer(
            renderer.targets[&0].texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(SIZE * 4),
                    rows_per_image: Some(SIZE),
                },
            },
            wgpu::Extent3d {
                width: SIZE,
                height: SIZE,
                depth_or_array_layers: 1,
            },
        );
        let submission = queue.submit([encoder.finish()]);
        let (sender, receiver) = std::sync::mpsc::channel();
        readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        device.poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(Duration::from_secs(20)),
        })?;
        receiver.recv_timeout(Duration::from_secs(1))??;
        let mut rgba = readback.slice(..).get_mapped_range()?.to_vec();
        readback.unmap();
        // The MSAA target contains premultiplied linear color; PNG uses straight sRGB alpha.
        for (index, pixel) in rgba.chunks_exact_mut(4).enumerate() {
            let color = egui::Rgba::from_rgba_premultiplied(
                pixel[0] as f32 / 255.0,
                pixel[1] as f32 / 255.0,
                pixel[2] as f32 / 255.0,
                pixel[3] as f32 / 255.0,
            );
            pixel.copy_from_slice(&color.to_srgba_unmultiplied());
            if view < 3 {
                let offset = (index / SIZE as usize * SIZE as usize * 3
                    + view * SIZE as usize
                    + index % SIZE as usize)
                    * 4;
                preview[offset..offset + 4].copy_from_slice(
                    &egui::Color32::from(color + paper * (1.0 - color.a())).to_array(),
                );
            }
        }
        let png = egui::IconData {
            rgba,
            width: SIZE,
            height: SIZE,
        }
        .to_png_bytes()?;
        std::fs::write(directory.join(format!("{name}.png")), png)?;
    }
    std::fs::write(
        directory.join("preview.png"),
        egui::IconData {
            rgba: preview,
            width: SIZE * 3,
            height: SIZE,
        }
        .to_png_bytes()?,
    )?;
    Ok(())
}
