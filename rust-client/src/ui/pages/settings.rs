use crate::ui::components::{self, SubNavItem, section, sub_sidebar};
use eframe::egui;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, Default)]
pub enum SettingsSection {
    #[default]
    GeneralAppearance,
    ServiceProviders,
    Plugins,
    BackendServer,
}

pub fn render(app: &mut crate::XRTranslateApp, ui: &mut egui::Ui) {
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
    ];

    ui.horizontal_top(|ui| {
        sub_sidebar(ui, &mut app.settings_section, &nav_items, app.ui_language);

        ui.add_space(12.0);

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
                            SettingsSection::ServiceProviders => {
                                let project_root = app.project_root();
                                let live_tts_backend = app.tts_runtime_backend.clone();
                                let live_tts_cuda_version = app.tts_runtime_cuda_version.clone();
                                let (apply, delete_runtime) = app.service_config.render(
                                    ui,
                                    &mut app.backend_manager,
                                    &mut app.model_task_manager,
                                    &mut app.runtime_installer,
                                    live_tts_backend.as_deref(),
                                    live_tts_cuda_version.as_deref(),
                                    &project_root,
                                    app.ui_language,
                                );
                                if apply {
                                    app.apply_service_configuration(Some(ui.ctx().clone()));
                                }
                                if delete_runtime {
                                    app.request_runtime_resource_deletion();
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
    });
}

fn render_general_appearance_section(app: &mut crate::XRTranslateApp, ui: &mut egui::Ui) {
    section(ui, crate::i18n::tr(app.ui_language, "Language"), |ui| {
        if components::language_selector(ui, "settings_ui_language", &mut app.ui_language) {
            app.set_ui_language(app.ui_language);
        }
    });
    ui.add_space(14.0);

    section(ui, crate::i18n::tr(app.ui_language, "Downloads"), |ui| {
        ui.label(
            egui::RichText::new(crate::i18n::tr(app.ui_language, "Download proxy"))
                .color(crate::ui::theme::text_strong())
                .strong(),
        );
        let response = ui.add(
            egui::TextEdit::singleline(&mut app.download_proxy_url)
                .hint_text(crate::i18n::tr(
                    app.ui_language,
                    "Optional, e.g. http://127.0.0.1:7890",
                ))
                .desired_width(300.0),
        );
        if response.lost_focus() || response.changed() {
            app.set_download_proxy_url(app.download_proxy_url.clone());
        }
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(crate::i18n::tr(
                app.ui_language,
                "Used only for updates and downloads. Leave empty when your VPN uses global mode.",
            ))
            .size(12.0)
            .color(crate::ui::theme::text_weak()),
        );
    });
    ui.add_space(14.0);

    section(ui, crate::i18n::tr(app.ui_language, "About"), |ui| {
        ui.horizontal(|ui| {
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
        ui.horizontal(|ui| {
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
                                ui.label(
                                    egui::RichText::new(format!(
                                        "— {}",
                                        contributor.contributions[0]
                                    ))
                                    .size(12.0)
                                    .color(crate::ui::theme::text_weak()),
                                );
                            }
                        });
                        if contributor.contributions.len() > 1 {
                            ui.add_space(2.0);
                            for item in &contributor.contributions {
                                ui.horizontal(|ui| {
                                    ui.add_space(8.0);
                                    ui.label(
                                        egui::RichText::new("•")
                                            .size(11.0)
                                            .color(crate::ui::theme::primary_dark()),
                                    );
                                    ui.label(
                                        egui::RichText::new(item)
                                            .size(12.0)
                                            .color(crate::ui::theme::text_weak()),
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

fn render_update_controls(app: &mut crate::XRTranslateApp, ui: &mut egui::Ui) {
    use crate::app_update::AppUpdateState;
    use crate::client_settings::UpdateChannel;

    let state = app.app_update_state().clone();
    let mut beta_enabled = app.update_channel == UpdateChannel::Beta;
    if ui
        .checkbox(&mut beta_enabled, "Receive beta updates")
        .on_hover_text(
            "Include prerelease builds. Beta builds can still update to stable releases.",
        )
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
        let status = match &state {
            AppUpdateState::Idle => "Ready",
            AppUpdateState::Checking => "Checking for updates...",
            AppUpdateState::Current => "You're up to date",
            AppUpdateState::Available(_) => "Update available",
            AppUpdateState::Downloading { .. } => "Downloading",
            AppUpdateState::Ready(_) => "Ready to install",
            AppUpdateState::Installing => "Installing...",
            AppUpdateState::Failed(_) => "Check failed",
        };
        ui.label(egui::RichText::new(status).color(crate::ui::theme::text_weak()));
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
                ui.label(
                    egui::RichText::new(format!(
                        "{} / {}",
                        components::format_file_size(*downloaded),
                        components::format_file_size(*total)
                    ))
                    .color(crate::ui::theme::text_strong()),
                );
            }
            _ => {}
        }
    });

    if let AppUpdateState::Failed(error) = &state {
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(error)
                .size(12.0)
                .color(egui::Color32::from_rgb(220, 38, 38)),
        );
    }

    ui.add_space(10.0);
    ui.horizontal(|ui| {
        let busy = app.app_update_manager.is_busy();
        let is_actionable = matches!(
            &state,
            AppUpdateState::Ready(_) | AppUpdateState::Available(_)
        );
        let primary_label = match &state {
            AppUpdateState::Ready(_) => "Install and Restart",
            AppUpdateState::Available(_) => "Download Update",
            AppUpdateState::Checking => "Checking...",
            AppUpdateState::Downloading { .. } => "Downloading...",
            AppUpdateState::Installing => "Installing...",
            AppUpdateState::Current => "Check for Updates",
            AppUpdateState::Failed(_) => "Try Again",
            AppUpdateState::Idle => "Check for Updates",
        };

        let btn_clicked = if is_actionable {
            components::primary_button_enabled(ui, primary_label, !busy).clicked()
        } else {
            components::animated_button_enabled(ui, primary_label, !busy).clicked()
        };

        if btn_clicked {
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
    });
}

fn render_server_section(app: &mut crate::XRTranslateApp, ui: &mut egui::Ui) {
    section(ui, crate::i18n::tr(app.ui_language, "Backend"), |ui| {
        ui.horizontal(|ui| {
            ui.label(format!(
                "{}:",
                crate::i18n::tr(app.ui_language, "Runtime Directory")
            ));
            let dir_changed = components::directory_path_input(
                ui,
                &mut app.backend_manager.runtime_directory,
                crate::i18n::tr(app.ui_language, "Choose runtime directory"),
                crate::i18n::tr(app.ui_language, "Browse…"),
                (ui.available_width() - 170.0).max(160.0),
            );
            if dir_changed {
                match app.backend_manager.save_runtime_directory() {
                    Ok(()) => app.last_error = None,
                    Err(error) => app.last_error = Some(error),
                }
            }
        });
    });

    ui.add_space(14.0);

    section(ui, crate::i18n::tr(app.ui_language, "Server"), |ui| {
        ui.horizontal(|ui| {
            ui.label("URL:");
            if ui
                .add(
                    egui::TextEdit::singleline(&mut app.server_url)
                        .desired_width((ui.available_width() - 100.0).clamp(240.0, 360.0)),
                )
                .changed()
            {
                app.save_settings();
            }
        });
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

                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new(crate::i18n::tr(language, descriptor.title_key))
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
                    });

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
