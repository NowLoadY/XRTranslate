pub mod animation;
pub mod components;
pub mod fonts;
pub(crate) mod graph_canvas;
pub(crate) mod graph_editor;
pub(crate) mod graph_style;
pub mod layout;
pub mod modal;
pub mod organic_border;
pub mod pages;
pub mod theme;

use eframe::egui::{self, Align, Color32, CornerRadius, Frame, Layout, Margin, RichText, Stroke};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

pub use pages::onboarding::render_onboarding_fullscreen;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, Default)]
pub enum Page {
    #[default]
    Translation,
    Plugin(crate::plugins::PluginId),
    Settings,
    AudioStudio,
    PromptStudio,
}

impl Serialize for Page {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Translation => serializer.serialize_str("Translation"),
            Self::Settings => serializer.serialize_str("Settings"),
            Self::AudioStudio => serializer.serialize_str("AudioStudio"),
            Self::PromptStudio => serializer.serialize_str("PromptStudio"),
            Self::Plugin(id) => serializer.serialize_str(&format!("plugin:{}", id.as_str())),
        }
    }
}

impl<'de> Deserialize<'de> for Page {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match value.as_str() {
            "Translation" | "translation" => Ok(Self::Translation),
            "Settings" | "settings" => Ok(Self::Settings),
            "AudioStudio" => Ok(Self::AudioStudio),
            "PromptStudio" | "prompt_studio" | "prompt-studio" => Ok(Self::PromptStudio),
            // Compatibility with the former derived enum representation.
            "Osc" | "osc" => Ok(Self::Plugin(crate::plugins::PluginId::OSC)),
            "Meeting" | "meeting" => Ok(Self::Plugin(crate::plugins::PluginId::MEETING)),
            _ if value.starts_with("plugin:") => Ok(value
                .strip_prefix("plugin:")
                .and_then(crate::plugins::PluginId::parse)
                .map(Self::Plugin)
                // A settings file can outlive the build that supplied a
                // plugin. Preserve the rest of the settings and use a safe
                // core route instead of rejecting the whole document.
                .unwrap_or(Self::Translation)),
            _ => Err(de::Error::custom(format!("unknown page: {value}"))),
        }
    }
}

pub struct NavigationState {
    pub collapsed: bool,
    pub page: Page,
}

impl Default for NavigationState {
    fn default() -> Self {
        Self {
            collapsed: false,
            page: Page::Translation,
        }
    }
}

#[cfg(test)]
mod page_tests {
    use super::Page;
    use crate::plugins::PluginId;

    #[test]
    fn plugin_pages_have_stable_readable_serialization() {
        assert_eq!(
            serde_json::to_string(&Page::Plugin(PluginId::OSC)).unwrap(),
            r#""plugin:osc""#
        );
    }

    #[test]
    fn core_studio_pages_have_stable_readable_serialization() {
        assert_eq!(
            serde_json::to_string(&Page::AudioStudio).unwrap(),
            r#""AudioStudio""#
        );
        assert_eq!(
            serde_json::to_string(&Page::PromptStudio).unwrap(),
            r#""PromptStudio""#
        );
        assert_eq!(
            serde_json::from_str::<Page>(r#""AudioStudio""#).unwrap(),
            Page::AudioStudio
        );
    }

    #[test]
    fn legacy_plugin_page_names_still_load() {
        assert_eq!(
            serde_json::from_str::<Page>(r#""Osc""#).unwrap(),
            Page::Plugin(PluginId::OSC)
        );
        assert_eq!(
            serde_json::from_str::<Page>(r#""Meeting""#).unwrap(),
            Page::Plugin(PluginId::MEETING)
        );
    }
}

pub fn render_sidebar(
    ui: &mut egui::Ui,
    navigation: &mut NavigationState,
    plugin_preferences: &crate::plugins::PluginPreferences,
    modal_dialog: &mut modal::ModalDialog,
    first_run: &mut bool,
    onboarding_page: &mut usize,
    language: crate::i18n::UiLanguage,
    expand_factor: f32,
) {
    use egui::include_image;

    let icon_tr = include_image!("../../resources/icons/translation.svg");
    let icon_settings = include_image!("../../resources/icons/settings.svg");
    let icon_guide = include_image!("../../resources/icons/guide.svg");
    let icon_prompt = include_image!("../../resources/icons/prompt-studio.svg");
    let icon_audio = include_image!("../../resources/icons/audio-studio.svg");
    let icon_expand = include_image!("../../resources/icons/chevron-right.svg");
    let icon_collapse = include_image!("../../resources/icons/chevron-left.svg");

    ui.vertical(|ui| {
        ui.add_space(4.0);

        // Brand Header & Expand/Collapse Toggle
        ui.horizontal(|ui| {
            if expand_factor > 0.15 {
                let text_opacity = ((expand_factor - 0.15) / 0.85).clamp(0.0, 1.0);
                ui.scope(|ui| {
                    ui.set_opacity(text_opacity);
                    ui.label(
                        RichText::new("XRTranslate")
                            .size(16.0)
                            .color(theme::text_strong())
                            .strong(),
                    );
                });
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let (toggle_icon, tooltip) = if navigation.collapsed {
                    (icon_expand, "Expand sidebar")
                } else {
                    (icon_collapse, "Collapse sidebar")
                };

                let toggle_img = egui::Image::new(toggle_icon)
                    .fit_to_exact_size(egui::vec2(14.0, 14.0))
                    .tint(theme::text_strong());

                let toggle_btn = ui
                    .add(
                        egui::Button::image(toggle_img)
                            .min_size(egui::vec2(28.0, 28.0))
                            .corner_radius(CornerRadius::same(14)),
                    )
                    .on_hover_text(crate::i18n::tr(language, tooltip));

                if toggle_btn.clicked() {
                    navigation.collapsed = !navigation.collapsed;
                }
            });
        });

        ui.add_space(16.0);

        nav_item_animated(
            ui,
            navigation,
            Page::Translation,
            icon_tr,
            crate::i18n::tr(language, "Translation"),
            expand_factor,
        );
        ui.add_space(4.0);
        let mut plugin_descriptors = crate::plugins::PluginRegistry::builtin()
            .descriptors()
            .iter()
            .collect::<Vec<_>>();
        plugin_descriptors.sort_by_key(|descriptor| descriptor.navigation_order);
        for descriptor in &plugin_descriptors {
            if !plugin_preferences.is_enabled(descriptor.id) {
                continue;
            }
            nav_item_animated(
                ui,
                navigation,
                Page::Plugin(descriptor.id),
                descriptor.icon.image_source(),
                crate::i18n::tr(language, descriptor.title_key),
                expand_factor,
            );
            ui.add_space(4.0);
        }
        nav_item_animated(
            ui,
            navigation,
            Page::AudioStudio,
            icon_audio,
            crate::i18n::tr(language, "Audio Studio"),
            expand_factor,
        );
        ui.add_space(4.0);
        nav_item_animated(
            ui,
            navigation,
            Page::PromptStudio,
            icon_prompt,
            crate::i18n::tr(language, "Prompt Studio"),
            expand_factor,
        );
        ui.add_space(4.0);
        nav_item_animated(
            ui,
            navigation,
            Page::Settings,
            icon_settings,
            crate::i18n::tr(language, "Settings"),
            expand_factor,
        );

        ui.add_space(20.0);
        components::wavy_divider_black_shadow(ui);
        ui.add_space(12.0);

        guide_button_animated(
            ui,
            modal_dialog,
            language,
            icon_guide.clone(),
            expand_factor,
        );
        ui.add_space(4.0);
        if sidebar_text_button(
            ui,
            "sidebar_welcome_btn",
            "Welcome Page",
            icon_guide,
            language,
            expand_factor,
        ) {
            *onboarding_page = 0;
            *first_run = true;
        }
    });
}

fn sidebar_text_button(
    ui: &mut egui::Ui,
    id_source: &str,
    label: &'static str,
    icon: egui::ImageSource<'static>,
    language: crate::i18n::UiLanguage,
    expand_factor: f32,
) -> bool {
    let id = ui.make_persistent_id(id_source);
    let hovered = ui.memory(|memory| {
        memory
            .data
            .get_temp::<bool>(id.with("hover_state"))
            .unwrap_or(false)
    });
    let active = ui.memory(|memory| {
        memory
            .data
            .get_temp::<bool>(id.with("active_state"))
            .unwrap_or(false)
    });

    let hover = animation::AnimationSystem::hover(ui.ctx(), id.with("hover"), hovered);
    let active_factor = animation::AnimationSystem::active(ui.ctx(), id.with("active"), active);

    let bg_fill = Color32::TRANSPARENT;
    let foreground =
        animation::AnimationSystem::lerp_color(theme::text_strong(), theme::primary(), hover);
    let foreground =
        animation::AnimationSystem::lerp_color(foreground, theme::primary_dark(), active_factor);

    let response = Frame::new()
        .fill(bg_fill)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(
            (12.0 * expand_factor + 8.0 * (1.0 - expand_factor)).round() as i8,
            8,
        ))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                if expand_factor < 0.2 {
                    ui.add_space(((ui.available_width() - 16.0) / 2.0).max(0.0));
                }
                ui.add(
                    egui::Image::new(icon)
                        .fit_to_exact_size(egui::vec2(16.0, 16.0))
                        .tint(foreground),
                );
                if expand_factor > 0.1 {
                    ui.add_space(10.0 * ((expand_factor - 0.1) / 0.9).clamp(0.0, 1.0));
                    ui.label(
                        RichText::new(crate::i18n::tr(language, label))
                            .color(foreground)
                            .size(13.0),
                    );
                }
            });
        })
        .response
        .interact(egui::Sense::click());
    if expand_factor < 0.3 {
        response
            .clone()
            .on_hover_text(crate::i18n::tr(language, label));
    }
    ui.memory_mut(|memory| {
        memory
            .data
            .insert_temp(id.with("hover_state"), response.hovered());
        memory.data.insert_temp(
            id.with("active_state"),
            response.is_pointer_button_down_on(),
        );
    });
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.clicked()
}

fn open_guide_modal(modal_dialog: &mut modal::ModalDialog, language: crate::i18n::UiLanguage) {
    *modal_dialog = modal::ModalDialog::carousel(vec![
        modal::ModalPage::new(
            crate::i18n::tr(language, "Translation"),
            crate::i18n::tr(language, "Select audio and start."),
        ),
        modal::ModalPage::new(
            crate::i18n::tr(language, "VRChat OSC"),
            crate::i18n::tr(language, "Configure chatbox output."),
        ),
        modal::ModalPage::new(
            crate::i18n::tr(language, "Settings"),
            crate::i18n::tr(language, "Install llama.cpp and models."),
        ),
    ]);
}

fn nav_item_animated(
    ui: &mut egui::Ui,
    navigation: &mut NavigationState,
    page: Page,
    icon: egui::ImageSource<'static>,
    label: &str,
    expand_factor: f32,
) {
    let is_selected = navigation.page == page;
    let id = ui.make_persistent_id(label);

    let is_hovered = ui.memory(|m| {
        m.data
            .get_temp::<bool>(id.with("hover_state"))
            .unwrap_or(false)
    });
    let is_active = ui.memory(|m| {
        m.data
            .get_temp::<bool>(id.with("active_state"))
            .unwrap_or(false)
    });

    let hover_factor =
        animation::AnimationSystem::hover(ui.ctx(), id.with("hover"), is_hovered && !is_selected);
    let active_factor =
        animation::AnimationSystem::active(ui.ctx(), id.with("active"), is_active && !is_selected);
    let select_factor =
        animation::AnimationSystem::selection(ui.ctx(), id.with("select"), is_selected);

    let bg_fill = Color32::TRANSPARENT;
    let text_color = animation::AnimationSystem::lerp_color(
        theme::text_normal(),
        theme::primary(),
        hover_factor,
    );
    let text_color =
        animation::AnimationSystem::lerp_color(text_color, theme::primary_dark(), active_factor);
    let text_color =
        animation::AnimationSystem::lerp_color(text_color, theme::primary_dark(), select_factor);

    let inner_padding_x = (12.0 * expand_factor + 8.0 * (1.0 - expand_factor)).round();

    let frame_response = Frame::new()
        .fill(bg_fill)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(inner_padding_x as i8, 9))
        .stroke(Stroke::NONE)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                if expand_factor < 0.2 {
                    let indent = ((ui.available_width() - 16.0) / 2.0).max(0.0);
                    ui.add_space(indent);
                }

                if select_factor > 0.05 && expand_factor > 0.2 {
                    let (bar_rect, _) =
                        ui.allocate_exact_size(egui::vec2(3.0, 14.0), egui::Sense::hover());
                    let base = theme::primary_dark();
                    let bar_color = Color32::from_rgba_premultiplied(
                        base.r(),
                        base.g(),
                        base.b(),
                        (255.0 * select_factor) as u8,
                    );
                    ui.painter()
                        .rect_filled(bar_rect, CornerRadius::same(2), bar_color);
                    ui.add_space(3.0);
                }

                ui.add(
                    egui::Image::new(icon)
                        .fit_to_exact_size(egui::vec2(16.0, 16.0))
                        .tint(text_color),
                );

                if expand_factor > 0.1 {
                    let text_opacity = ((expand_factor - 0.1) / 0.9).clamp(0.0, 1.0);
                    ui.add_space(10.0 * text_opacity);
                    ui.scope(|ui| {
                        ui.set_opacity(text_opacity);
                        let mut rt = RichText::new(label).color(text_color).size(13.5);
                        if is_selected {
                            rt = rt.strong();
                        }
                        ui.label(rt);
                    });
                }
            });
        });

    let response = frame_response.response.interact(egui::Sense::click());

    if expand_factor < 0.3 {
        response.clone().on_hover_text(label);
    }

    ui.memory_mut(|m| {
        m.data
            .insert_temp(id.with("hover_state"), response.hovered());
        m.data.insert_temp(
            id.with("active_state"),
            response.is_pointer_button_down_on(),
        );
    });

    if response.clicked() {
        navigation.page = page;
    }

    if response.hovered() && !is_selected {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
}

fn guide_button_animated(
    ui: &mut egui::Ui,
    modal_dialog: &mut modal::ModalDialog,
    language: crate::i18n::UiLanguage,
    icon: egui::ImageSource<'static>,
    expand_factor: f32,
) {
    let guide_id = ui.make_persistent_id("sidebar_guide_btn");
    let is_hovered = ui.memory(|m| {
        m.data
            .get_temp::<bool>(guide_id.with("hover_state"))
            .unwrap_or(false)
    });
    let is_active = ui.memory(|m| {
        m.data
            .get_temp::<bool>(guide_id.with("active_state"))
            .unwrap_or(false)
    });

    let hover_factor =
        animation::AnimationSystem::hover(ui.ctx(), guide_id.with("hover"), is_hovered);
    let active_factor =
        animation::AnimationSystem::active(ui.ctx(), guide_id.with("active"), is_active);

    let bg_fill = Color32::TRANSPARENT;
    let foreground = animation::AnimationSystem::lerp_color(
        theme::text_strong(),
        theme::primary(),
        hover_factor,
    );
    let foreground =
        animation::AnimationSystem::lerp_color(foreground, theme::primary_dark(), active_factor);

    let inner_padding_x = (12.0 * expand_factor + 8.0 * (1.0 - expand_factor)).round();

    let frame_response = Frame::new()
        .fill(bg_fill)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(inner_padding_x as i8, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                if expand_factor < 0.2 {
                    let indent = ((ui.available_width() - 16.0) / 2.0).max(0.0);
                    ui.add_space(indent);
                }

                let guide_img = egui::Image::new(icon)
                    .fit_to_exact_size(egui::vec2(16.0, 16.0))
                    .tint(foreground);
                ui.add(guide_img);

                if expand_factor > 0.1 {
                    let text_opacity = ((expand_factor - 0.1) / 0.9).clamp(0.0, 1.0);
                    ui.add_space(10.0 * text_opacity);
                    ui.scope(|ui| {
                        ui.set_opacity(text_opacity);
                        ui.label(
                            RichText::new(crate::i18n::tr(language, "User Guide"))
                                .color(foreground)
                                .size(13.0),
                        );
                    });
                }
            });
        });

    let response = frame_response.response.interact(egui::Sense::click());

    if expand_factor < 0.3 {
        response
            .clone()
            .on_hover_text(crate::i18n::tr(language, "User Guide"));
    }

    ui.memory_mut(|m| {
        m.data
            .insert_temp(guide_id.with("hover_state"), response.hovered());
        m.data.insert_temp(
            guide_id.with("active_state"),
            response.is_pointer_button_down_on(),
        );
    });

    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    if response.clicked() {
        open_guide_modal(modal_dialog, language);
    }
}
