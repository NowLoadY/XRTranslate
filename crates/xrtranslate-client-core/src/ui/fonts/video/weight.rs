use super::Installed;
use ab_glyph::{Font, FontRef, ScaleFont};
use eframe::egui::{
    self,
    epaint::text::{
        FontPriority, GlyphBitmap, GlyphRasterizer, GlyphRasterizerRequest, RasterizedGlyph,
        VariationCoords,
    },
};
use std::{path::PathBuf, sync::Arc};

pub(super) fn real_face(face: &Installed, regular: &egui::FontData) -> Option<egui::FontData> {
    if let Some((path, index)) = bold_file(face)
        && (path != face.path || index != face.index)
        && let Ok(bytes) = std::fs::read(path)
    {
        let mut bold = egui::FontData::from_owned(bytes);
        bold.index = index;
        return Some(bold);
    }
    if regular
        .variation_axes()
        .iter()
        .any(|axis| axis.tag.to_string() == "wght" && axis.range.max >= 700.)
    {
        let mut bold = regular.clone();
        bold.tweak.coords = VariationCoords::new([(b"wght", 700.)]);
        return Some(bold);
    }
    None
}

#[cfg(windows)]
fn bold_file(face: &Installed) -> Option<(PathBuf, u32)> {
    let name = match face.path.file_name()?.to_str()? {
        "msyh.ttc" => "msyhbd.ttc",
        "msjh.ttc" => "msjhbd.ttc",
        "arial.ttf" => "arialbd.ttf",
        _ => return None,
    };
    Some((face.path.with_file_name(name), 0))
}

#[cfg(target_os = "android")]
fn bold_file(face: &Installed) -> Option<(PathBuf, u32)> {
    let name = face
        .path
        .file_name()?
        .to_str()?
        .replace("-Regular.", "-Bold.");
    Some((face.path.with_file_name(name), face.index))
}

#[cfg(not(any(windows, target_os = "android")))]
fn bold_file(face: &Installed) -> Option<(PathBuf, u32)> {
    super::super::fontconfig_face(&face.name, true)
}

pub(super) fn install_synthetic(
    ctx: &egui::Context,
    family: egui::FontFamily,
    data: Arc<egui::FontData>,
) {
    let key = format!("xrtranslate:weight:{family:?}");
    ctx.add_glyph_rasterizer(
        GlyphRasterizer::new(key, move |request| {
            if request.family != &family {
                return None;
            }
            synthetic(&data, request)
        })
        .with_priority(FontPriority::Highest),
    );
}

/// Static faces such as SimSun have no bold file. libass widens their glyphs
/// by one sixty-fourth of an em without changing the advance. Expand the
/// cached coverage once so opacity, karaoke, outlines and shadows still use
/// a single silhouette.
fn synthetic(
    data: &egui::FontData,
    request: &GlyphRasterizerRequest<'_>,
) -> Option<RasterizedGlyph> {
    let mut cluster = request.cluster.chars();
    let first = cluster.next()?;
    if first.is_whitespace() || cluster.next().is_some() {
        return None;
    }
    let font = FontRef::try_from_slice_and_index(data.bytes(), data.index).ok()?;
    let scale = request.font_size_px * font.height_unscaled() / font.units_per_em()?;
    let scaled = font.as_scaled(scale);
    let glyph = scaled.scaled_glyph(first);
    if glyph.id.0 == 0 {
        return None;
    }
    let advance_px = scaled.h_advance(glyph.id);
    let outline = font.outline_glyph(glyph)?;
    let bounds = outline.px_bounds();
    let (w, h) = (bounds.width() as usize, bounds.height() as usize);
    if w == 0 || h == 0 || w.max(h) > 2048 {
        return None;
    }
    let strength = request.font_size_px / 64.;
    let pad = strength.ceil() as usize;
    let (width, height) = (w + pad, h + pad);
    let mut row_mask = vec![0u8; width * h];
    let coverage = |value: u8, step: usize| {
        (f32::from(value) * (strength + 1. - step as f32).clamp(0., 1.)).round() as u8
    };
    outline.draw(|x, y, alpha| {
        let i = y as usize * width + x as usize;
        let value = (alpha * 255.).round() as u8;
        for dx in 0..=pad {
            row_mask[i + dx] = row_mask[i + dx].max(coverage(value, dx));
        }
    });
    let mut mask = vec![0; width * height];
    for (i, &value) in row_mask.iter().enumerate().filter(|(_, alpha)| **alpha > 0) {
        for dy in 0..=pad {
            let dst = i + (pad - dy) * width;
            mask[dst] = mask[dst].max(coverage(value, dy));
        }
    }
    Some(RasterizedGlyph {
        advance_px,
        bitmap: GlyphBitmap {
            image: egui::ColorImage::new(
                [width, height],
                mask.into_iter()
                    .map(egui::Color32::from_white_alpha)
                    .collect(),
            ),
            offset_px: egui::vec2(bounds.min.x, bounds.min.y - pad as f32),
            is_color: false,
        },
    })
}
