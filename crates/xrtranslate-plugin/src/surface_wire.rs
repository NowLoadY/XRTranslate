//! Negotiated MessagePack frames share the control pipe without JSON conversion.
//! A zero byte distinguishes a length-prefixed frame from a JSON control message.
use super::Frame;
#[cfg(any(feature = "host", test))]
use std::io::Read;
use std::io::{self, Write};

pub(crate) const CAPABILITY: &str = "ui.surface.msgpack.v1";

pub(crate) fn write(output: &mut impl Write, frame: &Frame) -> io::Result<()> {
    // Named fields preserve serde defaults and optional fields in egui's types.
    let bytes = rmp_serde::to_vec_named(frame).map_err(io::Error::other)?;
    if bytes.len() > crate::MAX_MESSAGE_BYTES {
        return Err(io::Error::other("Plugin frame is too large"));
    }
    output.write_all(&[0])?;
    output.write_all(&(bytes.len() as u32).to_le_bytes())?;
    output.write_all(&bytes)?;
    output.flush()
}

/// The caller has consumed the zero-byte marker.
#[cfg(any(feature = "host", test))]
pub(crate) fn read(input: &mut impl Read) -> io::Result<Frame> {
    let mut length = [0; 4];
    input.read_exact(&mut length)?;
    let length = u32::from_le_bytes(length) as usize;
    if length > crate::MAX_MESSAGE_BYTES {
        return Err(io::Error::other("Plugin frame is too large"));
    }
    let mut bytes = vec![0; length];
    input.read_exact(&mut bytes)?;
    rmp_serde::from_slice(&bytes).map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::{Drawing, Paint};
    use egui::{
        Color32, ColorImage, Rect, TextureId,
        epaint::{ImageDelta, Mesh},
    };
    use std::{io::Cursor, sync::Arc, time::Duration};

    #[test]
    fn binary_frame_preserves_meshes_textures_platform_and_control_boundaries() {
        let mut mesh = Mesh::with_texture(TextureId::Managed(0));
        let rect = Rect::from_min_size(egui::pos2(4.25, 8.5), egui::vec2(80.0, 32.0));
        mesh.add_colored_rect(rect, Color32::WHITE);
        let pixels = [Color32::RED, Color32::GREEN];
        let frame = Frame {
            drawings: vec![
                Drawing::Mesh {
                    clip: Rect::EVERYTHING,
                    mesh: Arc::new(mesh),
                },
                Drawing::Paint {
                    clip: rect,
                    paint: Paint {
                        rect,
                        name: "test".into(),
                        data: serde_json::json!({"text":"字幕", "optional":null}),
                    },
                },
            ],
            reuse_drawings: false,
            shared: None,
            content_bounds: Some(crate::surface::ContentBounds {
                occupied: vec![rect],
                heading: Some(rect),
            }),
            textures: vec![(
                TextureId::Managed(0),
                vec![ImageDelta::full(
                    ColorImage::new([2, 1], pixels.to_vec()),
                    Default::default(),
                )],
            )],
            free: vec![TextureId::Managed(2)],
            platform: egui::PlatformOutput {
                cursor_icon: egui::CursorIcon::Text,
                commands: vec![egui::OutputCommand::CopyText("hello".into())],
                ..Default::default()
            },
            snapshot: Default::default(),
            feedback: Vec::new(),
            feedback_until: 0.0,
            repaint_after: Duration::MAX,
        };
        let mut bytes = Vec::new();
        write(&mut bytes, &frame).unwrap();
        bytes.extend_from_slice(b"{\"type\":\"cancel\",\"id\":3}\n");
        assert_eq!(bytes[0], 0);
        let mut input = Cursor::new(&bytes[1..]);
        let decoded = read(&mut input).unwrap();
        assert!(decoded.drawings == frame.drawings);
        assert_eq!(decoded.content_bounds, frame.content_bounds);
        assert_eq!(decoded.platform.cursor_icon, egui::CursorIcon::Text);
        assert_eq!(decoded.platform.commands.len(), 1);
        assert_eq!(decoded.free, frame.free);
        assert_eq!(decoded.repaint_after, Duration::MAX);
        let egui::ImageData::Color(image) = &decoded.textures[0].1[0].image;
        assert_eq!(image.pixels, pixels);
        let mut remaining = String::new();
        input.read_to_string(&mut remaining).unwrap();
        assert_eq!(remaining, "{\"type\":\"cancel\",\"id\":3}\n");
        // Legacy JSON texture encoding remains available to older hosts.
        let mut legacy = serde_json::to_value(&Frame {
            drawings: Vec::new(),
            ..frame
        })
        .unwrap();
        legacy.as_object_mut().unwrap().remove("content_bounds");
        let decoded: Frame = serde_json::from_value(legacy).unwrap();
        assert!(decoded.content_bounds.is_none());
        let egui::ImageData::Color(image) = &decoded.textures[0].1[0].image;
        assert_eq!(image.pixels, pixels);
    }

    #[test]
    fn rejects_oversized_or_truncated_frames() {
        let oversized = (crate::MAX_MESSAGE_BYTES as u32 + 1).to_le_bytes();
        assert!(read(&mut oversized.as_slice()).is_err());
        assert!(read(&mut [10, 0, 0, 0, 1].as_slice()).is_err());
    }
}
