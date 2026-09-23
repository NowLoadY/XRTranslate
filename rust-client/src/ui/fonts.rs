//! Host UI font fallback configuration.
//!
//! Keep font discovery in the desktop host. Domain and plugin renderers use
//! egui's normal proportional/monospace families and do not need to know which
//! operating-system font supplies a particular script.

#[cfg(windows)]
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui;

#[cfg(windows)]
struct SystemFont {
    name: &'static str,
    file_name: &'static str,
    purpose: &'static str,
}

// Preserve the existing CJK fallback order so adding script coverage does not
// change the established UI appearance. Segoe UI supplies Vietnamese Latin
// Extended glyphs and Nirmala UI supplies Devanagari/Hindi glyphs on Windows.
#[cfg(windows)]
const WINDOWS_FONTS: &[SystemFont] = &[
    SystemFont {
        name: "microsoft_yahei",
        file_name: "msyh.ttc",
        purpose: "Chinese",
    },
    SystemFont {
        name: "malgun_gothic",
        file_name: "malgun.ttf",
        purpose: "Korean",
    },
    SystemFont {
        name: "segoe_ui",
        file_name: "segoeui.ttf",
        purpose: "Latin Extended and Vietnamese",
    },
    SystemFont {
        name: "nirmala_ui",
        file_name: "Nirmala.ttc",
        purpose: "Indic and Devanagari",
    },
    SystemFont {
        name: "cascadia_code",
        file_name: "CascadiaCode.ttf",
        purpose: "Emoji and Modern Pictographs",
    },
    SystemFont {
        name: "cascadia_mono",
        file_name: "CascadiaMono.ttf",
        purpose: "Monospace Symbols",
    },
    SystemFont {
        name: "segoe_ui_symbol",
        file_name: "seguisym.ttf",
        purpose: "Symbols and Icons",
    },
];

#[cfg(not(windows))]
const FONTCONFIG_FONTS: &[(&str, &str)] = &[
    ("noto_sans", "Noto Sans"),
    ("noto_cjk_sc", "Noto Sans CJK SC"),
    ("noto_cjk_kr", "Noto Sans CJK KR"),
    ("noto_devanagari", "Noto Sans Devanagari"),
    ("noto_symbols", "Noto Sans Symbols 2"),
    ("noto_monospace", "DejaVu Sans Mono"),
];

pub fn configure_multilingual_fonts(ctx: &egui::Context) {
    let mut definitions = egui::FontDefinitions::default();
    let mut loaded = Vec::new();
    for (name, path, index, purpose) in system_fonts() {
        match std::fs::read(&path) {
            Ok(bytes) => {
                let mut data = egui::FontData::from_owned(bytes);
                data.index = index;
                definitions.font_data.insert(name.into(), Arc::new(data));
                loaded.push(name);
            }
            Err(error) => log::warn!(
                "{} UI font not found at {}: {error}",
                purpose,
                path.display()
            ),
        }
    }

    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        let fallbacks = definitions.families.entry(family).or_default();
        for (position, name) in loaded.iter().enumerate() {
            fallbacks.insert(position, (*name).into());
        }
    }
    ctx.set_fonts(definitions);
}

#[cfg(windows)]
fn system_fonts() -> Vec<(&'static str, PathBuf, u32, &'static str)> {
    let directory = windows_font_directory();
    WINDOWS_FONTS
        .iter()
        .map(|font| (font.name, directory.join(font.file_name), 0, font.purpose))
        .collect()
}

#[cfg(not(windows))]
fn system_fonts() -> Vec<(&'static str, std::path::PathBuf, u32, &'static str)> {
    FONTCONFIG_FONTS
        .iter()
        .filter_map(|&(name, family)| {
            let output = std::process::Command::new("fc-match")
                .args(["-f", "%{file}\t%{index}", family])
                .output()
                .ok()?;
            let match_text = String::from_utf8(output.stdout).ok()?;
            let (path, index) = match_text.trim().split_once('\t')?;
            Some((name, path.into(), index.parse().ok()?, family))
        })
        .collect()
}

#[cfg(windows)]
fn windows_font_directory() -> PathBuf {
    std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(r"C:\Windows").to_owned())
        .join("Fonts")
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn preserves_existing_fonts_before_new_script_fallbacks() {
        assert_eq!(
            WINDOWS_FONTS
                .iter()
                .map(|font| font.name)
                .collect::<Vec<_>>(),
            [
                "microsoft_yahei",
                "malgun_gothic",
                "segoe_ui",
                "nirmala_ui",
                "cascadia_code",
                "cascadia_mono",
                "segoe_ui_symbol",
            ]
        );
    }

    #[test]
    fn windows_fallbacks_cover_vietnamese_and_hindi_text() {
        let ctx = egui::Context::default();
        configure_multilingual_fonts(&ctx);

        let mut coverage = None;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            coverage = Some(ui.ctx().fonts_mut(|fonts| {
                let font = egui::FontId::proportional(14.0);
                (
                    fonts.has_glyphs(&font, "Tiếng Việt"),
                    fonts.has_glyphs(&font, "नमस्ते दुनिया"),
                    fonts.has_glyphs(&font, "🎤 🔊 💬"),
                )
            }));
        });
        output.textures_delta.clear();
        assert_eq!(coverage, Some((true, true, true)));
    }
}
