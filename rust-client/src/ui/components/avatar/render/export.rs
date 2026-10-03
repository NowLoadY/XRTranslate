//! Debug-only 3D → transparent PNG output, using the application's actual meshes and render passes.
use super::super::{Classic, Gaze, HeadShape, Pose};
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
    export_mesh(directory)?;
    let mut preview = vec![0; (SIZE * SIZE * 3 * 2 * 4) as usize];
    let paper = egui::Rgba::from(egui::Color32::from_gray(240));
    for (view, (name, yaw, pitch, joy, blink, hair)) in [
        ("front", 0.0, 0.0, 0.0, 0.0, [0.0; 2]),
        ("three-quarter", -0.55, 0.10, 0.0, 0.0, [0.0; 2]),
        (
            "side",
            -std::f32::consts::FRAC_PI_2,
            0.0,
            0.0,
            0.0,
            [0.0; 2],
        ),
        ("back-three-quarter", -2.35, 0.0, 0.0, 0.0, [0.0; 2]),
        ("back", std::f32::consts::PI, 0.0, 0.0, 0.0, [0.0; 2]),
        ("top", 0.0, 1.05, 0.0, 0.0, [0.0; 2]),
        ("happy", -0.20, 0.10, 1.0, 0.0, [0.0; 2]),
        ("blink", 0.0, 0.0, 0.0, 1.0, [0.0; 2]),
        ("curious", 0.0, 0.0, 0.0, 0.0, [0.0; 2]),
        ("thinking", 0.0, 0.0, 0.0, 0.0, [0.0; 2]),
        ("concerned", 0.0, 0.0, 0.0, 0.0, [0.0; 2]),
        ("talking", 0.0, 0.0, 0.2, 0.0, [0.0; 2]),
        ("relaxed", 0.0, 0.0, 0.32, 0.0, [0.0; 2]),
        ("half-blink", 0.0, 0.0, 0.0, 0.5, [0.0; 2]),
        ("sway-left", 0.0, 0.0, 0.0, 0.0, [-1.0, 0.0]),
        ("sway-right", 0.0, 0.0, 0.0, 0.0, [1.0, 0.0]),
        ("sway-forward", -0.55, 0.10, 0.0, 0.0, [0.0, 1.0]),
        ("sway-back", -0.55, 0.10, 0.0, 0.0, [0.0, -1.0]),
        ("head", -0.35, -0.10, 0.0, 0.0, [0.0; 2]),
        ("head-front", 0.0, 0.0, 0.0, 0.0, [0.0; 2]),
        (
            "head-side",
            -std::f32::consts::FRAC_PI_2,
            0.0,
            0.0,
            0.0,
            [0.0; 2],
        ),
        (
            "crown",
            0.0,
            std::f32::consts::FRAC_PI_2,
            0.0,
            0.0,
            [0.0; 2],
        ),
        ("dressed", -0.35, 0.0, 0.0, 0.0, [0.0; 2]),
        (
            "dressed-side",
            -std::f32::consts::FRAC_PI_2,
            0.0,
            0.0,
            0.0,
            [0.0; 2],
        ),
        ("slim", -0.35, 0.0, 0.0, 0.0, [0.0; 2]),
        ("wide", -0.35, 0.0, 0.0, 0.0, [0.0; 2]),
        ("round", -0.35, 0.0, 0.0, 0.0, [0.0; 2]),
        ("tapered", -0.35, 0.0, 0.0, 0.0, [0.0; 2]),
        ("head-round", -0.35, -0.10, 0.0, 0.0, [0.0; 2]),
        ("head-tapered", -0.35, -0.10, 0.0, 0.0, [0.0; 2]),
        ("dressed-round", -0.35, 0.0, 0.0, 0.0, [0.0; 2]),
        ("dressed-tapered", -0.35, 0.0, 0.0, 0.0, [0.0; 2]),
    ]
    .into_iter()
    .enumerate()
    {
        let mut avatar = Classic::model().clone();
        avatar.head = if name.ends_with("round") {
            HeadShape {
                cheeks: 0.8,
                chin: 0.65,
            }
        } else if name.ends_with("tapered") {
            HeadShape {
                cheeks: -0.55,
                chin: -0.8,
            }
        } else {
            HeadShape::default()
        };
        if name.starts_with("head") {
            avatar.hair = None;
        }
        if name.starts_with("dressed") || matches!(name, "slim" | "wide") {
            avatar.attachments = vec![
                Classic::cap(egui::Color32::from_rgb(170, 185, 212)),
                Classic::scarf(egui::Color32::from_rgb(143, 167, 198)),
            ];
            avatar.aspect = match name {
                "slim" => 0.86,
                "wide" => 1.16,
                _ => 1.0,
            };
        }
        let mut model = avatar.assemble(Pose {
            gaze: Gaze { yaw, pitch },
            joy,
            blink,
            hair,
            curiosity: if name == "curious" { 1.0 } else { 0.0 },
            thinking: if name == "thinking" { 1.0 } else { 0.0 },
            concern: if name == "concerned" { 1.0 } else { 0.0 },
            speech: if name == "talking" { 0.85 } else { 0.0 },
            ..Default::default()
        });
        // Fit the sheet into the same orthographic camera used by the floating UI avatar.
        let scale = 1.27;
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
            if view < 6 {
                let offset =
                    ((view / 3 * SIZE as usize + index / SIZE as usize) * SIZE as usize * 3
                        + view % 3 * SIZE as usize
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
            height: SIZE * 2,
        }
        .to_png_bytes()?,
    )?;
    Ok(())
}

/// The same generated geometry used by the renderer, for cross-section inspection.
fn export_mesh(directory: &Path) -> Result<(), Box<dyn Error + Send + Sync>> {
    use super::super::geometry;
    use std::fmt::Write;
    let mut output = String::new();
    let mut base = 1;
    for (name, mesh) in [
        ("head", geometry::body::mesh()),
        ("hair", super::super::hair::geometry::mesh()),
    ] {
        writeln!(output, "o {name}")?;
        for vertex in &mesh.vertices {
            let [x, y, z] = vertex.position;
            writeln!(output, "v {x} {y} {z}")?;
            let [x, y, z] = vertex.normal;
            writeln!(output, "vn {x} {y} {z}")?;
        }
        for face in mesh.indices.chunks_exact(3) {
            let [a, b, c] = [face[0] + base, face[1] + base, face[2] + base];
            writeln!(output, "f {a}//{a} {b}//{b} {c}//{c}")?;
        }
        base += mesh.vertices.len() as u32;
    }
    std::fs::write(directory.join("geometry.obj"), output)?;
    Ok(())
}
