//! One subtitle layout for the desktop preview and the RGBA OpenVR texture.

use eframe::egui::{self, Color32, FontId, Painter, Pos2, Rect, Vec2};

#[derive(Clone, Debug, PartialEq)]
pub struct VrSubtitleCard {
    pub id: u64,
    pub source: String,
    pub translated: String,
    pub additional_translations: Vec<String>,
    pub speaker: String,
    pub live: bool,
}

#[derive(Clone)]
struct ScrollingCard {
    card: VrSubtitleCard,
    y: f32,
}

#[derive(Clone, Default)]
struct CardScroll {
    rows: Vec<ScrollingCard>,
    active_ids: Vec<u64>,
    time: f64,
}

impl VrSubtitleCard {
    fn primary(&self) -> &str {
        if self.translated.trim().is_empty() {
            &self.source
        } else {
            &self.translated
        }
    }

    fn show_source(&self, bilingual: bool) -> bool {
        bilingual
            && !self.translated.trim().is_empty()
            && !self.source.trim().is_empty()
            && self.source.trim() != self.translated.trim()
    }

    pub(super) fn preview_text(&self, bilingual: bool) -> String {
        let mut lines = vec![self.primary()];
        lines.extend(self.additional_translations.iter().map(String::as_str));
        if self.show_source(bilingual) {
            lines.push(&self.source);
        }
        lines
            .into_iter()
            .filter(|line| !line.trim().is_empty())
            .collect::<Vec<_>>()
            .join(" | ")
    }
}

/// Fixed-height captions scroll upward; text elides and the surface clips overflow.
pub fn paint_cards(
    painter: &Painter,
    rect: Rect,
    cards: &[VrSubtitleCard],
    bilingual: bool,
    single: bool,
    font_size: f32,
    colors: [[u8; 4]; 2],
) -> Vec<Rect> {
    if !rect.is_finite() || rect.width() <= 0.0 || rect.height() <= 0.0 {
        return Vec::new();
    }
    let scale = rect.width() / 640.0;
    let scroll_id = painter.layer_id().id.with("caption-scroll");
    let time = painter.ctx().input(|input| input.time);
    let mut scroll = painter
        .ctx()
        .data_mut(|data| data.get_temp::<CardScroll>(scroll_id))
        .unwrap_or_default();
    if single {
        scroll.rows = cards
            .last()
            .map(|card| {
                vec![ScrollingCard {
                    card: card.clone(),
                    y: 8.0,
                }]
            })
            .unwrap_or_default();
        scroll.active_ids.clear();
        painter
            .ctx()
            .data_mut(|data| data.insert_temp(scroll_id, CardScroll::default()));
    }
    let initial = scroll.rows.is_empty();
    let dt = (time - scroll.time).clamp(0.0, 0.05) as f32;
    scroll.time = time;
    const HEIGHT: f32 = 96.0;
    const STEP: f32 = HEIGHT + 8.0;
    let bottom = rect.height() / scale - 8.0;
    if !single {
        let incoming = cards
            .iter()
            .any(|card| !scroll.rows.iter().any(|row| row.card.id == card.id));
        let active_ids: Vec<_> = cards.iter().map(|card| card.id).collect();
        // Expiration/clear removes content immediately. Only rolling new captions
        // retains outgoing rows until they cross the surface edge.
        if cards.is_empty() {
            scroll.rows.clear();
        } else if !incoming && active_ids != scroll.active_ids {
            scroll
                .rows
                .retain(|row| cards.iter().any(|card| card.id == row.card.id));
        }
        scroll.active_ids = active_ids;
        for (index, card) in cards.iter().enumerate() {
            if let Some(row) = scroll.rows.iter_mut().find(|row| row.card.id == card.id) {
                row.card = card.clone();
            } else {
                let y = if initial {
                    bottom - HEIGHT - (cards.len() - 1 - index) as f32 * STEP
                } else {
                    scroll.rows.last().map_or(bottom, |row| row.y + STEP)
                };
                scroll.rows.push(ScrollingCard {
                    card: card.clone(),
                    y,
                });
            }
        }
        // Move the whole column by one offset, keeping every gap exactly constant,
        // even when another caption arrives before the previous scroll finishes.
        if let Some(last) = scroll.rows.last() {
            let distance = bottom - HEIGHT - last.y;
            let shift = if distance.abs() > 0.05 {
                painter.ctx().request_repaint();
                distance * (1.0 - (-dt / 0.09).exp())
            } else {
                distance
            };
            let last_y = last.y + shift;
            let count = scroll.rows.len();
            for (index, row) in scroll.rows.iter_mut().enumerate() {
                row.y = last_y - (count - 1 - index) as f32 * STEP;
            }
        }
        scroll
            .rows
            .retain(|row| row.y + HEIGHT > 0.0 || cards.iter().any(|card| card.id == row.card.id));
        painter
            .ctx()
            .data_mut(|data| data.insert_temp(scroll_id, scroll.clone()));
    }
    let light_background = colors[0][3] >= 160
        && (u32::from(colors[0][0]) * 299
            + u32::from(colors[0][1]) * 587
            + u32::from(colors[0][2]) * 114)
            > 160_000;
    let primary_color = if light_background {
        Color32::from_rgb(20, 28, 38)
    } else {
        Color32::from_rgb(245, 249, 252)
    };
    let secondary_color = if light_background {
        Color32::from_rgb(46, 58, 72)
    } else {
        Color32::from_rgb(213, 227, 239)
    };

    let font = if font_size.is_finite() {
        font_size.clamp(12.0, 36.0)
    } else {
        20.0
    } * scale;
    let padding = 12.0 * scale;
    let gap = 8.0 * scale;
    let budget = if single {
        (rect.height() - gap * 2.0).max(1.0)
    } else {
        HEIGHT * scale
    };
    let width = (rect.width() - padding * 4.0).max(1.0);
    let text = |value: &str, size: f32, rows: usize, color: Color32| {
        let mut job = egui::text::LayoutJob::simple(
            value.chars().take(4096).collect(),
            FontId::proportional(size),
            color,
            width,
        );
        if single {
            job.halign = egui::Align::Center;
        }
        job.wrap.max_rows = rows.max(1);
        job.wrap.break_anywhere = true;
        job.wrap.overflow_character = Some('…');
        painter.layout_job(job)
    };
    let mut layouts = Vec::new();
    for row in &scroll.rows {
        let card = &row.card;
        let secondary = card.show_source(bilingual);
        let numbered = |value: &str| {
            if card.speaker.is_empty() || value.trim().is_empty() {
                value.to_owned()
            } else {
                format!("{}  {value}", card.speaker)
            }
        };
        let primary = if secondary {
            card.primary().to_owned()
        } else {
            numbered(card.primary())
        };
        let available =
            (budget - padding * 2.0 - gap * 0.5 * card.additional_translations.len() as f32)
                .max(font * 1.2);
        let line_weight =
            if secondary { 2.3 } else { 1.2 } + card.additional_translations.len() as f32 * 1.2;
        let rows = ((available / line_weight) / font).floor().max(1.0) as usize;
        let main = text(&primary, font, rows, primary_color);
        let additional = card
            .additional_translations
            .iter()
            .map(|line| text(line, font * 0.94, rows, primary_color))
            .collect::<Vec<_>>();
        let source =
            secondary.then(|| text(&numbered(&card.source), font * 0.84, rows, secondary_color));
        layouts.push((card, main, source, row.y * scale + rect.top(), additional));
    }
    let mut card_rects = Vec::with_capacity(layouts.len());
    for (card, main, source, y, additional) in layouts {
        let bounds = Rect::from_min_size(
            Pos2::new(rect.left() + padding, y),
            Vec2::new(rect.width() - padding * 2.0, budget),
        );
        card_rects.push(bounds);
        let p = painter.with_clip_rect(bounds.intersect(rect));
        let accent = Color32::from_rgb(94, 187, 234);
        p.rect_filled(
            bounds,
            8.0 * scale,
            Color32::from_rgba_unmultiplied(colors[0][0], colors[0][1], colors[0][2], colors[0][3]),
        );
        p.rect_stroke(
            bounds,
            8.0 * scale,
            egui::Stroke::new(
                scale,
                Color32::from_rgba_unmultiplied(
                    colors[1][0],
                    colors[1][1],
                    colors[1][2],
                    colors[1][3],
                ),
            ),
            egui::StrokeKind::Inside,
        );
        let text_painter = p.with_clip_rect(bounds.shrink(padding).intersect(rect));
        let paint_text = |mut position: Pos2, galley: std::sync::Arc<egui::Galley>| {
            if single {
                position.x = bounds.center().x - galley.rect.center().x;
            }
            if colors[0][3] < 160 {
                // Transparent captions need their own contrast against the scene.
                let outline = Color32::from_black_alpha(200);
                for offset in [
                    Vec2::new(-scale, 0.0),
                    Vec2::new(scale, 0.0),
                    Vec2::new(0.0, -scale),
                    Vec2::new(0.0, scale),
                ] {
                    text_painter.galley_with_override_text_color(
                        position + offset,
                        galley.clone(),
                        outline,
                    );
                }
            }
            text_painter.galley(position, galley, Color32::WHITE);
        };
        let mut position = bounds.min + Vec2::splat(padding);
        if single || !bilingual {
            let content_height = main.size().y
                + source
                    .as_ref()
                    .map_or(0.0, |line| line.size().y + gap * 0.5)
                + additional
                    .iter()
                    .map(|line| line.size().y + gap * 0.5)
                    .sum::<f32>();
            position.y = bounds.top() + ((budget - content_height) * 0.5).max(padding);
        }
        if card.live {
            p.circle_filled(
                Pos2::new(bounds.right() - padding * 0.6, bounds.top() + padding * 0.6),
                2.0 * scale,
                accent,
            );
        }
        let h = main.size().y;
        paint_text(position, main);
        position.y += h + gap * 0.5;
        for extra in additional {
            let h = extra.size().y;
            paint_text(position, extra);
            position.y += h + gap * 0.5;
        }
        if let Some(source) = source {
            paint_text(position, source);
        }
    }
    card_rects
}

/// CPU rasterization keeps the exact subtitle layout testable without a headset,
/// GPU context or Windows-only Direct2D fallback. The only texture is egui's atlas.
pub struct VrOverlayRenderer {
    width: u32,
    height: u32,
    ctx: egui::Context,
    atlas: egui::ColorImage,
    speech_layout: crate::ui::components::avatar::Speech,
    started: std::time::Instant,
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
            speech_layout: Default::default(),
            started: std::time::Instant::now(),
        })
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn needs_frame(&self) -> bool {
        self.ctx.has_requested_repaint()
    }

    /// Returns straight-alpha RGBA. Global opacity is applied once by OpenVR.
    pub fn render(
        &mut self,
        cards: &[VrSubtitleCard],
        bilingual: bool,
        single: bool,
        font_size: f32,
        colors: [[u8; 4]; 2],
    ) -> Result<Vec<u8>, String> {
        self.render_surface(|painter, rect| {
            paint_cards(painter, rect, cards, bilingual, single, font_size, colors);
        })
    }

    pub(super) fn render_speech(
        &mut self,
        speech: &crate::ui::components::avatar::Speech,
        clock: f64,
        dt: f32,
    ) -> Result<Vec<u8>, String> {
        let mut speech = speech.on_surface(&self.speech_layout);
        let output = self.render_surface(|painter, rect| {
            speech.paint(
                painter,
                rect,
                Pos2::new(rect.center().x, rect.bottom() - 16.0),
                24.0,
                clock,
                dt,
            );
        });
        self.speech_layout = speech;
        output
    }

    pub(super) fn speech_visible(&self) -> bool {
        self.speech_layout.bounds().is_some()
    }

    pub(super) fn render_voice(
        &mut self,
        voice: &crate::voice_interaction::VoiceSnapshot,
    ) -> Result<Vec<u8>, String> {
        let status = if voice.instruction {
            if voice.busy {
                "Editing…"
            } else {
                "Editing · Describe modification…"
            }
        } else if voice.recording && matches!(voice.status.as_str(), "Ready" | "Sent") {
            "Hearing…"
        } else {
            &voice.status
        };
        let card = VrSubtitleCard {
            id: 0,
            source: String::new(),
            translated: if voice.text.trim().is_empty() {
                status.to_owned()
            } else {
                format!("{status}\n{}", voice.text)
            },
            additional_translations: Vec::new(),
            speaker: String::new(),
            live: voice.recording,
        };
        self.render(
            &[card],
            false,
            false,
            24.0,
            [
                super::runtime::default_background_color(),
                if voice.instruction {
                    [94, 225, 210, 255]
                } else {
                    super::runtime::default_border_color()
                },
            ],
        )
    }

    pub(super) fn render_surface(
        &mut self,
        mut paint: impl FnMut(&Painter, Rect),
    ) -> Result<Vec<u8>, String> {
        let rect =
            Rect::from_min_size(Pos2::ZERO, Vec2::new(self.width as f32, self.height as f32));
        let mut output = self.ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(rect),
                time: Some(self.started.elapsed().as_secs_f64()),
                ..Default::default()
            },
            |ui| paint(ui.painter(), rect),
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
    // Include fractional boundary pixels; coverage is decided at pixel centers.
    let height = pixels.len() / width;
    for y in (bounds.top().floor().max(0.0) as usize)..(bounds.bottom().ceil() as usize).min(height)
    {
        for x in
            (bounds.left().floor().max(0.0) as usize)..(bounds.right().ceil() as usize).min(width)
        {
            let point = Pos2::new(x as f32 + 0.5, y as f32 + 0.5);
            if !clip.contains(point) {
                continue;
            }
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
            // Match the atlas's linear sampling instead of stepping across glyph pixels.
            let tx = (uv.x * atlas.width() as f32 - 0.5).clamp(0.0, atlas.width() as f32 - 1.0);
            let ty = (uv.y * atlas.height() as f32 - 0.5).clamp(0.0, atlas.height() as f32 - 1.0);
            let (x0, y0) = (tx.floor() as usize, ty.floor() as usize);
            let (x1, y1) = (
                (x0 + 1).min(atlas.width() - 1),
                (y0 + 1).min(atlas.height() - 1),
            );
            let samples = [(x0, y0), (x1, y0), (x0, y1), (x1, y1)]
                .map(|(x, y)| atlas.pixels[y * atlas.width() + x].to_array());
            let (fx, fy) = (tx.fract(), ty.fract());
            let tex: [f32; 4] = std::array::from_fn(|c| {
                let top = samples[0][c] as f32 * (1.0 - fx) + samples[1][c] as f32 * fx;
                let bottom = samples[2][c] as f32 * (1.0 - fx) + samples[3][c] as f32 * fx;
                top * (1.0 - fy) + bottom * fy
            });
            let color: [f32; 4] = std::array::from_fn(|c| {
                (0..3)
                    .map(|i| v[i].color.to_array()[c] as f32 / 255.0 * weights[i])
                    .sum::<f32>()
                    * tex[c]
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
mod raster_tests {
    use super::*;

    #[test]
    fn single_caption_centers_both_languages_without_scroll_or_height_limit() {
        let ctx = egui::Context::default();
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 320.0));
        let mut card = VrSubtitleCard {
            id: 1,
            source: "Original words on this line".into(),
            translated: "Translation".into(),
            additional_translations: vec![],
            speaker: "1".into(),
            live: true,
        };
        for (index, bilingual) in [false, true, false, true].into_iter().enumerate() {
            card.id += 1;
            let mut bounds = Vec::new();
            let mut output = ctx.run_ui(
                egui::RawInput {
                    time: Some(index as f64 * 0.02),
                    screen_rect: Some(rect),
                    ..Default::default()
                },
                |ui| {
                    bounds = paint_cards(
                        ui.painter(),
                        rect,
                        std::slice::from_ref(&card),
                        bilingual,
                        true,
                        20.0,
                        [[0, 0, 0, 200], [255; 4]],
                    );
                },
            );
            assert_eq!(bounds.len(), 1);
            assert_eq!(bounds[0].height(), 304.0);
            let mut text_bounds = Rect::NOTHING;
            for shape in &output.shapes {
                if let egui::Shape::Text(text) = &shape.shape {
                    let text_rect = text.galley.rect.translate(text.pos.to_vec2());
                    assert!((text_rect.center().x - rect.center().x).abs() < 0.01);
                    text_bounds = text_bounds.union(text_rect);
                }
            }
            assert!((text_bounds.center().y - rect.center().y).abs() < 0.01);
            output.textures_delta.clear();
        }
        assert!(!ctx.has_requested_repaint());
    }

    #[test]
    fn fixed_cards_scroll_without_resizing() {
        let ctx = egui::Context::default();
        let mut cards = vec![VrSubtitleCard {
            id: 1,
            source: "Short".into(),
            translated: String::new(),
            additional_translations: vec![],
            speaker: "1".into(),
            live: true,
        }];
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 320.0));
        let frame = |time: f64, cards: &[VrSubtitleCard]| {
            let mut bounds = Vec::new();
            let mut output = ctx.run_ui(
                egui::RawInput {
                    time: Some(time),
                    screen_rect: Some(rect),
                    ..Default::default()
                },
                |ui| {
                    bounds = paint_cards(
                        ui.painter(),
                        rect,
                        cards,
                        false,
                        false,
                        20.0,
                        [[0, 0, 0, 200], [255; 4]],
                    );
                },
            );
            output.textures_delta.clear();
            bounds
        };
        let first = frame(0.0, &cards)[0];
        cards[0].source = "A long recognition result that wraps across many lines. ".repeat(20);
        assert_eq!(first, frame(0.02, &cards)[0]);
        let mut next = cards[0].clone();
        next.id = 2;
        cards.push(next);
        let moving = frame(0.04, &cards);
        assert!((moving[0].height() - first.height()).abs() < 0.001);
        assert!(moving[0].top() < first.top());
        assert!(moving[0].top() > first.top() - 104.0);
        for n in 1..40 {
            frame(0.04 + n as f64 * 0.05, &cards);
        }
        let settled = frame(2.1, &cards);
        assert_eq!(settled[0].top(), first.top() - 104.0);
        assert!(!ctx.has_requested_repaint());
        for index in 0..8 {
            let mut next = cards.last().unwrap().clone();
            next.id += 1;
            cards.push(next);
            if cards.len() > 3 {
                cards.remove(0);
            }
            let bounds = frame(2.12 + index as f64 * 0.02, &cards);
            for pair in bounds.windows(2) {
                assert!(
                    (pair[1].top() - pair[0].bottom() - 8.0).abs() < 0.001,
                    "Scrolling must preserve gaps during overlapping arrivals"
                );
            }
        }
        assert!(frame(2.4, &[]).is_empty());
    }

    #[test]
    fn preview_and_headset_keep_the_same_three_captions() {
        let ctx = egui::Context::default();
        let mut cards = vec![
            VrSubtitleCard {
                id: 0,
                source: "Original caption".into(),
                translated: "Translated caption".into(),
                additional_translations: vec![],
                speaker: "1".into(),
                live: false,
            };
            3
        ];
        for (index, card) in cards.iter_mut().enumerate() {
            card.id = index as u64;
        }
        let colors = [
            super::super::runtime::default_background_color(),
            super::super::runtime::default_border_color(),
        ];
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            for width in [244.0, 640.0] {
                let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(width, width * 0.5));
                assert_eq!(
                    paint_cards(ui.painter(), rect, &cards, true, false, 20.0, colors).len(),
                    3
                );
            }
        });
        output.textures_delta.clear();
    }

    #[test]
    fn fractional_triangle_bounds_cover_pixel_centers_once() {
        let atlas = egui::ColorImage::filled([1, 1], Color32::WHITE);
        let mut pixels = vec![[0.0; 4]; 9];
        let vertex = |x, y| egui::epaint::Vertex {
            pos: Pos2::new(x, y),
            uv: Pos2::new(0.5, 0.5),
            color: Color32::from_white_alpha(128),
        };
        let a = vertex(0.25, 0.25);
        let b = vertex(2.25, 0.25);
        let c = vertex(2.25, 2.25);
        let d = vertex(0.25, 2.25);
        let clip = Rect::from_min_max(Pos2::ZERO, Pos2::new(3.0, 3.0));
        for triangle in [[a, b, c], [a, c, d]] {
            raster_triangle(&mut pixels, 3, clip, triangle, &atlas);
        }
        for y in 0..3 {
            for x in 0..3 {
                let expected = if x < 2 && y < 2 { 128.0 / 255.0 } else { 0.0 };
                assert!(
                    (pixels[y * 3 + x][3] - expected).abs() < 1e-6,
                    "incorrect coverage at ({x}, {y}): {:?}",
                    pixels[y * 3 + x]
                );
            }
        }
    }
}
