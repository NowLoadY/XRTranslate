use super::Installed;
use eframe::egui::{self, epaint::text::VariationCoords};
use std::path::PathBuf;

pub(super) fn real_face(
    face: &Installed,
    regular: &egui::FontData,
    weight: u16,
) -> Option<egui::FontData> {
    if let Some(axis) = regular
        .variation_axes()
        .into_iter()
        .find(|a| a.tag.to_string() == "wght" && a.range.max >= 700.)
    {
        let mut bold = regular.clone();
        bold.tweak.coords = VariationCoords::new([(
            b"wght",
            f32::from(weight).clamp(axis.range.min, axis.range.max),
        )]);
        return Some(bold);
    }
    if let Some((path, index)) = bold_file(face)
        && (path != face.path || index != face.index)
        && let Ok(bytes) = std::fs::read(path)
    {
        let mut bold = egui::FontData::from_owned(bytes);
        bold.index = index;
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
        "SourceHanSerifSC-Regular.otf" => "SourceHanSerifSC-Bold.otf",
        "SourceHanSansSC-Regular.otf" => "SourceHanSansSC-Bold.otf",
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
