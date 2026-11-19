//! One image operation on the configured Agent connection. Only pixels cross
//! the backend boundary. Local filenames stay with the application.
use serde::{Deserialize, Serialize};

pub const MAX_INPUT_BYTES: usize = 20 * 1024 * 1024;
pub const MAX_ENCODED_BYTES: usize = MAX_INPUT_BYTES.div_ceil(3) * 4;
pub const MAX_IMAGES: usize = 8;
pub const MAX_PROMPT_BYTES: usize = 32_000;

/// RGB representation for opaque review attachments. The PNG on disk retains
/// its alpha. Both Agent previews and bin contact sheets use this backdrop.
pub fn review_rgb(x: u32, y: u32, rgba: [u8; 4]) -> [u8; 3] {
    let bg = if (x / 12 + y / 12) % 2 == 0 { 220 } else { 175 };
    let alpha = u32::from(rgba[3]);
    std::array::from_fn(|i| ((u32::from(rgba[i]) * alpha + bg * (255 - alpha)) / 255) as u8)
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageAction {
    Generate,
    Edit,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageQuality {
    Auto,
    Low,
    Medium,
    High,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentImageRequest {
    pub prompt: String,
    pub action: ImageAction,
    /// Validated PNG, JPEG or WebP data URLs supplied by the application.
    pub images: Vec<String>,
    pub transparent_background: Option<bool>,
    pub size: Option<String>,
    pub quality: Option<ImageQuality>,
}

impl AgentImageRequest {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.prompt.trim().is_empty() || self.prompt.len() > MAX_PROMPT_BYTES {
            return Err("Supply an image prompt of 1 to 32000 bytes");
        }
        if self.images.len() > MAX_IMAGES
            || self.action == ImageAction::Edit && self.images.is_empty()
        {
            return Err("Use up to eight input images and provide an edit target for an edit");
        }
        let mut total = 0;
        for image in &self.images {
            let (prefix, data) = image.split_once(',').ok_or("Expected an image data URL")?;
            if !matches!(
                prefix,
                "data:image/png;base64" | "data:image/jpeg;base64" | "data:image/webp;base64"
            ) || data.is_empty()
            {
                return Err("Supply PNG, JPEG or WebP image data");
            }
            total += data.len();
        }
        if total > MAX_ENCODED_BYTES + 4 * MAX_IMAGES {
            return Err("Input images exceed 20 MiB combined");
        }
        if let Some(size) = self.size.as_deref().filter(|size| *size != "auto") {
            let pair = size
                .split_once('x')
                .and_then(|(w, h)| Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?)));
            if !pair.is_some_and(|(w, h)| {
                w > 0
                    && h > 0
                    && w.max(h) <= 3840
                    && w % 16 == 0
                    && h % 16 == 0
                    && w.max(h) <= 3 * w.min(h)
                    && (655_360..=8_294_400).contains(&(w * h))
            }) {
                return Err(
                    "Use auto or WIDTHxHEIGHT in multiples of 16, up to 3840 per side, a 3:1 aspect ratio and 655360–8294400 pixels",
                );
            }
        }
        Ok(())
    }
}
