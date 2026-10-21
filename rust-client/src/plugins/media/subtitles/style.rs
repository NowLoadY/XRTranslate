use serde::{Deserialize, Serialize};

/// Shared by playback and export, using a 720-line reference frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SubtitleStyle {
    pub font: String,
    pub size: f32,
    pub color: [u8; 3],
}

impl Default for SubtitleStyle {
    fn default() -> Self {
        Self {
            font: if cfg!(windows) {
                "Microsoft YaHei"
            } else {
                "sans-serif"
            }
            .into(),
            size: 48.0,
            color: [255; 3],
        }
    }
}

impl SubtitleStyle {
    pub fn normalize(&mut self) {
        self.size = if self.size.is_finite() {
            self.size.clamp(12.0, 72.0)
        } else {
            Self::default().size
        };
        self.font.retain(|c| !c.is_control());
        if self.font.trim().is_empty() {
            self.font = Self::default().font;
        }
    }

    #[cfg(feature = "mpv")]
    pub fn rgb(&self) -> String {
        format!(
            "#{:02X}{:02X}{:02X}",
            self.color[0], self.color[1], self.color[2]
        )
    }
}
