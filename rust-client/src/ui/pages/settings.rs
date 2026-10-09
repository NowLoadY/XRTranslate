use crate::ui::components::{self, SubNavItem, section, sub_sidebar};
use eframe::egui;
mod avatar;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, Default)]
pub enum SettingsSection {
    #[default]
    GeneralAppearance,
    ServiceProviders,
    Plugins,
    BackendServer,
    Avatar,
}

pub fn render(app: &mut crate::XRTranslateApp, ui: &mut egui::Ui) {
    let compact = crate::ui::layout::should_stack(ui.available_width(), 2, 260.0);
    // The settings navigator and its active pane can wrap internally, but both
    // must retain a usable side-by-side editing area.
    if !compact {
        crate::ui::layout::require_content_size(ui, egui::vec2(520.0, 360.0));
    }
    ui.label(
        egui::RichText::new(crate::i18n::tr(app.ui_language, "Settings"))
            .size(22.0)
            .color(crate::ui::theme::text_strong())
            .strong(),
    );
    ui.add_space(12.0);

    let nav_items = [
        SubNavItem {
            id: SettingsSection::GeneralAppearance,
            icon: "",
            label: crate::i18n::tr(app.ui_language, "General"),
        },
        SubNavItem {
            id: SettingsSection::ServiceProviders,
            icon: "",
            label: crate::i18n::tr(app.ui_language, "Service Providers"),
        },
        SubNavItem {
            id: SettingsSection::Plugins,
            icon: "",
            label: crate::i18n::tr(app.ui_language, "Plugins"),
        },
        SubNavItem {
            id: SettingsSection::BackendServer,
            icon: "",
            label: crate::i18n::tr(app.ui_language, "Local Service"),
        },
        SubNavItem {
            id: SettingsSection::Avatar,
            icon: "",
            label: crate::i18n::tr(app.ui_language, "Avatar"),
        },
    ];

    let content = |ui: &mut egui::Ui, app: &mut crate::XRTranslateApp| {
        ui.vertical(|ui| {
            ui.set_min_width(ui.available_width());
            egui::ScrollArea::vertical()
                .id_salt("settings_scroll_area")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let cur = app.settings_section;

                    crate::ui::animation::AnimationSystem::render_animated_page(ui, cur, |ui| {
                        match cur {
                            SettingsSection::GeneralAppearance => {
                                render_general_appearance_section(app, ui);
                            }
                            SettingsSection::Avatar => {
                                if avatar::render(ui, &mut app.avatar_appearance, app.ui_language) {
                                    app.save_settings();
                                }
                            }
                            SettingsSection::ServiceProviders => {
                                let apply = app.service_config.render(ui, app.ui_language);
                                if apply {
                                    app.apply_service_configuration(Some(ui.ctx().clone()));
                                }
                            }
                            SettingsSection::Plugins => {
                                render_plugins_section(app, ui);
                            }
                            SettingsSection::BackendServer => {
                                render_server_section(app, ui);
                            }
                        }
                    });
                });
        });
    };
    if compact {
        egui::ScrollArea::horizontal()
            .id_salt("settings_navigation")
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for item in &nav_items {
                        ui.selectable_value(&mut app.settings_section, item.id, item.label);
                    }
                });
            });
        ui.add_space(12.0);
        content(ui, app);
    } else {
        ui.horizontal_top(|ui| {
            sub_sidebar(ui, &mut app.settings_section, &nav_items, app.ui_language);
            ui.add_space(12.0);
            content(ui, app);
        });
    }
}

fn background_image_controls(app: &mut crate::XRTranslateApp, ui: &mut egui::Ui) {
    let label = crate::i18n::tr(app.ui_language, "Background image");
    let response = components::compact_icon_button(
        ui,
        "settings_background_image",
        app.background_settings.image_path.is_some(),
        label,
        None,
    );
    egui::Image::new(egui::include_image!("../../../resources/icons/image.svg"))
        .tint(crate::ui::theme::text_strong())
        .paint_at(
            ui,
            egui::Rect::from_center_size(response.rect.center(), egui::vec2(18.0, 18.0)),
        );
    egui::Popup::from_toggle_button_response(&response)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .layout(egui::Layout::top_down_justified(egui::Align::Min))
        .width(208.0)
        .show(|ui| {
            ui.set_width(208.0);
            if components::secondary_button(ui, crate::i18n::tr(app.ui_language, "Choose image…"))
                .clicked()
            {
                app.choose_background_image(ui.ctx());
                ui.close();
            }
            ui.horizontal(|ui| {
                let label = ui.label(crate::i18n::tr(app.ui_language, "Image opacity"));
                let mut percent = app.background_settings.opacity * 100.0;
                let response = ui
                    .add(
                        egui::DragValue::new(&mut percent)
                            .range(0.0..=100.0)
                            .speed(0.5)
                            .fixed_decimals(2)
                            .suffix("%"),
                    )
                    .labelled_by(label.id);
                if response.changed() {
                    app.set_background_opacity(percent / 100.0);
                }
            });
            if components::secondary_button(ui, crate::i18n::tr(app.ui_language, "Restore default"))
                .clicked()
            {
                app.reset_background();
            }
        });
}

fn render_general_appearance_section(app: &mut crate::XRTranslateApp, ui: &mut egui::Ui) {
    section(
        ui,
        crate::i18n::tr(app.ui_language, "Language & Theme"),
        |ui| {
            let is_compact = ui.available_width() < 420.0;
            if is_compact {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(crate::i18n::tr(app.ui_language, "Language"))
                            .color(crate::ui::theme::text_strong())
                            .strong(),
                    );
                    if components::language_selector(
                        ui,
                        "settings_ui_language",
                        &mut app.ui_language,
                    ) {
                        app.set_ui_language(app.ui_language);
                    }
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(crate::i18n::tr(app.ui_language, "Theme"))
                            .color(crate::ui::theme::text_strong())
                            .strong(),
                    );
                    let mut variant = app.ui_theme.variant;
                    let selected_variant_text = match variant {
                        crate::ui::theme::ThemeVariant::Default => {
                            crate::i18n::tr(app.ui_language, "Default")
                        }
                        crate::ui::theme::ThemeVariant::HandDrawn => {
                            crate::i18n::tr(app.ui_language, "Hand-drawn")
                        }
                    };
                    crate::ui::components::combobox_ui(
                        ui,
                        "settings_theme_variant",
                        selected_variant_text,
                        |ui| {
                            ui.selectable_value(
                                &mut variant,
                                crate::ui::theme::ThemeVariant::Default,
                                crate::i18n::tr(app.ui_language, "Default"),
                            );
                            ui.selectable_value(
                                &mut variant,
                                crate::ui::theme::ThemeVariant::HandDrawn,
                                crate::i18n::tr(app.ui_language, "Hand-drawn"),
                            );
                        },
                    );
                    if variant != app.ui_theme.variant {
                        app.set_ui_theme(crate::ui::theme::UiTheme { variant });
                    }
                    background_image_controls(app, ui);
                });
            } else {
                crate::ui::layout::flow_row(ui, |ui| {
                    ui.label(
                        egui::RichText::new(crate::i18n::tr(app.ui_language, "Language"))
                            .color(crate::ui::theme::text_strong())
                            .strong(),
                    );
                    if components::language_selector(
                        ui,
                        "settings_ui_language",
                        &mut app.ui_language,
                    ) {
                        app.set_ui_language(app.ui_language);
                    }
                    ui.add_space(18.0);
                    ui.label(
                        egui::RichText::new(crate::i18n::tr(app.ui_language, "Theme"))
                            .color(crate::ui::theme::text_strong())
                            .strong(),
                    );
                    ui.label(crate::i18n::tr(app.ui_language, "Theme variant"));
                    let mut variant = app.ui_theme.variant;
                    let selected_variant_text = match variant {
                        crate::ui::theme::ThemeVariant::Default => {
                            crate::i18n::tr(app.ui_language, "Default")
                        }
                        crate::ui::theme::ThemeVariant::HandDrawn => {
                            crate::i18n::tr(app.ui_language, "Hand-drawn")
                        }
                    };
                    crate::ui::components::combobox_ui(
                        ui,
                        "settings_theme_variant",
                        selected_variant_text,
                        |ui| {
                            ui.selectable_value(
                                &mut variant,
                                crate::ui::theme::ThemeVariant::Default,
                                crate::i18n::tr(app.ui_language, "Default"),
                            );
                            ui.selectable_value(
                                &mut variant,
                                crate::ui::theme::ThemeVariant::HandDrawn,
                                crate::i18n::tr(app.ui_language, "Hand-drawn"),
                            );
                        },
                    );
                    if variant != app.ui_theme.variant {
                        app.set_ui_theme(crate::ui::theme::UiTheme { variant });
                    }
                    background_image_controls(app, ui);
                });
            }
        },
    );
    ui.add_space(14.0);

    section(
        ui,
        crate::i18n::tr(app.ui_language, "Translation history"),
        |ui| {
            let mut save = app.history_archive.enabled();
            if components::toggle_with_label(
                ui,
                &mut save,
                crate::i18n::tr(app.ui_language, "Save translation history locally"),
            )
            .changed()
            {
                app.history_archive.set_enabled(save);
                app.save_settings();
            }
            ui.weak(crate::i18n::tr(
                app.ui_language,
                "Saving starts when enabled. Turning it off keeps existing records.",
            ));
        },
    );
    ui.add_space(14.0);

    #[cfg(any(windows, target_os = "linux"))]
    section(
        ui,
        crate::i18n::tr(app.ui_language, "Quick translation"),
        |ui| {
            let lang = app.ui_language;
            #[cfg(windows)]
            if components::toggle_with_label(
                ui,
                &mut app.quick_translate.settings.enabled,
                crate::i18n::tr(lang, "Enable global shortcut"),
            )
            .changed()
            {
                app.save_settings();
            }
            ui.horizontal_wrapped(|ui| {
                #[cfg(windows)]
                {
                    ui.label(crate::i18n::tr(lang, "Shortcut"));
                    ui.add(
                        egui::TextEdit::singleline(&mut app.quick_translate.shortcut_draft)
                            .desired_width(150.0),
                    );
                    if components::secondary_button(ui, crate::i18n::tr(lang, "Apply")).clicked() {
                        app.quick_translate.apply_shortcut();
                        app.save_settings();
                    }
                }
                if components::secondary_button(ui, crate::i18n::tr(lang, "Translate clipboard"))
                    .clicked()
                {
                    app.begin_quick_translation(
                        crate::desktop_shortcut::clipboard_text(),
                        ui.ctx(),
                    );
                }
            });
            #[cfg(windows)]
            ui.weak(crate::i18n::tr(lang, "Select or copy text, then press the shortcut. Uses the languages on the translation page."));
            #[cfg(target_os = "linux")]
            ui.weak(crate::i18n::tr(
                lang,
                "Copy text, then translate it here. Uses the languages on the translation page.",
            ));
            #[cfg(windows)]
            if let Some(error) = &app.quick_translate.shortcut_error {
                ui.add(egui::Label::new(crate::i18n::tr_dynamic(lang, error)).wrap());
            }
        },
    );
    ui.add_space(14.0);

    section(ui, crate::i18n::tr(app.ui_language, "Downloads"), |ui| {
        crate::ui::layout::flow_row(ui, |ui| {
            ui.label(
                egui::RichText::new(crate::i18n::tr(app.ui_language, "Download proxy"))
                    .color(crate::ui::theme::text_strong())
                    .strong(),
            );
            let response = crate::ui::components::text_edit_ui(
                ui,
                "settings_download_proxy_url",
                egui::TextEdit::singleline(&mut app.download_proxy_url)
                    .hint_text(crate::i18n::tr(
                        app.ui_language,
                        "Optional, e.g. http://127.0.0.1:7890",
                    ))
                    .desired_width((ui.available_width() - 120.0).max(220.0)),
            );
            if response.lost_focus() || response.changed() {
                app.set_download_proxy_url(app.download_proxy_url.clone());
            }
        });
    });
    ui.add_space(14.0);

    section(ui, crate::i18n::tr(app.ui_language, "About"), |ui| {
        crate::ui::layout::flow_row(ui, |ui| {
            ui.label(
                egui::RichText::new(crate::i18n::tr(app.ui_language, "Version:"))
                    .color(crate::ui::theme::text_strong())
                    .strong(),
            );
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(crate::version::version_display_string())
                    .color(crate::ui::theme::text_normal()),
            );
        });
        ui.add_space(8.0);
        crate::ui::layout::flow_row(ui, |ui| {
            ui.label(
                egui::RichText::new("GitHub:")
                    .color(crate::ui::theme::text_strong())
                    .strong(),
            );
            ui.add_space(4.0);
            ui.hyperlink_to(
                "https://github.com/NowLoadY/XRTranslate",
                "https://github.com/NowLoadY/XRTranslate",
            );
        });
        ui.add_space(8.0);
        crate::ui::layout::flow_row(ui, |ui| {
            ui.label(
                egui::RichText::new("chatgpt.site:")
                    .color(crate::ui::theme::text_strong())
                    .strong(),
            );
            ui.add_space(4.0);
            ui.hyperlink_to(
                "https://xrtranslate.nowloady.chatgpt.site",
                "https://xrtranslate.nowloady.chatgpt.site",
            );
        });
        ui.add_space(14.0);
        render_update_controls(app, ui);

        let groups = crate::contributors::load_contributors_cached(&app.project_root());
        if !groups.is_empty() {
            ui.add_space(14.0);
            components::wavy_divider(ui, crate::ui::theme::border());
            ui.add_space(12.0);

            for group in groups {
                let section_title = match &group.role {
                    crate::contributors::ContributorRole::CodeContributors => {
                        crate::i18n::tr(app.ui_language, "Code Contributors")
                    }
                    crate::contributors::ContributorRole::BetaTesters => {
                        crate::i18n::tr(app.ui_language, "Beta Testers")
                    }
                    crate::contributors::ContributorRole::Other(title) => title.as_str(),
                };

                ui.label(
                    egui::RichText::new(section_title)
                        .color(crate::ui::theme::text_strong())
                        .size(13.5)
                        .strong(),
                );
                ui.add_space(6.0);

                for contributor in &group.contributors {
                    ui.vertical(|ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                egui::RichText::new(&contributor.name)
                                    .color(crate::ui::theme::text_strong())
                                    .strong(),
                            );
                            if !contributor.links.is_empty() {
                                ui.add_space(4.0);
                                for link in &contributor.links {
                                    render_social_link_chip(ui, &link.label, &link.url);
                                    ui.add_space(3.0);
                                }
                            }
                            if contributor.contributions.len() == 1 {
                                ui.add_space(4.0);
                                let text = format!("— {}", contributor.contributions[0]);
                                ui.add(
                                    egui::Label::new(contributor_markdown_job(&text, 12.0)).wrap(),
                                );
                            }
                        });
                        if contributor.contributions.len() > 1 {
                            ui.add_space(2.0);
                            for item in &contributor.contributions {
                                ui.horizontal_top(|ui| {
                                    ui.add_space(8.0);
                                    ui.label(
                                        egui::RichText::new("•")
                                            .size(11.0)
                                            .color(crate::ui::theme::primary_dark()),
                                    );
                                    ui.add(
                                        egui::Label::new(contributor_markdown_job(item, 12.0))
                                            .wrap(),
                                    );
                                });
                            }
                        }
                    });
                    ui.add_space(5.0);
                }
                ui.add_space(6.0);
            }
        }
    });
    ui.add_space(14.0);

    section(ui, crate::i18n::tr(app.ui_language, "Notice"), |ui| {
        for (index, item) in crate::i18n::usage_notice_items(app.ui_language)
            .into_iter()
            .enumerate()
        {
            if index > 0 {
                ui.add_space(7.0);
            }
            render_notice_item(ui, item, index == 0);
        }
    });
}

fn contributor_markdown_job(text: &str, size: f32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    for span in crate::contributors::parse_inline_markdown(text) {
        job.append(
            &span.text,
            0.0,
            egui::TextFormat {
                font_id: egui::FontId::proportional(size),
                color: if span.strong {
                    crate::ui::theme::text_strong()
                } else {
                    crate::ui::theme::text_weak()
                },
                ..Default::default()
            },
        );
    }
    job
}

fn render_notice_item(ui: &mut egui::Ui, text: &str, strong: bool) {
    ui.horizontal_top(|ui| {
        ui.label(
            egui::RichText::new("•")
                .color(crate::ui::theme::primary_dark())
                .strong(),
        );
        let text = egui::RichText::new(text).size(12.5).color(if strong {
            crate::ui::theme::text_strong()
        } else {
            crate::ui::theme::text_normal()
        });
        ui.add(egui::Label::new(if strong { text.strong() } else { text }).wrap());
    });
}

fn render_social_link_chip(ui: &mut egui::Ui, label: &str, url: &str) {
    let is_external = url.starts_with("http://") || url.starts_with("https://");
    let display_text = if is_external {
        format!("{label} ↗")
    } else {
        label.to_string()
    };

    egui::Frame::new()
        .fill(crate::ui::theme::surface_control())
        .stroke(egui::Stroke::new(1.0, crate::ui::theme::border()))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.hyperlink_to(
                egui::RichText::new(display_text)
                    .size(11.5)
                    .color(crate::ui::theme::primary_dark()),
                url,
            );
        });
}

pub(crate) fn render_update_action_button(app: &mut crate::XRTranslateApp, ui: &mut egui::Ui) {
    if !crate::app_update::AppUpdateManager::is_supported() {
        return;
    }
    use crate::app_update::AppUpdateState;

    let language = app.ui_language;
    let state = app.app_update_state().clone();
    let busy = app.app_update_manager.is_busy();
    let is_actionable = matches!(
        &state,
        AppUpdateState::Ready(_) | AppUpdateState::Available(_)
    );
    let label = match &state {
        AppUpdateState::Ready(_) => crate::i18n::tr(
            language,
            if cfg!(target_os = "android") {
                "Install"
            } else {
                "Install and restart"
            },
        )
        .to_string(),
        AppUpdateState::Available(_) => crate::i18n::tr(language, "Download Update").to_string(),
        AppUpdateState::Checking => crate::i18n::tr(language, "Checking...").to_string(),
        AppUpdateState::Downloading {
            downloaded, total, ..
        } => {
            if *total > 0 {
                let percent = (*downloaded as f64 / *total as f64 * 100.0).clamp(0.0, 100.0);
                format!(
                    "{} {:.0}%",
                    crate::i18n::tr(language, "Downloading..."),
                    percent
                )
            } else {
                crate::i18n::tr(language, "Downloading...").to_string()
            }
        }
        AppUpdateState::Installing => crate::i18n::tr(language, "Installing...").to_string(),
        AppUpdateState::Current | AppUpdateState::Idle => {
            crate::i18n::tr(language, "Check for Updates").to_string()
        }
        AppUpdateState::Failed(_) => crate::i18n::tr(language, "Try Again").to_string(),
    };
    let button_resp = if is_actionable {
        components::primary_button_enabled(ui, &label, !busy)
    } else {
        components::animated_button_enabled(ui, &label, !busy)
    };
    let button_resp = if let AppUpdateState::Downloading {
        downloaded, total, ..
    } = &state
    {
        button_resp.on_hover_text(format!(
            "{}\n{} / {}",
            crate::i18n::tr(language, "Downloading..."),
            components::format_file_size(*downloaded),
            components::format_file_size(*total)
        ))
    } else if let AppUpdateState::Current = &state {
        button_resp.on_hover_text(crate::i18n::tr(language, "You're up to date"))
    } else {
        button_resp
    };
    if button_resp.clicked() {
        match &state {
            AppUpdateState::Ready(_) => app.install_update_and_restart(),
            AppUpdateState::Available(_) => app.download_update(),
            AppUpdateState::Current | AppUpdateState::Idle | AppUpdateState::Failed(_) => {
                app.check_for_updates()
            }
            AppUpdateState::Checking
            | AppUpdateState::Downloading { .. }
            | AppUpdateState::Installing => {}
        }
    }
}

fn render_update_controls(app: &mut crate::XRTranslateApp, ui: &mut egui::Ui) {
    if !crate::app_update::AppUpdateManager::is_supported() {
        return;
    }
    use crate::app_update::AppUpdateState;
    use crate::client_settings::UpdateChannel;

    let language = app.ui_language;
    let mut beta_enabled = app.update_channel == UpdateChannel::Beta;
    if components::checkbox(
        ui,
        &mut beta_enabled,
        crate::i18n::tr(language, "Receive beta updates"),
    )
    .on_hover_text(crate::i18n::tr(
        language,
        "Include prerelease builds. Beta builds can still update to stable releases.",
    ))
    .changed()
    {
        app.set_update_channel(if beta_enabled {
            UpdateChannel::Beta
        } else {
            UpdateChannel::Stable
        });
    }
    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        ui.label(
            egui::RichText::new("Update")
                .color(crate::ui::theme::text_strong())
                .strong(),
        );
        ui.add_space(4.0);
        let state = app.app_update_state().clone();
        let status = match &state {
            AppUpdateState::Idle => crate::i18n::tr(language, "Ready"),
            AppUpdateState::Checking => crate::i18n::tr(language, "Checking..."),
            AppUpdateState::Current => crate::i18n::tr(language, "You're up to date"),
            AppUpdateState::Available(_) => crate::i18n::tr(language, "Update available"),
            AppUpdateState::Downloading { .. } => crate::i18n::tr(language, "Downloading..."),
            AppUpdateState::Ready(_) => crate::i18n::tr(language, "Ready to install"),
            AppUpdateState::Installing => crate::i18n::tr(language, "Installing..."),
            AppUpdateState::Failed(_) => crate::i18n::tr(language, "Update failed"),
        };
        let response = ui.label(egui::RichText::new(status).color(crate::ui::theme::text_weak()));
        if let AppUpdateState::Failed(error) = &state {
            response.on_hover_text(error);
        }
        match &state {
            AppUpdateState::Available(info) | AppUpdateState::Ready(info) => {
                ui.add_space(10.0);
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(format!("v{}", info.version))
                            .color(crate::ui::theme::text_strong())
                            .strong(),
                    );
                    ui.label(
                        egui::RichText::new(info.asset_name.as_str())
                            .size(12.0)
                            .color(crate::ui::theme::text_weak()),
                    );
                });
            }
            AppUpdateState::Downloading {
                downloaded, total, ..
            } => {
                ui.add_space(10.0);
                let percent = if *total > 0 {
                    (*downloaded as f32 / *total as f32).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "{} / {} ({:.0}%)",
                            components::format_file_size(*downloaded),
                            components::format_file_size(*total),
                            percent * 100.0
                        ))
                        .color(crate::ui::theme::text_strong()),
                    );
                    ui.add_space(4.0);
                    ui.add(
                        egui::ProgressBar::new(percent)
                            .desired_width(180.0)
                            .animate(true),
                    );
                });
            }
            _ => {}
        }
    });
    ui.add_space(10.0);
    crate::ui::layout::flow_row(ui, |ui| {
        render_update_action_button(app, ui);
    });
}

fn render_server_section(app: &mut crate::XRTranslateApp, ui: &mut egui::Ui) {
    let compact = ui.available_width() < 480.0;
    section(ui, crate::i18n::tr(app.ui_language, "Backend"), |ui| {
        ui.label(format!(
            "{}:",
            crate::i18n::tr(app.ui_language, "Runtime Directory")
        ));
        ui.add(
            egui::Label::new(
                egui::RichText::new(
                    app.backend_manager
                        .runtime_layout()
                        .runtime_root()
                        .display()
                        .to_string(),
                )
                .color(crate::ui::theme::text_weak()),
            )
            .wrap(),
        );
    });

    ui.add_space(14.0);

    section(ui, crate::i18n::tr(app.ui_language, "Server"), |ui| {
        if compact {
            ui.vertical(|ui| {
                ui.label("URL:");
                ui.add_space(4.0);
                if crate::ui::components::text_edit_ui(
                    ui,
                    "settings_server_url",
                    egui::TextEdit::singleline(&mut app.server_url)
                        .desired_width(ui.available_width()),
                )
                .changed()
                {
                    app.save_settings();
                }
            });
        } else {
            crate::ui::layout::flow_row(ui, |ui| {
                ui.label("URL:");
                if crate::ui::components::text_edit_ui(
                    ui,
                    "settings_server_url",
                    egui::TextEdit::singleline(&mut app.server_url)
                        .desired_width((ui.available_width() - 100.0).clamp(240.0, 360.0)),
                )
                .changed()
                {
                    app.save_settings();
                }
            });
        }
    });
}

fn render_plugins_section(app: &mut crate::XRTranslateApp, ui: &mut egui::Ui) {
    let language = app.ui_language;
    ui.label(
        egui::RichText::new(crate::i18n::tr(language, "Plugins"))
            .size(20.0)
            .color(crate::ui::theme::text_strong())
            .strong(),
    );
    ui.add_space(2.0);
    ui.label(
        egui::RichText::new(crate::i18n::tr(
            language,
            "Choose which optional tools appear in the sidebar.",
        ))
        .size(12.5)
        .color(crate::ui::theme::text_weak()),
    );
    ui.add_space(16.0);

    for descriptor in crate::plugins::PluginRegistry::builtin().descriptors() {
        if !app.plugin_available(descriptor.id) {
            continue;
        }
        let mut enabled = app.plugin_enabled(descriptor.id);
        let disable_reason = enabled
            .then(|| app.plugin_disable_block_reason(descriptor.id))
            .flatten();

        components::card(ui, |ui| {
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    egui::Frame::new()
                        .fill(egui::Color32::from_rgb(238, 242, 255))
                        .corner_radius(egui::CornerRadius::same(10))
                        .inner_margin(egui::Margin::same(8))
                        .show(ui, |ui| {
                            ui.add(
                                egui::Image::new(descriptor.icon.image_source())
                                    .fit_to_exact_size(egui::vec2(20.0, 20.0))
                                    .tint(egui::Color32::from_rgb(59, 130, 246)),
                            );
                        });

                    ui.add_space(10.0);

                    let available_for_text = (ui.available_width() - 56.0).max(80.0);
                    ui.allocate_ui_with_layout(
                        egui::vec2(available_for_text, 0.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.label(
                                egui::RichText::new(crate::i18n::tr(
                                    language,
                                    descriptor.title_key,
                                ))
                                .size(14.5)
                                .color(crate::ui::theme::text_strong())
                                .strong(),
                            );
                            ui.add_space(2.0);
                            ui.label(
                                egui::RichText::new(crate::i18n::tr(
                                    language,
                                    descriptor.description_key,
                                ))
                                .size(12.0)
                                .color(crate::ui::theme::text_weak()),
                            );
                            if let Some(reason) = &disable_reason {
                                ui.add_space(2.0);
                                ui.label(
                                    egui::RichText::new(reason)
                                        .size(11.5)
                                        .color(egui::Color32::from_rgb(220, 38, 38)),
                                );
                            }
                        },
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let response = ui
                            .add_enabled_ui(disable_reason.is_none(), |ui| {
                                components::pill_toggle(ui, &mut enabled)
                            })
                            .inner;
                        if response.changed() {
                            app.set_plugin_enabled(descriptor.id, enabled);
                        }
                    });
                });

                if enabled
                    && descriptor.settings_contribution
                        == crate::plugins::PluginSettingsContribution::Plugin
                {
                    ui.add_space(14.0);
                    components::wavy_divider(ui, crate::ui::theme::text_strong());
                    ui.add_space(14.0);
                    app.render_plugin_settings(descriptor.id, ui);
                }
            });
        });
        ui.add_space(12.0);
    }
}
