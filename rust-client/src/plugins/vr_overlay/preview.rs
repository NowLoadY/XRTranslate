//! Offline preview of the actual stereo mesh passes; no SteamVR mock image.
use super::{renderer::VrOverlayRenderer, space::CompanionSpace};
use crate::ui::components::avatar::{Gaze, Pose, Speech, StereoRenderer};
use eframe::{egui, icon_data::IconDataExt, wgpu};
use glam::{Mat4, Vec3};
use std::{error::Error, path::Path, time::Duration};

pub(crate) fn render(directory: &Path) -> Result<(), Box<dyn Error + Send + Sync>> {
    futures::executor::block_on(render_async(directory))
}

async fn render_async(directory: &Path) -> Result<(), Box<dyn Error + Send + Sync>> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = instance.request_adapter(&Default::default()).await?;
    let (device, queue) = adapter.request_device(&Default::default()).await?;
    let mut renderer = StereoRenderer::new(device, queue);
    let readback = renderer.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("VR companion preview"),
        size: 1024 * 512 * 4,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    std::fs::create_dir_all(directory)?;
    let head = Mat4::from_translation(Vec3::Y * 1.6);
    let eye_positions =
        |head: Mat4| [-0.032, 0.032].map(|x| head * Mat4::from_translation(Vec3::X * x));
    let mut space = CompanionSpace::default();
    space
        .update(head, eye_positions(head), 0.0)
        .ok_or("Invalid initial scene")?;
    let mut sheets = Vec::new();
    let mut label_renderer = VrOverlayRenderer::new(1024, 72)?;
    let header = label_renderer.render_surface(|painter, rect| {
        painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(236, 240, 246));
        painter.text(
            egui::pos2(24.0, 20.0),
            egui::Align2::LEFT_TOP,
            "XRTranslate · 3D avatar",
            egui::FontId::proportional(22.0),
            egui::Color32::from_rgb(44, 53, 70),
        );
        painter.text(
            egui::pos2(24.0, 50.0),
            egui::Align2::LEFT_TOP,
            "Offline preview · VR hardware untested",
            egui::FontId::proportional(14.0),
            egui::Color32::from_rgb(90, 104, 124),
        );
    })?;

    let mut talking = Pose::default();
    talking.speech = 0.65;
    talking.gaze = Gaze {
        yaw: 0.1,
        pitch: 0.0,
    };
    for (name, x, pose) in [
        ("front", 0.0, Pose::default()),
        ("left", -0.6, Pose::default()),
        ("right", 0.6, Pose::default()),
        ("talking", 0.0, talking),
    ] {
        let view = head * Mat4::from_translation(Vec3::X * x);
        let scene = space
            .update(view, eye_positions(view), 0.0)
            .ok_or("Invalid preview scene")?;
        for attempt in 0..4 {
            if renderer.render(pose, scene.model, scene.cameras)? {
                break;
            }
            renderer.device.poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(Duration::from_secs(5)),
            })?;
            if attempt == 3 {
                return Err("Companion GPU preview did not complete".into());
            }
        }
        let mut encoder = renderer.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            renderer.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(1024 * 4),
                    rows_per_image: Some(512),
                },
            },
            wgpu::Extent3d {
                width: 1024,
                height: 512,
                depth_or_array_layers: 1,
            },
        );
        renderer.queue.submit([encoder.finish()]);
        readback.map_async(wgpu::MapMode::Read, .., |_| {});
        renderer.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(5)),
        })?;
        let pixels = readback.get_mapped_range(..)?.to_vec();
        readback.unmap();
        let visible = pixels.chunks_exact(4).filter(|p| p[3] > 64).count();
        if visible < 1000 {
            return Err(format!("{name}: companion did not render").into());
        }
        let mut differences = 0;
        for row in pixels.chunks_exact(1024 * 4) {
            differences += row[..512 * 4]
                .chunks_exact(4)
                .zip(row[512 * 4..].chunks_exact(4))
                .filter(|(l, r)| l != r)
                .count();
        }
        if differences < 500 {
            return Err(format!("{name}: stereo eyes unexpectedly identical").into());
        }
        println!("{name}: {visible} visible pixels; {differences} different eye pixels");
        let mut straight = pixels.clone();
        let paper = egui::Rgba::from(egui::Color32::from_rgb(236, 240, 246));
        let mut sheet = Vec::with_capacity(pixels.len());
        for (dest, source) in straight.chunks_exact_mut(4).zip(pixels.chunks_exact(4)) {
            let color = egui::Rgba::from_rgba_premultiplied(
                source[0] as f32 / 255.0,
                source[1] as f32 / 255.0,
                source[2] as f32 / 255.0,
                source[3] as f32 / 255.0,
            );
            dest.copy_from_slice(&color.to_srgba_unmultiplied());
            sheet.extend_from_slice(
                &egui::Color32::from(color + paper * (1.0 - color.a())).to_array(),
            );
        }
        std::fs::write(
            directory.join(format!("{name}-stereo.png")),
            egui::IconData {
                rgba: straight,
                width: 1024,
                height: 512,
            }
            .to_png_bytes()?,
        )?;
        let label = match name {
            "front" => "Front",
            "left" => "View from left",
            "right" => "View from right",
            _ => "Speaking",
        };
        sheets.push(label_renderer.render_surface(|painter, rect| {
            painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(236, 240, 246));
            for (x, eye) in [(24.0, "Left eye"), (536.0, "Right eye")] {
                painter.text(
                    egui::pos2(x, 42.0),
                    egui::Align2::LEFT_CENTER,
                    format!("{label} · {eye}"),
                    egui::FontId::proportional(17.0),
                    egui::Color32::from_rgb(70, 84, 109),
                );
            }
        })?);
        sheets.push(sheet);
    }
    // Rows preserve left/right eye ordering; head translation changes the view
    // of the same world-space body, then the same mouth animation is sampled.
    let mut overview = header;
    for sheet in sheets {
        overview.extend_from_slice(&sheet);
    }
    std::fs::write(
        directory.join("stereo-preview.png"),
        egui::IconData {
            rgba: overview,
            width: 1024,
            height: 72 + (512 + 72) * 4,
        }
        .to_png_bytes()?,
    )?;
    let mut speech = Speech::default();
    speech.say("Hi, I'm right here!", 0.0);
    speech.advance(1.5);
    let mut text = VrOverlayRenderer::new(640, 320)?;
    for _ in 0..20 {
        text.render_speech(&speech, 1.5)?;
    }
    let pixels = text.render_speech(&speech, 1.5)?;
    std::fs::write(
        directory.join("speech.png"),
        egui::IconData {
            rgba: pixels,
            width: 640,
            height: 320,
        }
        .to_png_bytes()?,
    )?;
    Ok(())
}
