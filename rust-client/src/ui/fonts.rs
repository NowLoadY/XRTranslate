//! Host UI font fallback configuration.
//!
//! Keep font discovery in the desktop host. Domain and plugin renderers use
//! egui's normal proportional/monospace families and do not need to know which
//! operating-system font supplies a particular script.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui;

struct SystemFont {
    name: &'static str,
    file_name: &'static str,
    purpose: &'static str,
}

// Preserve the existing CJK fallback order so adding script coverage does not
// change the established UI appearance. Segoe UI supplies Vietnamese Latin
// Extended glyphs and Nirmala UI supplies Devanagari/Hindi glyphs on Windows.
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

pub fn configure_multilingual_fonts(ctx: &egui::Context) {
    let mut definitions = egui::FontDefinitions::default();
    let mut loaded = Vec::with_capacity(WINDOWS_FONTS.len());
    let font_directory = windows_font_directory();

    for font in WINDOWS_FONTS {
        let path = font_directory.join(font.file_name);
        match std::fs::read(&path) {
            Ok(bytes) => {
                definitions.font_data.insert(
                    font.name.into(),
                    Arc::new(egui::FontData::from_owned(bytes)),
                );
                loaded.push(font.name);
            }
            Err(error) => log::warn!(
                "{} UI font not found at {}: {error}",
                font.purpose,
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

fn windows_font_directory() -> PathBuf {
    std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(r"C:\Windows").to_owned())
        .join("Fonts")
}

#[cfg(test)]
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

    #[cfg(target_os = "windows")]
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
