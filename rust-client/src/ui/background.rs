use std::{
    fs,
    io::{Cursor, Read},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
};

use eframe::egui::{self, Color32, ColorImage, Painter, Rect, TextureHandle, Vec2};
use image::{ImageDecoder, ImageFormat, ImageReader};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_IMAGE_SIDE: u32 = 16_384;
const MAX_IMAGE_PIXELS: u64 = 40_000_000;
const MAX_TEXTURE_SIDE: usize = 4096;

pub fn default_opacity() -> f32 {
    super::theme::content_backdrop(true).a() as f32 / 255.0
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BackgroundSettings {
    pub image_path: Option<PathBuf>,
    pub opacity: f32,
}

impl Default for BackgroundSettings {
    fn default() -> Self {
        Self {
            image_path: None,
            opacity: default_opacity(),
        }
    }
}

impl BackgroundSettings {
    pub fn normalize(&mut self) {
        self.opacity = normalized_opacity(self.opacity);
    }
}

fn normalized_opacity(opacity: f32) -> f32 {
    if opacity.is_finite() {
        opacity.clamp(0.0, 1.0)
    } else {
        default_opacity()
    }
}

struct LoadedImage {
    path: PathBuf,
    image: ColorImage,
}

/// Decoding and managed-file writes happen on a worker. The current texture
/// remains visible until a replacement has successfully finished loading.
#[derive(Default)]
pub struct BackgroundImage {
    texture: Option<TextureHandle>,
    attempted_path: Option<PathBuf>,
    pending: Option<Receiver<Result<LoadedImage, String>>>,
}

impl BackgroundImage {
    pub fn ensure_loaded(&mut self, settings: &BackgroundSettings, ctx: &egui::Context) {
        // An import does not change persisted settings until it succeeds. Do
        // not let the old settings cancel that pending replacement.
        if self.pending.is_some() || self.attempted_path == settings.image_path {
            return;
        }
        self.attempted_path = settings.image_path.clone();
        if let Some(path) = &settings.image_path {
            self.start_load(path.clone(), None, ctx);
        } else {
            self.reset();
        }
    }

    pub fn request_import(&mut self, path: PathBuf, project_root: PathBuf, ctx: &egui::Context) {
        self.start_load(path, Some(project_root), ctx);
    }

    fn start_load(&mut self, path: PathBuf, project_root: Option<PathBuf>, ctx: &egui::Context) {
        let max_side = ctx
            .input(|input| input.max_texture_side)
            .clamp(1, MAX_TEXTURE_SIDE) as u32;
        let (sender, receiver) = mpsc::channel();
        let error_sender = sender.clone();
        let repaint = ctx.clone();
        // Dropping the old receiver also discards completions from an older
        // selection or from a request that was reset while decoding.
        self.pending = Some(receiver);
        if let Err(error) = std::thread::Builder::new()
            .name("background-image".into())
            .spawn(move || {
                let result = load_image(&path, project_root.as_deref(), max_side);
                let _ = sender.send(result);
                repaint.request_repaint();
            })
        {
            let _ = error_sender.send(Err(format!("Unable to load background image: {error}")));
            ctx.request_repaint();
        }
    }

    /// A successful result returns the durable path to persist in settings.
    /// Startup loads return the existing path and need no settings rewrite.
    pub fn poll(&mut self, ctx: &egui::Context) -> Option<Result<PathBuf, String>> {
        let result = match self.pending.as_ref()?.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => {
                Err("The background image loader stopped unexpectedly".into())
            }
        };
        self.pending = None;
        Some(match result {
            Ok(loaded) => {
                self.texture = Some(ctx.load_texture(
                    "application-background",
                    loaded.image,
                    egui::TextureOptions::LINEAR,
                ));
                self.attempted_path = Some(loaded.path.clone());
                Ok(loaded.path)
            }
            Err(error) => Err(error),
        })
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn is_loaded(&self) -> bool {
        self.texture.is_some()
    }

    /// Paint one image beneath all panels. Cropping the UVs fills the whole
    /// window without stretching, with excess image cropped equally on each
    /// side. Zero opacity still replaces the default white mask.
    pub fn paint(&self, painter: &Painter, rect: Rect, opacity: f32) -> bool {
        let Some(texture) = &self.texture else {
            return false;
        };
        if let Some(uv) = cover_uv(texture.size_vec2(), rect.size()) {
            let alpha = (normalized_opacity(opacity) * 255.0).round() as u8;
            if alpha > 0 {
                painter.with_clip_rect(rect).image(
                    texture.id(),
                    rect,
                    uv,
                    Color32::from_white_alpha(alpha),
                );
            }
        }
        true
    }
}

fn cover_uv(image: Vec2, viewport: Vec2) -> Option<Rect> {
    if !image.is_finite()
        || !viewport.is_finite()
        || image.x <= 0.0
        || image.y <= 0.0
        || viewport.x <= 0.0
        || viewport.y <= 0.0
    {
        return None;
    }
    let image_aspect = image.x / image.y;
    let viewport_aspect = viewport.x / viewport.y;
    let visible = if image_aspect > viewport_aspect {
        egui::vec2(viewport_aspect / image_aspect, 1.0)
    } else {
        egui::vec2(1.0, image_aspect / viewport_aspect)
    };
    Some(Rect::from_center_size(egui::pos2(0.5, 0.5), visible))
}

fn validate_dimensions(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err("The background image has no pixels".into());
    }
    if width > MAX_IMAGE_SIDE
        || height > MAX_IMAGE_SIDE
        || u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS
    {
        return Err(
            "The background image is too large (maximum 40 megapixels and 16384 pixels per side)"
                .into(),
        );
    }
    Ok(())
}

fn load_image(
    path: &Path,
    project_root: Option<&Path>,
    max_side: u32,
) -> Result<LoadedImage, String> {
    let file = fs::File::open(path)
        .map_err(|error| format!("Unable to open background image: {error}"))?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Unable to read background image: {error}"))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err("The background image file exceeds 64 MB".into());
    }
    let mut reader = ImageReader::new(Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|error| format!("Unable to identify background image: {error}"))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_SIDE);
    limits.max_image_height = Some(MAX_IMAGE_SIDE);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let mut decoder = reader
        .into_decoder()
        .map_err(|error| format!("Unable to decode background image: {error}"))?;
    let (width, height) = decoder.dimensions();
    validate_dimensions(width, height)?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut decoded = image::DynamicImage::from_decoder(decoder)
        .map_err(|error| format!("Unable to decode background image: {error}"))?;
    decoded.apply_orientation(orientation);
    if decoded.width() > max_side || decoded.height() > max_side {
        decoded = decoded.resize(max_side, max_side, image::imageops::FilterType::Triangle);
    }
    let rgba = decoded.into_rgba8();
    let durable_path = if let Some(project_root) = project_root {
        // A content-addressed local copy survives moves/deletions of the
        // selected file and never overwrites a previously selected image.
        let hash = Sha256::digest(&bytes);
        let directory = project_root.join("runtime/backgrounds");
        fs::create_dir_all(&directory)
            .map_err(|error| format!("Unable to create background image folder: {error}"))?;
        let target = directory.join(format!("{hash:x}-{max_side}.png"));
        if !target.is_file() {
            let temporary = directory.join(format!("{}.png", uuid::Uuid::new_v4()));
            rgba.save_with_format(&temporary, ImageFormat::Png)
                .map_err(|error| format!("Unable to save background image: {error}"))?;
            if let Err(error) = fs::rename(&temporary, &target) {
                let _ = fs::remove_file(&temporary);
                // A concurrent import of the same file may already have
                // committed the exact same managed image.
                if !target.is_file() {
                    return Err(format!("Unable to store background image: {error}"));
                }
            }
        }
        target
    } else {
        path.to_path_buf()
    };
    Ok(LoadedImage {
        path: durable_path,
        image: ColorImage::from_rgba_unmultiplied(
            [rgba.width() as usize, rgba.height() as usize],
            rgba.as_raw(),
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_cover_centers_landscape_and_portrait_without_stretching() {
        for (image, viewport, expected) in [
            (
                egui::vec2(400.0, 200.0),
                egui::vec2(100.0, 100.0),
                Rect::from_min_max(egui::pos2(0.25, 0.0), egui::pos2(0.75, 1.0)),
            ),
            (
                egui::vec2(200.0, 400.0),
                egui::vec2(100.0, 100.0),
                Rect::from_min_max(egui::pos2(0.0, 0.25), egui::pos2(1.0, 0.75)),
            ),
            (
                egui::vec2(600.0, 400.0),
                egui::vec2(900.0, 600.0),
                Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            ),
        ] {
            let uv = cover_uv(image, viewport).unwrap();
            assert_eq!(uv, expected);
            assert_eq!(uv.center(), egui::pos2(0.5, 0.5));
            let sampled = uv.size() * image;
            assert!((sampled.x / sampled.y - viewport.x / viewport.y).abs() < 0.0001);
        }
        assert!(cover_uv(Vec2::ZERO, Vec2::splat(100.0)).is_none());
        assert!(cover_uv(Vec2::splat(100.0), egui::vec2(f32::NAN, 100.0)).is_none());
    }

    #[test]
    fn background_import_survives_source_removal_and_limits_texture_size() {
        let directory =
            std::env::temp_dir().join(format!("xrtranslate-background-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        let source = directory.join("source.png");
        image::RgbaImage::from_pixel(80, 40, image::Rgba([30, 60, 90, 255]))
            .save(&source)
            .unwrap();
        let imported = load_image(&source, Some(&directory), 32).unwrap();
        assert_eq!(imported.image.size, [32, 16]);
        assert!(
            imported
                .path
                .starts_with(directory.join("runtime/backgrounds"))
        );
        fs::remove_file(&source).unwrap();
        let reloaded = load_image(&imported.path, None, 32).unwrap();
        assert_eq!(reloaded.image.size, imported.image.size);
        assert_eq!(reloaded.image.pixels, imported.image.pixels);
        fs::write(&source, b"not an image").unwrap();
        assert!(load_image(&source, None, 32).is_err());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn background_failed_replacement_preserves_current_image_and_reset_discards_pending_load() {
        let ctx = egui::Context::default();
        let texture = ctx.load_texture(
            "previous-background",
            ColorImage::filled([1, 1], Color32::WHITE),
            egui::TextureOptions::LINEAR,
        );
        let texture_id = texture.id();
        let settings = BackgroundSettings {
            image_path: Some(PathBuf::from("previous.png")),
            ..Default::default()
        };
        let (sender, receiver) = mpsc::channel();
        let mut background = BackgroundImage {
            texture: Some(texture),
            attempted_path: settings.image_path.clone(),
            pending: Some(receiver),
        };
        background.ensure_loaded(&settings, &ctx);
        assert!(background.pending.is_some());
        sender.send(Err("invalid replacement".into())).unwrap();
        assert!(background.poll(&ctx).unwrap().is_err());
        assert_eq!(background.texture.as_ref().unwrap().id(), texture_id);
        assert_eq!(background.attempted_path, settings.image_path);
        assert!(background.pending.is_none());

        let (sender, receiver) = mpsc::channel();
        background.pending = Some(receiver);
        background.reset();
        assert!(!background.is_loaded());
        assert!(sender.send(Err("stale worker".into())).is_err());
        assert!(background.poll(&ctx).is_none());
    }
}
