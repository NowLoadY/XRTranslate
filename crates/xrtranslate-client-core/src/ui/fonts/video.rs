//! The editor and libass resolve the same installed face. Oblique lettering
//! uses a regular sans base and horizontal condensation on both renderers.
use eframe::egui;
use std::{
    path::PathBuf,
    sync::{Arc, OnceLock},
};
mod weight;

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum VideoFont {
    Sans,
    Serif,
    Italic,
}

impl VideoFont {
    pub fn from_name(name: &str) -> Self {
        match name {
            "serif" => Self::Serif,
            "italic" => Self::Italic,
            _ => Self::Sans,
        }
    }
    pub fn width_factor(self) -> f32 {
        if self == Self::Italic { 0.84 } else { 1. }
    }
    pub fn family(self) -> egui::FontFamily {
        self.weighted_family(false)
    }
    pub fn weighted_family(self, bold: bool) -> egui::FontFamily {
        egui::FontFamily::Name(
            match (self, bold) {
                (Self::Serif, false) => "video-serif",
                (Self::Serif, true) => "video-serif-bold",
                (_, false) => "video-sans",
                (_, true) => "video-sans-bold",
            }
            .into(),
        )
    }
    fn face(self) -> Option<&'static Installed> {
        static SANS: OnceLock<Option<Installed>> = OnceLock::new();
        static SERIF: OnceLock<Option<Installed>> = OnceLock::new();
        if self == Self::Serif {
            SERIF
                .get_or_init(|| installed(true))
                .as_ref()
                .or_else(|| Self::Sans.face())
        } else {
            SANS.get_or_init(|| installed(false)).as_ref()
        }
    }
    pub fn ass_name(self, fallback: &str) -> &str {
        self.face().map_or(fallback, |face| face.name.as_str())
    }
}

struct Installed {
    name: String,
    path: PathBuf,
    index: u32,
}

#[cfg(any(windows, target_os = "android"))]
fn installed(serif: bool) -> Option<Installed> {
    #[cfg(windows)]
    let (directory, candidates): (_, &[(&str, &str, u32)]) = (
        super::windows_font_directory(),
        if serif {
            &[
                ("SimSun", "simsun.ttc", 0),
                ("Noto Serif SC", "NotoSerifSC-VF.ttf", 0),
                ("MingLiU", "mingliu.ttc", 0),
            ]
        } else {
            &[
                ("Microsoft YaHei", "msyh.ttc", 0),
                ("Microsoft JhengHei", "msjh.ttc", 0),
                ("Arial", "arial.ttf", 0),
            ]
        },
    );
    #[cfg(target_os = "android")]
    let (directory, candidates): (PathBuf, &[(&str, &str, u32)]) = (
        "/system/fonts".into(),
        if serif {
            &[("Noto Serif CJK SC", "NotoSerifCJK-Regular.ttc", 2)]
        } else {
            &[
                ("Noto Sans CJK SC", "NotoSansCJK-Regular.ttc", 2),
                ("Noto Sans CJK SC", "NotoSansCJK-VF.ttc", 2),
                ("Roboto", "Roboto-Regular.ttf", 0),
            ]
        },
    );
    candidates.iter().find_map(|(name, file, index)| {
        let path = directory.join(file);
        path.is_file().then(|| Installed {
            name: (*name).into(),
            path,
            index: *index,
        })
    })
}

#[cfg(not(any(windows, target_os = "android")))]
fn installed(serif: bool) -> Option<Installed> {
    let family = if serif {
        "serif:lang=zh"
    } else {
        "sans-serif:lang=zh"
    };
    let output = std::process::Command::new("fc-match")
        .args(["-f", "%{family[0]}\t%{file}\t%{index}", "--", family])
        .output()
        .ok()?;
    let description = String::from_utf8(output.stdout).ok()?;
    let mut parts = description.trim().split('\t');
    Some(Installed {
        name: parts.next()?.into(),
        path: parts.next()?.into(),
        index: parts.next()?.parse().ok()?,
    })
}

pub(super) fn install(definitions: &mut egui::FontDefinitions, ctx: &egui::Context) {
    for style in [VideoFont::Sans, VideoFont::Serif] {
        let mut fallbacks = definitions.families[&egui::FontFamily::Proportional].clone();
        let mut bold_fallbacks = definitions.families[&super::bold_family()].clone();
        if let Some(face) = style.face() {
            let key = format!("video:{}", face.name);
            if !definitions.font_data.contains_key(&key)
                && let Ok(bytes) = std::fs::read(&face.path)
            {
                let mut data = egui::FontData::from_owned(bytes);
                data.index = face.index;
                definitions.font_data.insert(key.clone(), Arc::new(data));
            }
            if let Some(regular) = definitions.font_data.get(&key).cloned() {
                let bold_key = format!("{key}:bold");
                if let Some(data) = weight::real_face(face, &regular) {
                    definitions
                        .font_data
                        .insert(bold_key.clone(), Arc::new(data));
                    bold_fallbacks.insert(0, bold_key);
                } else {
                    // Retain this face's shaping metrics and use the expanded coverage
                    // only for its otherwise unavailable bold glyphs.
                    weight::install_synthetic(ctx, style.weighted_family(true), regular);
                    bold_fallbacks.insert(0, key.clone());
                }
                fallbacks.insert(0, key);
            }
        }
        definitions.families.insert(style.family(), fallbacks);
        definitions
            .families
            .insert(style.weighted_family(true), bold_fallbacks);
    }
}
