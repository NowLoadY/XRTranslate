//! Text and banner presentation shared by host output plugins.
use crate::ui::{components, layout};
use eframe::egui;
use serde::{Deserialize, Serialize};

/// Bounded song metadata and the current lyric, supplied together by the host.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct LyricsText {
    pub title: String,
    pub lyric: String,
}

impl LyricsText {
    pub fn preferred(&self) -> &str {
        if self.lyric.is_empty() {
            &self.title
        } else {
            &self.lyric
        }
    }

    pub fn is_empty(&self) -> bool {
        self.title.is_empty() && self.lyric.is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageFormat {
    BilingualSourceFirst, // Source \n Target
    BilingualTargetFirst, // Target \n Source
    Inline,               // Source | Target
    TargetOnly,           // Target only
}

impl MessageFormat {
    pub fn label(&self, language: crate::i18n::UiLanguage) -> &'static str {
        crate::i18n::tr(
            language,
            match self {
                Self::BilingualSourceFirst => "Bilingual (Source → Target)",
                Self::BilingualTargetFirst => "Bilingual (Target → Source)",
                Self::Inline => "Single Line (Source | Target)",
                Self::TargetOnly => "Target Only",
            },
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BannerContentType {
    None,
    CustomText,
    SystemTime,
    CpuStatus,
    GpuStatus,
    Lyrics,
}

impl BannerContentType {
    pub fn label(&self, language: crate::i18n::UiLanguage) -> &'static str {
        match self {
            Self::None => crate::i18n::tr(language, "None (Disabled)"),
            Self::CustomText => crate::i18n::tr(language, "Custom Text"),
            Self::SystemTime => crate::i18n::tr(language, "System Time"),
            Self::CpuStatus => crate::i18n::tr(language, "CPU Usage"),
            Self::GpuStatus => crate::i18n::tr(language, "GPU Usage"),
            Self::Lyrics => crate::i18n::tr(language, "Lyrics"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BannerConfig {
    pub content_type: BannerContentType,
    pub custom_text: String,
    pub show_device_name: bool,
}

impl Default for BannerConfig {
    fn default() -> Self {
        Self {
            content_type: BannerContentType::None,
            custom_text: String::new(),
            show_device_name: false,
        }
    }
}

impl BannerConfig {
    pub fn render_text(&self, metrics: &super::system::SystemMetrics, lyric: &str) -> String {
        match self.content_type {
            BannerContentType::Lyrics => lyric.to_owned(),
            BannerContentType::None => String::new(),
            BannerContentType::CustomText => self.custom_text.trim().to_string(),
            BannerContentType::SystemTime => {
                if metrics.time_str.is_empty() {
                    String::new()
                } else {
                    format!("[{}]", metrics.time_str)
                }
            }
            BannerContentType::CpuStatus => {
                if metrics.cpu_name.is_empty() {
                    "[CPU unavailable]".to_string()
                } else if self.show_device_name {
                    format!("[{} {}%]", metrics.cpu_name, metrics.cpu_usage)
                } else {
                    format!("[CPU {}%]", metrics.cpu_usage)
                }
            }
            BannerContentType::GpuStatus => {
                if metrics.gpu_name.is_empty() {
                    "[GPU unavailable]".to_string()
                } else if self.show_device_name {
                    format!("[{} {}%]", metrics.gpu_name, metrics.gpu_usage)
                } else {
                    format!("[GPU {}%]", metrics.gpu_usage)
                }
            }
        }
    }
}

/// Format one message without transport-specific punctuation or length rules.
pub(crate) fn format_message<'a>(
    source: &str,
    target: &str,
    additional: impl IntoIterator<Item = &'a str>,
    speaker: &str,
    mode: MessageFormat,
) -> String {
    let (mut parts, separator) = message_parts(source, target, additional, mode);
    if let Some(first) = parts.first_mut()
        && let Some(speaker) = crate::presentation::speaker::compact_speaker_label(speaker)
    {
        first.insert_str(0, &format!("[{speaker}] "));
    }
    parts.join(separator)
}

pub(crate) fn message_parts(
    source: &str,
    target: &str,
    additional: impl IntoIterator<Item = impl AsRef<str>>,
    mode: MessageFormat,
) -> (Vec<String>, &'static str) {
    let source = source.trim();
    let target = target.trim();
    let show_source = !source.is_empty()
        && !target.is_empty()
        && source != target
        && mode != MessageFormat::TargetOnly;
    let mut parts = Vec::new();
    if show_source && mode != MessageFormat::BilingualTargetFirst {
        parts.push(source.to_owned());
    }
    let main = if target.is_empty() && mode != MessageFormat::TargetOnly {
        source
    } else {
        target
    };
    if !main.is_empty() {
        parts.push(main.to_owned());
    }
    parts.extend(additional.into_iter().filter_map(|text| {
        let text = text.as_ref().trim();
        (!text.is_empty()).then(|| text.to_owned())
    }));
    if show_source && mode == MessageFormat::BilingualTargetFirst {
        parts.push(source.to_owned());
    }
    (
        parts,
        if mode == MessageFormat::Inline {
            " | "
        } else {
            "\n"
        },
    )
}

pub(crate) fn render_banner_selector(
    ui: &mut egui::Ui,
    label: &str,
    combo_id: &str,
    banner: &mut BannerConfig,
    language: crate::i18n::UiLanguage,
    lyrics_available: bool,
) -> bool {
    let mut changed = false;
    if !lyrics_available && banner.content_type == BannerContentType::Lyrics {
        banner.content_type = BannerContentType::None;
        changed = true;
    }

    layout::flow_row(ui, |ui| {
        ui.label(
            egui::RichText::new(label)
                .color(crate::ui::theme::text_strong())
                .strong(),
        );

        let combo_resp =
            components::combobox_ui(ui, combo_id, banner.content_type.label(language), |ui| {
                let mut changed = false;
                for kind in [
                    BannerContentType::None,
                    BannerContentType::CustomText,
                    BannerContentType::SystemTime,
                    BannerContentType::CpuStatus,
                    BannerContentType::GpuStatus,
                    BannerContentType::Lyrics,
                ] {
                    if kind == BannerContentType::Lyrics && !lyrics_available {
                        continue;
                    }
                    changed |= ui
                        .selectable_value(&mut banner.content_type, kind, kind.label(language))
                        .changed();
                }
                changed
            });

        if combo_resp.inner.unwrap_or(false) {
            changed = true;
        }

        ui.add_space(8.0);

        match banner.content_type {
            BannerContentType::None => {}
            BannerContentType::CustomText => {
                if components::text_edit_ui(
                    ui,
                    combo_id,
                    egui::TextEdit::singleline(&mut banner.custom_text)
                        .hint_text("e.g. [AFK] or [CN/JP]")
                        .desired_width(150.0),
                )
                .changed()
                {
                    changed = true;
                }
            }
            BannerContentType::SystemTime | BannerContentType::Lyrics => {}
            BannerContentType::CpuStatus | BannerContentType::GpuStatus => {
                if components::checkbox(
                    ui,
                    &mut banner.show_device_name,
                    crate::i18n::tr(language, "Full Name"),
                )
                .changed()
                {
                    changed = true;
                }
            }
        }
    });

    changed
}
