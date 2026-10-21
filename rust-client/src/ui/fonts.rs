//! Host UI font fallback configuration.
//!
//! Keep font discovery in the desktop host. Domain and plugin renderers use
//! egui's proportional/monospace and shared bold families without knowing which
//! operating-system font supplies a particular script.

#[cfg(windows)]
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui;

/// Installed family names for renderers that resolve fonts through the system.
pub(crate) fn family_options() -> &'static [(String, String)] {
    static FAMILIES: std::sync::OnceLock<Vec<(String, String)>> = std::sync::OnceLock::new();
    FAMILIES.get_or_init(|| {
        let mut names = installed_families();
        names.sort_by_key(|name| name.to_lowercase());
        names.dedup();
        names.into_iter().map(|name| (name.clone(), name)).collect()
    })
}

#[cfg(windows)]
fn installed_families() -> Vec<String> {
    use windows::Win32::Graphics::DirectWrite::*;
    let mut result = Vec::new();
    let _ = (|| -> windows::core::Result<()> {
        unsafe {
            let factory: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let mut collection = None;
            factory.GetSystemFontCollection(&mut collection, false)?;
            if let Some(collection) = collection {
                for index in 0..collection.GetFontFamilyCount() {
                    let names = collection.GetFontFamily(index)?.GetFamilyNames()?;
                    let mut buffer = vec![0u16; names.GetStringLength(0)? as usize + 1];
                    names.GetString(0, &mut buffer)?;
                    result.push(String::from_utf16_lossy(&buffer[..buffer.len() - 1]));
                }
            }
            Ok(())
        }
    })();
    result
}

#[cfg(not(windows))]
fn installed_families() -> Vec<String> {
    std::process::Command::new("fc-list")
        .arg("--format=%{family}\\n")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .flat_map(|line| line.split(','))
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_else(|| vec!["sans-serif".into(), "serif".into(), "monospace".into()])
}

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

#[cfg(not(any(windows, target_os = "android")))]
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
    for bold in [false, true] {
        let mut loaded = Vec::new();
        for (name, path, index, purpose) in system_fonts(bold) {
            match std::fs::read(&path) {
                Ok(bytes) => {
                    let mut data = egui::FontData::from_owned(bytes);
                    data.index = index;
                    let name = if bold {
                        format!("{name}_bold")
                    } else {
                        name.to_owned()
                    };
                    if bold {
                        data.tweak.coords =
                            egui::epaint::text::VariationCoords::new([(b"wght", 750.0)]);
                    }
                    definitions.font_data.insert(name.clone(), Arc::new(data));
                    loaded.push(name);
                }
                Err(error) => {
                    log::warn!("{purpose} UI font not found at {}: {error}", path.display())
                }
            }
        }
        if bold {
            // Missing bold glyphs retain the application's normal multilingual fallbacks.
            loaded.extend(
                definitions.families[&egui::FontFamily::Proportional]
                    .iter()
                    .cloned(),
            );
            definitions.families.insert(bold_family(), loaded);
        } else {
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                let fallbacks = definitions.families.entry(family).or_default();
                fallbacks.splice(0..0, loaded.iter().cloned());
            }
        }
    }
    ctx.set_fonts(definitions);
}

pub fn bold_family() -> egui::FontFamily {
    egui::FontFamily::Name("bold".into())
}

#[cfg(windows)]
fn system_fonts(bold: bool) -> Vec<(&'static str, PathBuf, u32, &'static str)> {
    let directory = windows_font_directory();
    WINDOWS_FONTS
        .iter()
        .map(|font| {
            let file = if bold {
                match font.file_name {
                    "msyh.ttc" => "msyhbd.ttc",
                    "malgun.ttf" => "malgunbd.ttf",
                    "segoeui.ttf" => "segoeuib.ttf",
                    "Nirmala.ttc" => "NirmalaB.ttc",
                    file => file,
                }
            } else {
                font.file_name
            };
            (font.name, directory.join(file), 0, font.purpose)
        })
        .collect()
}

#[cfg(not(any(windows, target_os = "android")))]
fn system_fonts(bold: bool) -> Vec<(&'static str, std::path::PathBuf, u32, &'static str)> {
    FONTCONFIG_FONTS
        .iter()
        .filter_map(|&(name, family)| {
            let output = std::process::Command::new("fc-match")
                .args([
                    "-f",
                    "%{file}\t%{index}",
                    &if bold {
                        format!("{family}:style=Bold")
                    } else {
                        family.to_owned()
                    },
                ])
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

#[cfg(target_os = "android")]
fn system_fonts(bold: bool) -> Vec<(&'static str, std::path::PathBuf, u32, &'static str)> {
    [
        (
            "roboto",
            if bold {
                &["Roboto-Bold.ttf", "Roboto.ttf"][..]
            } else {
                &["Roboto-Regular.ttf", "Roboto.ttf"][..]
            },
        ),
        (
            "noto_cjk",
            if bold {
                &[
                    "NotoSansCJK-Bold.ttc",
                    "NotoSansCJK-VF.ttc",
                    "NotoSansCJK-Regular.ttc",
                ][..]
            } else {
                &["NotoSansCJK-Regular.ttc", "NotoSansCJK-VF.ttc"][..]
            },
        ),
        (
            "noto_devanagari",
            if bold {
                &[
                    "NotoSansDevanagari-Bold.ttf",
                    "NotoSansDevanagari-VF.ttf",
                    "NotoSansDevanagari-Regular.ttf",
                ][..]
            } else {
                &[
                    "NotoSansDevanagari-Regular.ttf",
                    "NotoSansDevanagari-VF.ttf",
                ][..]
            },
        ),
        (
            "noto_arabic",
            if bold {
                &[
                    "NotoSansArabic-Bold.ttf",
                    "NotoSansArabic-VF.ttf",
                    "NotoSansArabic-Regular.ttf",
                ][..]
            } else {
                &["NotoSansArabic-Regular.ttf", "NotoSansArabic-VF.ttf"][..]
            },
        ),
        ("noto_symbols", &["NotoSansSymbols2-Regular.ttf"][..]),
    ]
    .into_iter()
    .filter_map(|(name, candidates)| {
        candidates
            .iter()
            .map(|file| std::path::Path::new("/system/fonts").join(file))
            .find(|path| path.is_file())
            .map(|path| (name, path, 0, name))
    })
    .collect()
}
