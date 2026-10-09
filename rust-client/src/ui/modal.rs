use eframe::egui::{self, Color32, Frame, Margin, RichText};

#[derive(Clone, Debug)]
pub struct ModalPage {
    pub title: String,
    pub content: String,
    pub footnote: Option<String>,
    pub error_details: Option<String>,
}

impl ModalPage {
    pub fn new(title: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            content: content.into(),
            footnote: None,
            error_details: None,
        }
    }

    pub fn footnote(mut self, footnote: impl Into<String>) -> Self {
        self.footnote = Some(footnote.into());
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
    update_version: Option<String>,
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
            update_version: None,
        }
    }
}

impl ModalDialog {
    pub fn update_available(version: &str, language: crate::i18n::UiLanguage) -> Self {
        Self {
            open: true,
            pages: vec![ModalPage::new(
                crate::i18n::tr(language, "A new version is here"),
                "",
            )],
            update_version: Some(format!("v{version}")),
            ok_label: crate::i18n::tr(language, "Update").into(),
            show_cancel_button: true,
            cancel_label: crate::i18n::tr(language, "Later").into(),
            ok_action: Some(ModalAction::DownloadUpdate),
            ..Self::default()
        }
    }

    pub fn update_ready(version: &str, language: crate::i18n::UiLanguage) -> Self {
        Self {
            open: true,
            pages: vec![
                ModalPage::new(
                    crate::i18n::tr(language, "Update ready"),
                    crate::i18n::tr(language, "Install the update now?"),
                )
                .footnote(crate::i18n::tr(
                    language,
                    "You can install it later from Settings > General.",
                )),
            ],
            update_version: Some(format!("v{version}")),
            ok_label: crate::i18n::tr(
                language,
                if cfg!(target_os = "android") {
                    "Install"
                } else {
                    "Install and restart"
                },
            )
            .into(),
            show_cancel_button: true,
            cancel_label: crate::i18n::tr(language, "Later").into(),
            ok_action: Some(ModalAction::InstallUpdate),
            ..Self::default()
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
            ok_label: crate::i18n::tr(language, "Delete").into(),
            show_cancel_button: true,
            cancel_label: crate::i18n::tr(language, "Cancel").into(),
            ok_action: Some(ModalAction::ConfirmResourceDeletion),
            destructive_ok: true,
            ..Self::default()
        }
    }

    pub fn usage_guidelines(language: crate::i18n::UiLanguage) -> Self {
        let content = crate::usage_guidelines::full_guidelines_text(language);
        Self {
            open: true,
            pages: vec![ModalPage::new(
                crate::i18n::tr(language, "Usage Guidelines"),
                content,
            )],
            ok_label: crate::i18n::tr(language, "Close").into(),
            ..Self::default()
        }
    }

    pub fn take_action(&mut self) -> Option<ModalAction> {
        self.action.take()
    }

    pub fn error(language: crate::i18n::UiLanguage, error: &str, log: Option<&str>) -> Self {
        let mut details = error.trim().to_owned();
        if let Some(log) = log.filter(|log| !log.trim().is_empty()) {
            details.push_str("\n\n--- Backend log ---\n");
            details.push_str(log.trim());
        }
        let mut page = ModalPage::new(
            crate::i18n::tr(language, "Something went wrong"),
            crate::i18n::tr(
                language,
                "You can copy the details and report this on GitHub Issues or in QQ group 1009732148.",
            ),
        );
        page.error_details = Some(details);
        Self {
            open: true,
            pages: vec![page],
            ok_label: crate::i18n::tr(language, "Close").into(),
            ..Self::default()
        }
    }

    pub fn carousel(pages: Vec<ModalPage>) -> Self {
        Self {
            open: true,
            pages,
            ok_label: "Finish".into(),
            ..Self::default()
        }
    }

    pub fn render(&mut self, ctx: &egui::Context, language: crate::i18n::UiLanguage) {
        self.render_with_id(ctx, language, egui::Id::new("modal_dialog"));
    }

    pub(super) fn render_with_id(
        &mut self,
        ctx: &egui::Context,
        language: crate::i18n::UiLanguage,
        id: egui::Id,
    ) {
        if !self.open || self.pages.is_empty() {
            return;
        }
        self.current_page = self.current_page.min(self.pages.len() - 1);
        let page = self.pages[self.current_page].clone();
        let total_pages = self.pages.len();
        let mut close = false;
        let response = dialog_with_width(
            ctx,
            id,
            language,
            &page.title,
            if self.update_version.is_some() {
                360.0
            } else {
                500.0
            },
            |ui| {
                if let Some(version) = &self.update_version {
                    crate::ui::components::status_badge(ui, version, false, false);
                }
                if !page.content.is_empty() {
                    ui.label(&page.content);
                }
                if let Some(details) = &page.error_details {
                    ui.add_space(10.0);
                    crate::ui::components::dark_container_frame(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.add(
                            egui::Label::new(
                                RichText::new(details)
                                    .monospace()
                                    .size(12.0)
                                    .color(Color32::from_rgb(240, 244, 255)),
                            )
                            .wrap(),
                        );
                    });
                }
                if let Some(footnote) = &page.footnote {
                    ui.add_space(10.0);
                    ui.label(
                        RichText::new(footnote)
                            .small()
                            .color(crate::ui::theme::text_weak()),
                    );
                }
            },
            |ui| {
                if let Some(details) = &page.error_details {
                    ui.horizontal_wrapped(|ui| {
                        crate::ui::components::error_actions(ui, language, details)
                    });
                }
                ui.horizontal_wrapped(|ui| {
                    if total_pages > 1 {
                        ui.label(format!("{}/{}", self.current_page + 1, total_pages));
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
                    if self.show_cancel_button
                        && crate::ui::components::secondary_button(ui, &self.cancel_label).clicked()
                    {
                        close = true;
                    }
                    if self.show_ok_button {
                        let final_page = self.current_page + 1 == total_pages;
                        let text = if final_page {
                            &self.ok_label
                        } else {
                            crate::i18n::tr(language, "Close")
                        };
                        let button = if self.destructive_ok {
                            crate::ui::components::danger_button(ui, text)
                        } else {
                            crate::ui::components::primary_button(ui, text)
                        };
                        if button.clicked() {
                            if final_page {
                                self.action = self.ok_action;
                            }
                            close = true;
                        }
                    }
                });
            },
        );
        if close || response {
            self.open = false;
        }
    }
}

pub(super) fn dialog(
    ctx: &egui::Context,
    id: egui::Id,
    language: crate::i18n::UiLanguage,
    title: &str,
    body: impl FnOnce(&mut egui::Ui),
    actions: impl FnOnce(&mut egui::Ui),
) -> bool {
    dialog_with_width(ctx, id, language, title, 500.0, body, actions)
}

fn dialog_with_width(
    ctx: &egui::Context,
    id: egui::Id,
    language: crate::i18n::UiLanguage,
    title: &str,
    max_width: f32,
    body: impl FnOnce(&mut egui::Ui),
    actions: impl FnOnce(&mut egui::Ui),
) -> bool {
    let available = ctx.content_rect().size();
    let width = (available.x - 64.0).clamp(80.0, max_width);
    let mut close = false;
    let response = egui::Modal::new(id).frame(Frame::NONE).show(ctx, |ui| {
        crate::ui::organic_border::show(
            ui,
            id.with("border"),
            Frame::new()
                .fill(crate::ui::theme::modal_backdrop())
                .corner_radius(crate::ui::theme::container_radius(16))
                .inner_margin(Margin::same(16)),
            crate::ui::theme::container_radius_f32(16.0),
            crate::ui::theme::text_weak(),
            |ui| {
                ui.set_width(width);
                // Area remembers its last size; allow the scroll viewport to grow.
                ui.set_max_height((available.y - 64.0).max(24.0));
                egui::ScrollArea::vertical()
                    .id_salt("dialog")
                    .max_height((available.y - 64.0).max(24.0))
                    .min_scrolled_height(0.0)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.add_sized(
                                [width - 40.0, 24.0],
                                egui::Label::new(RichText::new(title).size(17.0).strong()).wrap(),
                            );
                            close = crate::ui::components::secondary_button(ui, "×")
                                .on_hover_text(crate::i18n::tr(language, "Close"))
                                .clicked();
                        });
                        ui.add_space(8.0);
                        egui::ScrollArea::vertical()
                            .id_salt("body")
                            .max_height((available.y - 200.0).clamp(32.0, 440.0))
                            .min_scrolled_height(0.0)
                            .show(ui, |ui| {
                                ui.set_width(width);
                                body(ui);
                            });
                        ui.add_space(12.0);
                        actions(ui);
                    });
            },
        );
    });
    close || response.should_close()
}

/// Returns a decision only when the user confirms or dismisses the dialog.
pub fn confirm(
    ctx: &egui::Context,
    id: egui::Id,
    language: crate::i18n::UiLanguage,
    title: &str,
    message: &str,
    confirm_label: &str,
    enabled: bool,
) -> Option<bool> {
    let mut decision = None;
    let close = dialog(
        ctx,
        id,
        language,
        title,
        |ui| {
            ui.label(message);
        },
        |ui| {
            ui.horizontal_wrapped(|ui| {
                if crate::ui::components::secondary_button(ui, crate::i18n::tr(language, "Cancel"))
                    .clicked()
                {
                    decision = Some(false);
                }
                if crate::ui::components::danger_button_enabled(ui, confirm_label, enabled)
                    .clicked()
                {
                    decision = Some(true);
                }
            });
        },
    );
    decision.or_else(|| close.then_some(false))
}
