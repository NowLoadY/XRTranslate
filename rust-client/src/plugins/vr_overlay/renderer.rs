//! One subtitle layout for the desktop preview and the RGBA OpenVR texture.

use eframe::egui::{self, Color32, FontId, Painter, Pos2, Rect, Vec2};

#[derive(Clone, Debug, PartialEq)]
pub struct VrSubtitleCard {
    pub source: String,
    pub translated: String,
    pub speaker: String,
    pub live: bool,
}

/// Paint newest captions last. When space is tight, retain the newest cards and
/// truncate individual paragraphs instead of letting an old paragraph hide them.
pub fn paint_cards(
    painter: &Painter,
    rect: Rect,
    cards: &[VrSubtitleCard],
    bilingual: bool,
    font_size: f32,
) {
    if cards.is_empty() || !rect.is_finite() || rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    let scale = rect.width() / 640.0;
    let font = if font_size.is_finite() {
        font_size.clamp(12.0, 36.0)
    } else {
        20.0
    } * scale;
    let padding = 12.0 * scale;
    let gap = 8.0 * scale;
    let min_height = font * if bilingual { 2.8 } else { 1.7 } + padding * 2.0;
    let count = cards.len().min(
        ((rect.height() - gap) / (min_height + gap))
            .floor()
            .max(1.0) as usize,
    );
    let cards = &cards[cards.len() - count..];
    let budget = (rect.height() - gap * (count as f32 + 1.0)) / count as f32;
    let width = (rect.width() - padding * 4.0).max(1.0);
    let text = |value: &str, size: f32, rows: usize, color: Color32| {
        let mut job = egui::text::LayoutJob::simple(
            value.chars().take(4096).collect(),
            FontId::proportional(size),
            color,
            width,
        );
        job.wrap.max_rows = rows.max(1);
        job.wrap.break_anywhere = true;
        painter.layout_job(job)
    };
    let mut layouts = Vec::new();
    for card in cards {
        let primary = if card.translated.trim().is_empty() {
            &card.source
        } else {
            &card.translated
        };
        let secondary = bilingual
            && !card.translated.trim().is_empty()
            && !card.source.trim().is_empty()
            && card.source.trim() != card.translated.trim();
        let header = (!card.speaker.is_empty()).then(|| {
            text(
                &card.speaker,
                font * 0.68,
                1,
                Color32::from_rgb(143, 191, 226),
            )
        });
        let header_height = header.as_ref().map_or(0.0, |g| g.size().y + gap * 0.5);
        let available = (budget - padding * 2.0 - header_height).max(font * 1.2);
        let rows = ((available / if secondary { 2.3 } else { 1.2 }) / font)
            .floor()
            .max(1.0) as usize;
        let main = text(primary, font, rows, Color32::from_rgb(240, 247, 252));
        let source = secondary.then(|| {
            text(
                &card.source,
                font * 0.84,
                rows,
                Color32::from_rgb(169, 190, 211),
            )
        });
        let height = padding * 2.0
            + header_height
            + main.size().y
            + source.as_ref().map_or(0.0, |g| g.size().y + gap * 0.5);
        layouts.push((card, header, main, source, height.min(budget)));
    }
    let total = layouts.iter().map(|x| x.4 + gap).sum::<f32>() - gap;
    let mut y = rect.center().y - total * 0.5;
    for (card, header, main, source, height) in layouts {
        let bounds = Rect::from_min_size(
            Pos2::new(rect.left() + padding, y),
            Vec2::new(rect.width() - padding * 2.0, height),
        );
        let p = painter.with_clip_rect(bounds.intersect(rect));
        let accent = Color32::from_rgb(94, 187, 234);
        p.rect_filled(
            bounds,
            8.0 * scale,
            Color32::from_rgba_unmultiplied(13, 19, 29, 200),
        );
        p.rect_stroke(
            bounds,
            8.0 * scale,
            egui::Stroke::new(
                scale,
                if card.live {
                    accent
                } else {
                    Color32::from_rgb(57, 77, 97)
                },
            ),
            egui::StrokeKind::Inside,
        );
        let mut position = bounds.min + Vec2::splat(padding);
        if let Some(header) = header {
            let h = header.size().y;
            p.galley(position, header, Color32::WHITE);
            position.y += h + gap * 0.5;
        }
        if card.live {
            p.circle_filled(
                Pos2::new(bounds.right() - padding * 0.6, bounds.top() + padding * 0.6),
                2.0 * scale,
                accent,
            );
        }
        let h = main.size().y;
        p.galley(position, main, Color32::WHITE);
        if let Some(source) = source {
            position.y += h + gap * 0.5;
            p.galley(position, source, Color32::WHITE);
        }
        y += height + gap;
    }
}

/// CPU rasterization keeps the exact subtitle layout testable without a headset,
/// GPU context or Windows-only Direct2D fallback. The only texture is egui's atlas.
pub struct VrOverlayRenderer {
    width: u32,
    height: u32,
    ctx: egui::Context,
    atlas: egui::ColorImage,
}

impl VrOverlayRenderer {
    pub fn new(width: u32, height: u32) -> Result<Self, String> {
        super::openvr::raw_rgba_len(width, height)?;
        let ctx = egui::Context::default();
        crate::ui::fonts::configure_multilingual_fonts(&ctx);
        Ok(Self {
            width,
            height,
            ctx,
            atlas: egui::ColorImage::default(),
        })
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Returns straight-alpha RGBA. Global opacity is applied once by OpenVR.
    pub fn render(
        &mut self,
        cards: &[VrSubtitleCard],
        bilingual: bool,
        font_size: f32,
    ) -> Result<Vec<u8>, String> {
        let rect =
            Rect::from_min_size(Pos2::ZERO, Vec2::new(self.width as f32, self.height as f32));
        let mut output = self.ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(rect),
                ..Default::default()
            },
            |ui| {
                paint_cards(ui.painter(), rect, cards, bilingual, font_size);
            },
        );
        let deltas = std::mem::take(&mut output.textures_delta.set);
        output.textures_delta.clear();
        for (id, deltas) in &deltas {
            for delta in deltas {
                if *id != egui::TextureId::Managed(0) {
                    return Err("Unexpected subtitle texture".into());
                }
                let egui::ImageData::Color(image) = &delta.image;
                if let Some([x, y]) = delta.pos {
                    if x.checked_add(image.width())
                        .is_none_or(|end| end > self.atlas.width())
                        || y.checked_add(image.height())
                            .is_none_or(|end| end > self.atlas.height())
                    {
                        return Err("Subtitle atlas update is out of bounds".into());
                    }
                    for row in 0..image.height() {
                        let start = (y + row) * self.atlas.width() + x;
                        self.atlas.pixels[start..start + image.width()].copy_from_slice(
                            &image.pixels[row * image.width()..(row + 1) * image.width()],
                        );
                    }
                } else {
                    self.atlas = (**image).clone();
                }
            }
        }
        let meshes = self.ctx.tessellate(output.shapes, output.pixels_per_point);
        let pixel_count = super::openvr::raw_rgba_len(self.width, self.height)? / 4;
        let mut pixels = vec![[0.0_f32; 4]; pixel_count];
        if self.atlas.pixels.is_empty() {
            return Ok(vec![0; pixel_count * 4]);
        }
        for primitive in meshes {
            if let egui::epaint::Primitive::Mesh(mesh) = primitive.primitive {
                let clip = primitive.clip_rect.intersect(rect);
                for triangle in mesh.indices.chunks_exact(3) {
                    let v = std::array::from_fn(|i| mesh.vertices[triangle[i] as usize]);
                    raster_triangle(&mut pixels, self.width as usize, clip, v, &self.atlas);
                }
            }
        }
        Ok(pixels
            .into_iter()
            .flat_map(|p| {
                let alpha = p[3];
                [
                    if alpha > 0.0 {
                        (p[0] / alpha * 255.0).round() as u8
                    } else {
                        0
                    },
                    if alpha > 0.0 {
                        (p[1] / alpha * 255.0).round() as u8
                    } else {
                        0
                    },
                    if alpha > 0.0 {
                        (p[2] / alpha * 255.0).round() as u8
                    } else {
                        0
                    },
                    (alpha * 255.0).round() as u8,
                ]
            })
            .collect())
    }
}

fn raster_triangle(
    pixels: &mut [[f32; 4]],
    width: usize,
    clip: Rect,
    mut v: [egui::epaint::Vertex; 3],
    atlas: &egui::ColorImage,
) {
    let edge = |a: Pos2, b: Pos2, p: Pos2| (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
    let mut area = edge(v[0].pos, v[1].pos, v[2].pos);
    if area < 0.0 {
        v.swap(1, 2);
        area = -area;
    }
    if area < 1e-6 {
        return;
    }
    let bounds = Rect::from_min_max(
        Pos2::new(
            v.iter().map(|p| p.pos.x).fold(f32::INFINITY, f32::min),
            v.iter().map(|p| p.pos.y).fold(f32::INFINITY, f32::min),
        ),
        Pos2::new(
            v.iter().map(|p| p.pos.x).fold(f32::NEG_INFINITY, f32::max),
            v.iter().map(|p| p.pos.y).fold(f32::NEG_INFINITY, f32::max),
        ),
    )
    .intersect(clip);
    for y in bounds.top().ceil() as usize..bounds.bottom().ceil() as usize {
        for x in bounds.left().ceil() as usize..bounds.right().ceil() as usize {
            let point = Pos2::new(x as f32 + 0.5, y as f32 + 0.5);
            let e = [
                edge(v[1].pos, v[2].pos, point),
                edge(v[2].pos, v[0].pos, point),
                edge(v[0].pos, v[1].pos, point),
            ];
            // Top-left fill rule: shared triangle edges must be blended only once.
            let inside = (0..3).all(|i| {
                let a = v[(i + 1) % 3].pos;
                let b = v[(i + 2) % 3].pos;
                e[i] > 0.0 || (e[i] == 0.0 && (b.y < a.y || (b.y == a.y && b.x > a.x)))
            });
            if !inside {
                continue;
            }
            let weights = e.map(|e| e / area);
            let uv = v[0].uv.to_vec2() * weights[0]
                + v[1].uv.to_vec2() * weights[1]
                + v[2].uv.to_vec2() * weights[2];
            let tex = atlas.pixels[(uv.y * atlas.height() as f32)
                .floor()
                .clamp(0.0, atlas.height() as f32 - 1.0)
                as usize
                * atlas.width()
                + (uv.x * atlas.width() as f32)
                    .floor()
                    .clamp(0.0, atlas.width() as f32 - 1.0) as usize]
                .to_array();
            let color: [f32; 4] = std::array::from_fn(|c| {
                (0..3)
                    .map(|i| v[i].color.to_array()[c] as f32 / 255.0 * weights[i])
                    .sum::<f32>()
                    * tex[c] as f32
                    / 255.0
            });
            let dst = &mut pixels[y * width + x];
            for c in 0..4 {
                dst[c] = color[c] + dst[c] * (1.0 - color[3]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_dimensions_and_nonfinite_font_never_reach_unbounded_allocations() {
        for (w, h) in [(0, 320), (640, 0), (u32::MAX, u32::MAX), (2048, 2048)] {
            assert!(VrOverlayRenderer::new(w, h).is_err());
        }
        let mut renderer = VrOverlayRenderer::new(640, 320).unwrap();
        let card = VrSubtitleCard {
            source: "text".into(),
            translated: "字幕".into(),
            speaker: String::new(),
            live: true,
        };
        let expected = renderer.render(&[card.clone()], true, 20.0).unwrap();
        for font in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(
                renderer.render(&[card.clone()], true, font).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn repeated_count_font_language_and_empty_frames_keep_one_fixed_rgba_size() {
        let mut renderer = VrOverlayRenderer::new(640, 320).unwrap();
        let card = VrSubtitleCard {
            source: "日本語 English العربية ".repeat(1000),
            translated: "中文字幕".repeat(1000),
            speaker: "Speaker".into(),
            live: true,
        };
        for i in 0..30 {
            let count = i % 6;
            let cards = vec![card.clone(); count];
            let buffer = renderer
                .render(&cards, i % 2 == 0, if i % 2 == 0 { 12.0 } else { 36.0 })
                .unwrap();
            assert_eq!(buffer.len(), 819200);
            if count == 0 {
                assert!(buffer.iter().all(|x| *x == 0));
            } else {
                assert!(buffer.chunks_exact(4).any(|p| p[3] != 0));
            }
        }
        assert_eq!(renderer.dimensions(), (640, 320));
    }

    #[test]
    fn produces_text_pixels_and_transparency_on_every_platform() {
        let mut renderer = VrOverlayRenderer::new(640, 320).unwrap();
        let card = VrSubtitleCard {
            source: "Hello world".into(),
            translated: "你好，世界".into(),
            speaker: "Alice".into(),
            live: true,
        };
        let empty = renderer.render(&[], true, 20.0).unwrap();
        assert!(empty.iter().all(|x| *x == 0));
        let bilingual = renderer.render(&[card.clone()], true, 20.0).unwrap();
        let mono = renderer.render(&[card], false, 20.0).unwrap();
        assert_ne!(bilingual, mono);
        assert!(bilingual.chunks_exact(4).any(|p| p[0] > 220 && p[3] > 100));
        assert!(bilingual.chunks_exact(4).any(|p| p[3] == 0));
        assert!(bilingual.chunks_exact(4).any(|p| p[3] == 200));
    }
    #[test]
    fn latest_caption_survives_long_history_and_source_is_not_duplicated() {
        let mut renderer = VrOverlayRenderer::new(640, 320).unwrap();
        let old = VrSubtitleCard {
            source: "old ".repeat(2000),
            translated: "旧字幕".repeat(2000),
            speaker: "Speaker ".repeat(50),
            live: false,
        };
        let latest = VrSubtitleCard {
            source: "LATEST".into(),
            translated: String::new(),
            speaker: String::new(),
            live: true,
        };
        let bilingual = renderer.render(&[latest.clone()], true, 36.0).unwrap();
        assert_eq!(
            bilingual,
            renderer.render(&[latest.clone()], false, 36.0).unwrap()
        );
        let mixed = renderer
            .render(
                &[
                    old.clone(),
                    old.clone(),
                    old.clone(),
                    old.clone(),
                    latest.clone(),
                ],
                true,
                36.0,
            )
            .unwrap();
        let mixed_other = renderer
            .render(
                &[
                    old.clone(),
                    old.clone(),
                    old.clone(),
                    old,
                    VrSubtitleCard {
                        source: "OTHER".into(),
                        ..latest
                    },
                ],
                true,
                36.0,
            )
            .unwrap();
        assert_ne!(mixed, mixed_other);
    }
}
