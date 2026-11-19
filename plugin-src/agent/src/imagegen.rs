use base64::{Engine, engine::general_purpose::STANDARD};
use image::{DynamicImage, ImageFormat, ImageReader};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    fs,
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
};
use ui_director::{Tool, ToolCall, arguments};
use xrtranslate_protocol::image::{
    AgentImageRequest, ImageAction, ImageQuality, MAX_ENCODED_BYTES, MAX_IMAGES, MAX_INPUT_BYTES,
};

pub const TOOL: &str = "image.generate";

pub fn tool() -> Tool {
    Tool::new(
        TOOL,
        "Generate one PNG or edit local images with the configured OpenAI Agent. Read the imagegen skill for prompting, preserving alpha and importing the resulting path.",
        json!({
            "workspace":{"type":"string"},
            "prompt":{"type":"string","description":"Visual brief, exact copy, and what to keep or change. Label ordered inputs by role."},
            "images":{"type":"array","maxItems":MAX_IMAGES,"items":{"type":"string"},"description":"Absolute local PNG, JPEG or WebP paths including previous outputs. Put the edit target first."},
            "action":{"type":"string","enum":["generate","edit"],"description":"Defaults to edit with inputs, generate without. Choose generate for style references."},
            "transparent_background":{"type":"boolean","description":"Request genuine alpha (true) or an opaque backdrop (false). Edits preserve source transparency by default."},
            "size":{"type":"string","description":"auto or WIDTHxHEIGHT, e.g. 1024x1024, 1536x1024 or 2048x1152."},
            "quality":{"type":"string","enum":["auto","low","medium","high"]},
            "filename":{"type":"string","description":"Optional descriptive PNG basename for the asset bin."}
        }),
        &["prompt"],
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    prompt: String,
    #[serde(default)]
    images: Vec<PathBuf>,
    action: Option<ImageAction>,
    transparent_background: Option<bool>,
    size: Option<String>,
    quality: Option<ImageQuality>,
    filename: Option<String>,
    #[serde(rename = "workspace")]
    _workspace: Option<String>,
}

pub struct Output {
    file: fs::File,
    path: PathBuf,
    version: tempfile::TempDir,
    transparent: bool,
}

fn decode(bytes: &[u8], format: ImageFormat) -> Result<DynamicImage, String> {
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    reader
        .decode()
        .map_err(|error| format!("Cannot decode image: {error}"))
}

fn has_transparency(image: &DynamicImage) -> bool {
    image.color().has_alpha() && image.to_rgba8().pixels().any(|p| p[3] < 255)
}

pub fn prepare(call: &ToolCall, directory: &Path) -> Result<(AgentImageRequest, Output), String> {
    let args: Args = arguments(call)?;
    if args.images.len() > MAX_IMAGES {
        return Err("Supply at most eight local images".into());
    }
    let filename = args.filename.as_deref().unwrap_or("image.png");
    let stem = filename.strip_suffix(".png").unwrap_or(filename);
    if stem.trim().is_empty()
        || stem.len() > 96
        || stem
            .chars()
            .any(|c| c.is_control() || r#"<>:"/\|?*"#.contains(c))
        || stem.ends_with(['.', ' '])
    {
        return Err("Supply a short PNG basename, e.g. river-at-dawn.png".into());
    }
    let action = args.action.unwrap_or(if args.images.is_empty() {
        ImageAction::Generate
    } else {
        ImageAction::Edit
    });
    let mut background = args.transparent_background;
    let mut images = Vec::new();
    let mut remaining = MAX_INPUT_BYTES;
    for (index, path) in args.images.iter().enumerate() {
        if !path.is_absolute() || !path.is_file() {
            return Err(format!(
                "Input image {} needs an existing absolute local file path",
                index + 1
            ));
        }
        let file = fs::File::open(path).map_err(|error| error.to_string())?;
        let mut bytes = Vec::new();
        file.take(remaining as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        remaining = remaining
            .checked_sub(bytes.len())
            .ok_or("Input images exceed 20 MiB combined")?;
        let format = image::guess_format(&bytes).map_err(|_| "Input is not a recognized image")?;
        let mime = match format {
            ImageFormat::Png => "image/png",
            ImageFormat::Jpeg => "image/jpeg",
            ImageFormat::WebP => "image/webp",
            _ => return Err("Use PNG, JPEG or WebP inputs".into()),
        };
        let pixels = decode(&bytes, format)?;
        if index == 0
            && action == ImageAction::Edit
            && background.is_none()
            && has_transparency(&pixels)
        {
            background = Some(true);
        }
        images.push(format!("data:{mime};base64,{}", STANDARD.encode(bytes)));
    }
    let request = AgentImageRequest {
        prompt: args.prompt,
        images,
        action,
        transparent_background: background,
        size: args.size,
        quality: args.quality,
    };
    request.validate()?;
    // Reserve the destination before submitting the paid request. Dropping the
    // pending operation cleans it up on rejection or cancellation.
    fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    let directory = std::path::absolute(directory).map_err(|e| e.to_string())?;
    let version = tempfile::Builder::new()
        .prefix("asset-")
        .tempdir_in(directory)
        .map_err(|e| e.to_string())?;
    let path = version.path().join(format!("{stem}.png"));
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| e.to_string())?;
    Ok((
        request,
        Output {
            file,
            path,
            version,
            transparent: background == Some(true),
        },
    ))
}

impl Output {
    pub fn save(mut self, encoded: &str, review: bool) -> Result<(Value, Option<String>), String> {
        if encoded.len() > MAX_ENCODED_BYTES {
            return Err("Image exceeds the 20 MiB output limit".into());
        }
        let bytes = STANDARD
            .decode(encoded)
            .map_err(|_| "Image backend returned invalid base64")?;
        let pixels = decode(&bytes, ImageFormat::Png)?;
        let mut alpha = false;
        if pixels.color().has_alpha() {
            let mut visible = false;
            for p in pixels.to_rgba8().pixels() {
                visible |= p[3] > 0;
                alpha |= p[3] < 255;
                if visible && alpha {
                    break;
                }
            }
            if !visible {
                return Err("OpenAI returned a completely invisible PNG".into());
            }
        }
        let preview = if review {
            let rgb = pixels.thumbnail(768, 768).to_rgba8();
            // Composite onto a neutral checker so transparent edges are visible in JPEG.
            let flat = image::RgbImage::from_fn(rgb.width(), rgb.height(), |x, y| {
                image::Rgb(xrtranslate_protocol::image::review_rgb(x, y, rgb[(x, y)].0))
            });
            let mut jpg = Vec::new();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg, 83)
                .encode_image(&flat)
                .map_err(|e| e.to_string())?;
            Some(STANDARD.encode(jpg))
        } else {
            None
        };
        self.file.write_all(&bytes).map_err(|e| e.to_string())?;
        drop(self.file);
        let _ = self.version.keep();
        let mut receipt = json!({"path":self.path,"width":pixels.width(),"height":pixels.height(),"has_transparency":alpha});
        if self.transparent && !alpha {
            receipt["warning"] =
                json!("OpenAI returned an opaque PNG although transparency was requested.");
        }
        Ok((receipt, preview))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn edit_reuses_source_pixels_and_alpha_without_overwriting() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("input.png");
        let rgba = image::RgbaImage::from_fn(2, 1, |x, _| {
            image::Rgba([11, 22, 33, if x == 0 { 0 } else { 200 }])
        });
        rgba.save(&source).unwrap();
        let bytes = fs::read(&source).unwrap();
        let call = ToolCall {
            name: TOOL.into(),
            arguments: json!({"workspace":"one","prompt":"Recolor the cup","images":[source],"filename":"input.png"}),
        };
        let (request, output) = prepare(&call, dir.path()).unwrap();
        assert!(request.action == ImageAction::Edit);
        assert_eq!(request.transparent_background, Some(true));
        assert_eq!(
            request.images,
            [format!("data:image/png;base64,{}", STANDARD.encode(&bytes))]
        );
        let (receipt, preview) = output.save(&STANDARD.encode(&bytes), true).unwrap();
        assert_ne!(receipt["path"], json!(source));
        assert_eq!(receipt["has_transparency"], true);
        assert_eq!(fs::read(receipt["path"].as_str().unwrap()).unwrap(), bytes);
        assert!(
            STANDARD
                .decode(preview.unwrap())
                .unwrap()
                .starts_with(&[255, 216, 255])
        );
        let (_, cancelled) = prepare(&call, dir.path()).unwrap();
        let staging = cancelled.path.clone();
        drop(cancelled);
        assert!(!staging.exists());
        let invalid = ToolCall {
            name: TOOL.into(),
            arguments: json!({"prompt":"x","filename":"../escape.png"}),
        };
        assert!(prepare(&invalid, dir.path()).is_err());
    }
}
