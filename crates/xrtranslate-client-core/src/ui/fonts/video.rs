//! The editor and libass resolve the same installed type family and display weight.
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
    /// ASS and the editor request the same display weight; static families use their real bold face.
    pub const DISPLAY_WEIGHT: u16 = 800;
    /// Weight coordinate shared by the preview and exported video fonts.
    pub fn weight(self, bold: bool) -> u16 {
        match (self, bold) {
            (_, false) => 400,
            (Self::Serif, true) => 900,
            (_, true) => Self::DISPLAY_WEIGHT,
        }
    }
    /// All video faces retain the font designer's natural advance.
    pub fn width_factor(self) -> f32 {
        1.0
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
    pub fn source(self) -> Option<(&'static std::path::Path, u32)> {
        self.face().map(|f| (f.path.as_path(), f.index))
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
                ("Noto Serif SC", "NotoSerifSC-VF.ttf", 0),
                ("Source Han Serif SC", "SourceHanSerifSC-VF.ttf", 0),
                ("Source Han Serif SC", "SourceHanSerifSC-VF.otf", 0),
                ("Source Han Serif SC", "SourceHanSerifSC-Regular.otf", 0),
            ]
        } else {
            &[
                ("Noto Sans SC", "NotoSansSC-VF.ttf", 0),
                ("Source Han Sans SC", "SourceHanSansSC-VF.ttf", 0),
                ("Source Han Sans SC", "SourceHanSansSC-VF.otf", 0),
                ("Source Han Sans SC", "SourceHanSansSC-Regular.otf", 0),
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
    #[cfg(windows)]
    let user_fonts =
        std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Microsoft/Windows/Fonts"));
    #[cfg(not(windows))]
    let user_fonts: Option<PathBuf> = None;
    candidates.iter().find_map(|(name, file, index)| {
        [Some(&directory), user_fonts.as_ref()]
            .into_iter()
            .flatten()
            .map(|dir| dir.join(file))
            .find(|path| {
                if !path.is_file() {
                    return false;
                }
                #[cfg(windows)]
                if let Some(family) = file.strip_suffix("-Regular.otf") {
                    return path.with_file_name(format!("{family}-Bold.otf")).is_file();
                }
                true
            })
            .map(|path| Installed {
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

pub(super) fn install(definitions: &mut egui::FontDefinitions) {
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
                if data
                    .variation_axes()
                    .iter()
                    .any(|a| a.tag.to_string() == "wght")
                {
                    data.tweak.coords = egui::epaint::text::VariationCoords::new([(b"wght", 400.)]);
                }
                definitions.font_data.insert(key.clone(), Arc::new(data));
            }
            if let Some(regular) = definitions.font_data.get(&key).cloned() {
                let weight = style.weight(true);
                let bold_key = format!("{key}:{weight}");
                if let Some(data) = weight::real_face(face, &regular, weight) {
                    definitions
                        .font_data
                        .insert(bold_key.clone(), Arc::new(data));
                    bold_fallbacks.insert(0, bold_key);
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
