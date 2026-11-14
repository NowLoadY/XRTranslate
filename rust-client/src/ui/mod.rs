pub(crate) mod background;
pub(crate) mod companion;
pub(crate) mod tutorial;
pub use xrtranslate_client_core::ui::{
    animation, fonts, layout, modal, notifications, organic_border, organic_line, theme,
    virtual_list,
};
pub mod components;
pub(crate) mod graph_canvas;
pub(crate) mod graph_editor;
pub(crate) mod graph_style;
pub(crate) mod model_comparison;
pub mod pages;
pub use xrtranslate_client_core::ui::previews;

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
    TtsCenter,
    CorpusStudio,
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
            Self::TtsCenter => serializer.serialize_str("TtsCenter"),
            Self::PromptStudio => serializer.serialize_str("PromptStudio"),
            Self::CorpusStudio => serializer.serialize_str("CorpusStudio"),
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
            "GamePlugins" | "game_plugins" => Ok(Self::Plugin(crate::plugins::PluginId::AVATAR)),
            "Settings" | "settings" => Ok(Self::Settings),
            "AudioStudio" => Ok(Self::AudioStudio),
            "PromptStudio" | "prompt_studio" | "prompt-studio" => Ok(Self::PromptStudio),
            "TtsCenter" => Ok(Self::TtsCenter),
            "CorpusStudio" => Ok(Self::CorpusStudio),
            // Compatibility with the former derived enum representation.
            "Osc" | "osc" => Ok(Self::Plugin(crate::plugins::PluginId::OSC)),
            "Meeting" | "meeting" => Ok(Self::Plugin(crate::plugins::PluginId::MEETING)),
            "VrOverlay" | "vr_overlay" => Ok(Self::Plugin(crate::plugins::PluginId::VR_OVERLAY)),
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
    history: Vec<Page>,
}

impl Default for NavigationState {
    fn default() -> Self {
        Self::new(Page::Translation, false)
    }
}

impl NavigationState {
    pub fn new(page: Page, collapsed: bool) -> Self {
        Self {
            collapsed,
            page,
            history: Vec::new(),
        }
    }

    pub fn open(&mut self, page: Page) {
        if self.page == page {
            return;
        }
        if page == Page::Translation {
            self.history.clear();
        } else if let Some(index) = self.history.iter().position(|previous| *previous == page) {
            self.history.truncate(index);
        } else {
            self.history.push(self.page);
        }
        self.page = page;
    }

    pub fn back(&mut self) {
        self.page = self.history.pop().unwrap_or(Page::Translation);
    }
}

pub fn render_sidebar(
    ui: &mut egui::Ui,
    navigation: &mut NavigationState,
    plugin_pages: &crate::plugins::PluginPages,
    tts_configured: bool,
    first_run: &mut bool,
    onboarding_page: &mut usize,
    language: crate::i18n::UiLanguage,
    expand_factor: f32,
) {
    use egui::include_image;

    let icon_guide =
        include_image!("../../../crates/xrtranslate-client-core/resources/icons/guide.svg");
    let icon_expand =
        include_image!("../../../crates/xrtranslate-client-core/resources/icons/chevron-right.svg");
    let icon_collapse =
        include_image!("../../../crates/xrtranslate-client-core/resources/icons/chevron-left.svg");

    let compact_height = ui.available_height() < 500.0;

    ui.vertical(|ui| {
        ui.add_space(if compact_height { 2.0 } else { 4.0 });

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

            let (toggle_icon, tooltip) = if navigation.collapsed {
                (icon_expand, "Expand sidebar")
            } else {
                (icon_collapse, "Collapse sidebar")
            };

            let (toggle_size, btn_wh) = if compact_height {
                (12.0, 24.0)
            } else {
                (14.0, 28.0)
            };

            let toggle_img = egui::Image::new(toggle_icon)
                .fit_to_exact_size(egui::vec2(toggle_size, toggle_size))
                .tint(theme::text_strong());

            let layout = if expand_factor > 0.15 {
                Layout::right_to_left(Align::Center)
            } else {
                Layout::top_down(Align::Center)
            };

            ui.with_layout(layout, |ui| {
                if expand_factor <= 0.15 {
                    ui.set_width(ui.available_width());
                }
                let toggle_btn = ui
                    .add(
                        egui::Button::image(toggle_img)
                            .min_size(egui::vec2(btn_wh, btn_wh))
                            .corner_radius(CornerRadius::same(btn_wh as u8 / 2)),
                    )
                    .on_hover_text(crate::i18n::tr(language, tooltip));

                if toggle_btn.clicked() {
                    navigation.collapsed = !navigation.collapsed;
                }
            });
        });

        ui.add_space(if compact_height { 6.0 } else { 16.0 });

        egui::ScrollArea::vertical()
            .id_salt("sidebar_scroll")
            .auto_shrink([false, false])
            .min_scrolled_height(0.0)
            .show(ui, |ui| {
                let mut previous_group = "";
                for entry in navigation_entries(plugin_pages, tts_configured) {
                    let group = match entry.page {
                        Page::Plugin(id) => plugin_pages
                            .installed_pages
                            .iter()
                            .find(|page| page.id == id)
                            .map_or("", |page| page.group.as_str()),
                        Page::AudioStudio => "core-tools",
                        _ => previous_group,
                    };
                    if group != previous_group {
                        ui.add_space(if compact_height { 3.0 } else { 8.0 });
                        components::wavy_divider_black_shadow(ui);
                        ui.add_space(if compact_height { 3.0 } else { 8.0 });
                    }
                    previous_group = group;
                    nav_item_animated(
                        ui,
                        navigation,
                        entry.page,
                        entry.icon.clone(),
                        entry.label(language),
                        expand_factor,
                        compact_height,
                    );
                    if entry.page != Page::Settings {
                        ui.add_space(if compact_height { 1.5 } else { 4.0 });
                    }
                }

                ui.add_space(if compact_height { 6.0 } else { 20.0 });
                components::wavy_divider_black_shadow(ui);
                ui.add_space(if compact_height { 4.0 } else { 12.0 });

                guide_button_animated(
                    ui,
                    language,
                    icon_guide.clone(),
                    expand_factor,
                    compact_height,
                );
                ui.add_space(if compact_height { 2.0 } else { 4.0 });
                if sidebar_text_button(
                    ui,
                    "sidebar_welcome_btn",
                    "Welcome Page",
                    icon_guide,
                    language,
                    expand_factor,
                    compact_height,
                ) {
                    *onboarding_page = 0;
                    *first_run = true;
                }
            });
    });
}

fn sidebar_text_button(
    ui: &mut egui::Ui,
    id_source: &str,
    label: &'static str,
    icon: egui::ImageSource<'static>,
    language: crate::i18n::UiLanguage,
    expand_factor: f32,
    compact_height: bool,
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

    let v_padding = if compact_height { 3 } else { 8 };
    let response = Frame::new()
        .fill(bg_fill)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(
            (12.0 * expand_factor + 8.0 * (1.0 - expand_factor)).round() as i8,
            v_padding,
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

fn nav_item_animated(
    ui: &mut egui::Ui,
    navigation: &mut NavigationState,
    page: Page,
    icon: egui::ImageSource<'static>,
    label: &str,
    expand_factor: f32,
    compact_height: bool,
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
    let v_padding = if compact_height { 3 } else { 9 };

    let frame_response = Frame::new()
        .fill(bg_fill)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(inner_padding_x as i8, v_padding))
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
                    if theme::is_hand_drawn(ui.ctx()) {
                        let top = egui::pos2(bar_rect.center().x, bar_rect.top() + 0.5);
                        let btm = egui::pos2(bar_rect.center().x, bar_rect.bottom() - 0.5);
                        organic_line::paint_hand_drawn_line(
                            ui.painter(),
                            id.with("sidebar_active_bar"),
                            top,
                            btm,
                            Stroke::new(2.4, bar_color),
                        );
                    } else {
                        ui.painter()
                            .rect_filled(bar_rect, CornerRadius::same(2), bar_color);
                    }
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
        navigation.open(page);
    }

    if response.hovered() && !is_selected {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
}

fn guide_button_animated(
    ui: &mut egui::Ui,
    language: crate::i18n::UiLanguage,
    icon: egui::ImageSource<'static>,
    expand_factor: f32,
    compact_height: bool,
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
    let v_padding = if compact_height { 3 } else { 8 };

    let frame_response = Frame::new()
        .fill(bg_fill)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(inner_padding_x as i8, v_padding))
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
    let response = ui_director::egui::Control::button(crate::i18n::tr(language, "User Guide"))
        .response(ui, response);

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
        tutorial::start(ui.ctx());
    }
}

struct NavigationEntry {
    page: Page,
    title: std::borrow::Cow<'static, str>,
    icon: egui::ImageSource<'static>,
}
impl NavigationEntry {
    fn icon(&self, selected: bool) -> egui::Image<'static> {
        let mut source = self.icon.clone();
        if selected
            && let egui::ImageSource::Bytes { uri, bytes } = &source
            && let Ok(svg) = std::str::from_utf8(bytes)
        {
            let [r, g, b, _] = theme::primary().to_array();
            let color = format!("#{r:02x}{g:02x}{b:02x}");
            source = egui::ImageSource::Bytes {
                uri: format!("{uri}/{color}.svg").into(),
                bytes: svg.replace("currentColor", &color).into_bytes().into(),
            };
        }
        egui::Image::new(source).fit_to_exact_size(egui::vec2(18.0, 18.0))
    }

    fn label(&self, language: crate::i18n::UiLanguage) -> &str {
        match &self.title {
            std::borrow::Cow::Borrowed(key) => crate::i18n::tr(language, key),
            std::borrow::Cow::Owned(label) => label,
        }
    }
}

/// The CLI uses the same routes and availability as the sidebar.
pub(crate) fn navigation_pages(
    plugins: &crate::plugins::PluginPages,
    tts_configured: bool,
    language: crate::i18n::UiLanguage,
) -> Vec<ui_director::egui::registry::Page> {
    let mut pages: Vec<_> = navigation_entries(plugins, tts_configured)
        .into_iter()
        .map(|entry| ui_director::egui::registry::Page {
            route: serde_json::to_value(entry.page)
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned(),
            title: entry.title.to_string(),
            label: entry.label(language).into(),
        })
        .collect();
    pages.extend(
        pages::onboarding::STEPS
            .iter()
            .enumerate()
            .map(|(step, title)| ui_director::egui::registry::Page {
                route: format!("onboarding:{step}"),
                title: format!("Welcome: {title}"),
                label: format!(
                    "{}: {}",
                    crate::i18n::tr(language, "Welcome Page"),
                    crate::i18n::tr(language, title)
                ),
            }),
    );
    pages
}

fn navigation_entries(
    plugins: &crate::plugins::PluginPages,
    tts_configured: bool,
) -> Vec<NavigationEntry> {
    use egui::include_image;
    let mut entries = vec![NavigationEntry {
        page: Page::Translation,
        title: "Translation".into(),
        icon: include_image!(
            "../../../crates/xrtranslate-client-core/resources/icons/translation.svg"
        ),
    }];
    let mut contributed = plugins.installed_pages.iter().map(|page|(page.order,NavigationEntry{
        page:Page::Plugin(page.id),title:page.title.clone().into(),icon:page.icon.as_ref().map_or_else(||include_image!("../../../crates/xrtranslate-client-core/resources/icons/settings.svg"),|(uri,bytes)|egui::ImageSource::Bytes{uri:uri.clone().into(),bytes:bytes.clone().into()}),
    })).collect::<Vec<_>>();
    contributed.sort_by_key(|(order, _)| *order);
    entries.extend(contributed.into_iter().map(|(_, entry)| entry));
    entries.push(NavigationEntry {
        page: Page::AudioStudio,
        title: "Audio Studio".into(),
        icon: include_image!(
            "../../../crates/xrtranslate-client-core/resources/icons/audio-studio.svg"
        ),
    });
    entries.push(NavigationEntry {
        page: Page::PromptStudio,
        title: "Prompt Studio".into(),
        icon: include_image!(
            "../../../crates/xrtranslate-client-core/resources/icons/prompt-studio.svg"
        ),
    });
    if tts_configured {
        entries.push(NavigationEntry {
            page: Page::TtsCenter,
            title: "TTS Center".into(),
            icon: include_image!(
                "../../../crates/xrtranslate-client-core/resources/icons/tts-center.svg"
            ),
        });
    }
    entries.push(NavigationEntry {
        page: Page::CorpusStudio,
        title: "Terminology library".into(),
        icon: include_image!(
            "../../../crates/xrtranslate-client-core/resources/icons/corpus-studio.svg"
        ),
    });
    entries.push(NavigationEntry {
        page: Page::Settings,
        title: "Settings".into(),
        icon: include_image!(
            "../../../crates/xrtranslate-client-core/resources/icons/settings.svg"
        ),
    });
    entries
}

pub fn render_top_navigation(
    ui: &mut egui::Ui,
    navigation: &mut NavigationState,
    plugins: &crate::plugins::PluginPages,
    tts_configured: bool,
    first_run: &mut bool,
    onboarding_page: &mut usize,
    language: crate::i18n::UiLanguage,
) {
    let previous = ui
        .ctx()
        .data(|data| data.get_temp::<Page>(egui::Id::unique("top_navigation_selected")));
    let selected_before = navigation.page;
    let icons_only = ui.available_width() < 600.0;
    ui.horizontal(|ui| {
        egui::ScrollArea::horizontal()
            .id_salt("top_navigation")
            .max_width((ui.available_width() - 48.0).max(0.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for entry in navigation_entries(plugins, tts_configured) {
                        let selected = navigation.page == entry.page;
                        let icon = entry.icon(selected);
                        let title = entry.label(language);
                        let button = if icons_only {
                            egui::Button::image(icon)
                        } else {
                            egui::Button::image_and_text(icon, title)
                        };
                        let response = ui
                            .add(
                                button
                                    .selected(selected)
                                    .min_size(egui::vec2(if icons_only { 40.0 } else { 0.0 }, 40.0))
                                    .corner_radius(16),
                            )
                            .on_hover_text(title);
                        if response.clicked() {
                            navigation.open(entry.page);
                        }
                        if selected && previous != Some(entry.page) {
                            response.scroll_to_me(Some(egui::Align::Center));
                        }
                    }
                });
            });
        let menu_button = ui.add(
            egui::Button::new(RichText::new("…").size(15.0).strong())
                .min_size(egui::vec2(if icons_only { 40.0 } else { 0.0 }, 40.0))
                .corner_radius(16),
        );
        let fill = if ui.visuals().dark_mode {
            Color32::from_rgb(32, 33, 36)
        } else {
            Color32::WHITE
        };
        egui::Popup::menu(&menu_button)
            .frame(
                Frame::new()
                    .fill(fill)
                    .corner_radius(CornerRadius::same(12))
                    .shadow(egui::Shadow {
                        offset: [0, 6],
                        blur: 20,
                        spread: 0,
                        color: Color32::from_black_alpha(50),
                    })
                    .stroke(Stroke::new(1.0, theme::border()))
                    .inner_margin(Margin::same(12)),
            )
            .show(|ui| {
                ui.set_min_width(170.0);
                let guide_icon = egui::Image::new(egui::include_image!(
                    "../../../crates/xrtranslate-client-core/resources/icons/guide.svg"
                ))
                .fit_to_exact_size(egui::vec2(16.0, 16.0))
                .tint(theme::text_strong());
                let response = ui.add_sized(
                    [ui.available_width(), 36.0],
                    egui::Button::image_and_text(
                        guide_icon,
                        crate::i18n::tr(language, "User Guide"),
                    )
                    .corner_radius(8),
                );
                if ui_director::egui::Control::button(crate::i18n::tr(language, "User Guide"))
                    .response(ui, response)
                    .clicked()
                {
                    tutorial::start(ui.ctx());
                    ui.close();
                }
                ui.add_space(4.0);
                let home_icon = egui::Image::new(egui::include_image!(
                    "../../../crates/xrtranslate-client-core/resources/icons/translation.svg"
                ))
                .fit_to_exact_size(egui::vec2(16.0, 16.0))
                .tint(theme::text_strong());
                if ui
                    .add_sized(
                        [ui.available_width(), 36.0],
                        egui::Button::image_and_text(
                            home_icon,
                            crate::i18n::tr(language, "Welcome Page"),
                        )
                        .corner_radius(8),
                    )
                    .clicked()
                {
                    *onboarding_page = 0;
                    *first_run = true;
                    ui.close();
                }
            });
    });
    ui.ctx().data_mut(|data| {
        data.insert_temp(egui::Id::unique("top_navigation_selected"), selected_before)
    });
}
