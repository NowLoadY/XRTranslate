use eframe::egui::{self, Align, Color32, CornerRadius, Frame, Layout, Margin, RichText};

#[derive(Clone, Debug)]
pub struct ModalPage {
    pub title: String,
    pub content: String,
    pub footnote: Option<String>,
    pub is_code: bool,
}

impl ModalPage {
    pub fn new(title: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            content: content.into(),
            footnote: None,
            is_code: false,
        }
    }

    pub fn footnote(mut self, footnote: impl Into<String>) -> Self {
        self.footnote = Some(footnote.into());
        self
    }

    pub fn code(mut self) -> Self {
        self.is_code = true;
        self
    }
}

pub struct ModalDialog {
    pub open: bool,
    pub pages: Vec<ModalPage>,
    pub current_page: usize,
    pub show_ok_button: bool,
    pub ok_label: String,
    pub show_cancel_button: bool,
    pub cancel_label: String,
    action: Option<ModalAction>,
    ok_action: Option<ModalAction>,
    destructive_ok: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModalAction {
    DownloadUpdate,
    InstallUpdate,
    ConfirmResourceDeletion,
}

impl Default for ModalDialog {
    fn default() -> Self {
        Self {
            open: false,
            pages: Vec::new(),
            current_page: 0,
            show_ok_button: true,
            ok_label: "OK".into(),
            show_cancel_button: false,
            cancel_label: "Cancel".into(),
            action: None,
            ok_action: None,
            destructive_ok: false,
        }
    }
}

impl ModalDialog {
    pub fn update_available(version: &str, language: crate::i18n::UiLanguage) -> Self {
        Self {
            open: true,
            pages: vec![ModalPage::new(
                crate::i18n::tr(language, "Update available"),
                format!(
                    "{} v{}",
                    crate::i18n::tr(language, "A new version is available:"),
                    version
                ),
            )],
            current_page: 0,
            show_ok_button: true,
            ok_label: crate::i18n::tr(language, "Update").into(),
            show_cancel_button: true,
            cancel_label: crate::i18n::tr(language, "Later").into(),
            action: None,
            ok_action: Some(ModalAction::DownloadUpdate),
            destructive_ok: false,
        }
    }

    pub fn update_ready(version: &str, language: crate::i18n::UiLanguage) -> Self {
        Self {
            open: true,
            pages: vec![
                ModalPage::new(
                    crate::i18n::tr(language, "Update ready"),
                    format!(
                        "v{}\n{}",
                        version,
                        crate::i18n::tr(language, "Install the update now?")
                    ),
                )
                .footnote(crate::i18n::tr(
                    language,
                    "You can install it later from Settings > General.",
                )),
            ],
            current_page: 0,
            show_ok_button: true,
            ok_label: crate::i18n::tr(language, "Install").into(),
            show_cancel_button: true,
            cancel_label: crate::i18n::tr(language, "Later").into(),
            action: None,
            ok_action: Some(ModalAction::InstallUpdate),
            destructive_ok: false,
        }
    }

    pub fn confirm_resource_deletion(
        resource_label: &str,
        language: crate::i18n::UiLanguage,
    ) -> Self {
        Self {
            open: true,
            pages: vec![ModalPage::new(
                crate::i18n::tr(language, "Delete downloaded resource?"),
                format!(
                    "{resource_label}\n\n{}",
                    crate::i18n::tr(
                        language,
                        "Only this resource will be removed. You can download it again later."
                    )
                ),
            )],
            current_page: 0,
            show_ok_button: true,
            ok_label: crate::i18n::tr(language, "Delete").into(),
            show_cancel_button: true,
            cancel_label: crate::i18n::tr(language, "Cancel").into(),
            action: None,
            ok_action: Some(ModalAction::ConfirmResourceDeletion),
            destructive_ok: true,
        }
    }

    pub fn usage_guidelines(language: crate::i18n::UiLanguage) -> Self {
        let content = crate::i18n::usage_notice_items(language)
            .into_iter()
            .map(|item| format!("• {item}"))
            .collect::<Vec<_>>()
            .join("\n\n");
        Self {
            open: true,
            pages: vec![ModalPage::new(
                crate::i18n::tr(language, "Usage Guidelines"),
                content,
            )],
            current_page: 0,
            show_ok_button: true,
            ok_label: crate::i18n::tr(language, "Close").into(),
            show_cancel_button: false,
            cancel_label: crate::i18n::tr(language, "Close").into(),
            action: None,
            ok_action: None,
            destructive_ok: false,
        }
    }

    pub fn take_action(&mut self) -> Option<ModalAction> {
        self.action.take()
    }

    pub fn error(
        title: impl Into<String>,
        message: impl Into<String>,
        details: Option<&str>,
    ) -> Self {
        let mut content = message.into();
        if let Some(details) = details
            && !details.trim().is_empty()
        {
            content.push_str("\n\n--- Detailed Log Output ---\n");
            content.push_str(details.trim());
        }
        let page = ModalPage::new(title, content).code();
        Self {
            open: true,
            pages: vec![page],
            current_page: 0,
            show_ok_button: true,
            ok_label: "OK".into(),
            show_cancel_button: false,
            cancel_label: "Close".into(),
            action: None,
            ok_action: None,
            destructive_ok: false,
        }
    }

    pub fn carousel(pages: Vec<ModalPage>) -> Self {
        Self {
            open: true,
            pages,
            current_page: 0,
            show_ok_button: true,
            ok_label: "Finish".into(),
            show_cancel_button: false,
            cancel_label: "Close".into(),
            action: None,
            ok_action: None,
            destructive_ok: false,
        }
    }

    pub fn render(&mut self, ctx: &egui::Context, language: crate::i18n::UiLanguage) {
        if !self.open || self.pages.is_empty() {
            return;
        }

        let backdrop_response = egui::Area::new(egui::Id::new("modal_backdrop"))
            .interactable(true)
            .order(egui::Order::Middle)
            .fixed_pos([0.0, 0.0])
            .show(ctx, |ui| {
                let screen = ctx
                    .input(|i| i.raw.screen_rect)
                    .unwrap_or_else(|| ui.max_rect());
                ui.allocate_rect(screen, egui::Sense::click())
            })
            .inner;

        let mut close_dialog = false;
        if backdrop_response.clicked() {
            close_dialog = true;
        }

        if self.current_page >= self.pages.len() {
            self.current_page = 0;
        }
        let page = self.pages[self.current_page].clone();
        let total_pages = self.pages.len();
        let is_multi_page = total_pages > 1;

        let is_hand_drawn = crate::ui::theme::is_hand_drawn(ctx);
        let modal_window = egui::Window::new("modal_dialog_window")
            .title_bar(false)
            .resizable(false)
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .fixed_size([540.0, 380.0])
            .frame(
                Frame::new()
                    .fill(if is_hand_drawn {
                        Color32::TRANSPARENT
                    } else {
                        crate::ui::theme::modal_backdrop()
                    })
                    .corner_radius(CornerRadius::same(20))
                    .inner_margin(Margin::same(20)),
            )
            .show(ctx, |ui| {
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(&page.title)
                                .size(17.0)
                                .color(crate::ui::theme::text_strong())
                                .strong(),
                        );

                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let close_id = ui.make_persistent_id("modal_close_btn");
                            let is_hovered = ui.memory(|m| {
                                m.data
                                    .get_temp::<bool>(close_id.with("hover_state"))
                                    .unwrap_or(false)
                            });
                            let hover_factor = crate::ui::animation::AnimationSystem::hover(
                                ui.ctx(),
                                close_id.with("anim_hover"),
                                is_hovered,
                            );
                            let bg_color = Color32::from_rgba_unmultiplied(
                                188,
                                198,
                                201,
                                ((0.18 * hover_factor) * 255.0) as u8,
                            );
                            let text_color = crate::ui::animation::AnimationSystem::lerp_color(
                                crate::ui::theme::text_weak(),
                                crate::ui::theme::text_strong(),
                                hover_factor,
                            );
                            let close_btn = Frame::new()
                                .fill(bg_color)
                                .corner_radius(CornerRadius::same(13))
                                .inner_margin(Margin::symmetric(7, 3))
                                .show(ui, |ui| {
                                    ui.label(
                                        RichText::new("×")
                                            .size(16.0)
                                            .color(text_color)
                                            .strong(),
                                    )
                                })
                                .response
                                .interact(egui::Sense::click());
                            ui.memory_mut(|m| {
                                m.data
                                    .insert_temp(close_id.with("hover_state"), close_btn.hovered());
                            });
                            if close_btn.hovered() {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                            }
                            if close_btn.clicked() {
                                close_dialog = true;
                            }
                        });
                    });

                    ui.add_space(12.0);

                    let body_height = if is_multi_page { 220.0 } else { 240.0 };
                    egui::ScrollArea::vertical()
                        .id_salt("modal_body_scroll")
                        .max_height(body_height)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            if page.is_code {
                                crate::ui::components::dark_container_frame(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.label(
                                        RichText::new(&page.content)
                                            .family(egui::FontFamily::Monospace)
                                            .color(Color32::from_rgb(240, 244, 255))
                                            .size(12.0),
                                    );
                                });

                                ui.add_space(6.0);
                                ui.horizontal(|ui| {
                                    if crate::ui::components::secondary_button(
                                        ui,
                                        crate::i18n::tr(language, "Copy Log"),
                                    )
                                    .clicked()
                                    {
                                        ctx.copy_text(page.content.clone());
                                    }
                                });
                            } else {
                                ui.label(
                                    RichText::new(&page.content)
                                        .size(13.5)
                                        .color(crate::ui::theme::text_normal()),
                                );
                            }
                            if let Some(footnote) = &page.footnote {
                                ui.add_space(10.0);
                                ui.label(
                                    RichText::new(footnote)
                                        .size(12.0)
                                        .color(crate::ui::theme::text_weak()),
                                );
                            }
                        });

                    ui.add_space(14.0);

                    ui.horizontal(|ui| {
                        if is_multi_page {
                            ui.label(
                                RichText::new(format!(
                                    "{} {}/{}",
                                    crate::i18n::tr(language, "Page"),
                                    self.current_page + 1,
                                    total_pages
                                ))
                                .size(12.0)
                                .color(crate::ui::theme::text_weak())
                                .strong(),
                            );

                            ui.add_space(12.0);

                            if self.current_page > 0
                                && crate::ui::components::secondary_button(
                                    ui,
                                    crate::i18n::tr(language, "Prev"),
                                )
                                .clicked()
                            {
                                self.current_page -= 1;
                            }

                            if self.current_page + 1 < total_pages
                                && crate::ui::components::primary_button(
                                    ui,
                                    crate::i18n::tr(language, "Next"),
                                )
                                .clicked()
                            {
                                self.current_page += 1;
                            }
                        }

                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if self.show_ok_button {
                                let ok_text =
                                    if is_multi_page && self.current_page + 1 < total_pages {
                                        crate::i18n::tr(language, "Close")
                                    } else {
                                        &self.ok_label
                                    };
                                let is_final_or_single_page =
                                    !is_multi_page || self.current_page + 1 == total_pages;
                                let confirmed = if self.destructive_ok {
                                    crate::ui::components::danger_button(ui, ok_text).clicked()
                                } else if is_final_or_single_page {
                                    crate::ui::components::primary_button(ui, ok_text).clicked()
                                } else {
                                    crate::ui::components::secondary_button(ui, ok_text).clicked()
                                };
                                if confirmed {
                                    self.action = self.ok_action;
                                    close_dialog = true;
                                }
                            }
                            if self.show_cancel_button
                                && crate::ui::components::secondary_button(ui, &self.cancel_label)
                                    .clicked()
                            {
                                close_dialog = true;
                            }
                        });
                    });
                });
            });

        if let Some(window_response) = modal_window {
            if is_hand_drawn {
                let fill_layer = egui::LayerId::new(
                    egui::Order::Middle,
                    egui::Id::new("modal_organic_fill_layer"),
                );
                crate::ui::organic_border::paint_with_id(
                    ctx,
                    fill_layer,
                    egui::Id::new("modal_organic_fill"),
                    window_response.response.rect,
                    crate::ui::organic_border::OrganicBorderStyle {
                        radius: 20.0,
                        half_width: 0.0,
                        displacement: 1.8,
                        noise_scale: 0.034,
                        seed: 41.0,
                        color: crate::ui::theme::modal_backdrop(),
                    },
                );
            }
        }

        if close_dialog {
            self.open = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_update_modal_offers_install_with_a_settings_hint() {
        let modal = ModalDialog::update_ready("1.2.3", crate::i18n::UiLanguage::Chinese);

        assert_eq!(modal.pages[0].title, "更新已下载");
        assert!(modal.pages[0].content.contains("是否现在安装？"));
        assert_eq!(
            modal.pages[0].footnote.as_deref(),
            Some("你也可以稍后在设置 → 常规中安装。")
        );
        assert_eq!(modal.ok_label, "安装");
        assert_eq!(modal.cancel_label, "稍后");
        assert_eq!(modal.ok_action, Some(ModalAction::InstallUpdate));
    }

    #[test]
    fn resource_deletion_modal_is_explicit_and_destructive() {
        let modal =
            ModalDialog::confirm_resource_deletion("Audio model", crate::i18n::UiLanguage::Chinese);

        assert_eq!(modal.pages[0].title, "删除已下载的资源？");
        assert!(modal.pages[0].content.contains("Audio model"));
        assert_eq!(modal.ok_label, "删除");
        assert_eq!(modal.cancel_label, "取消");
        assert!(modal.destructive_ok);
        assert_eq!(modal.ok_action, Some(ModalAction::ConfirmResourceDeletion));
    }

    #[test]
    fn usage_guidelines_modal_reuses_the_localized_notice() {
        let modal = ModalDialog::usage_guidelines(crate::i18n::UiLanguage::Chinese);

        assert_eq!(modal.pages[0].title, "使用规范");
        assert!(modal.pages[0].content.contains("仅限克隆您本人的声音"));
        assert!(modal.pages[0].content.contains("所在国家或地区"));
        assert!(modal.pages[0].content.contains("重要内容请在使用前核对"));
        assert_eq!(modal.ok_label, "关闭");
        assert!(!modal.show_cancel_button);
    }
}
