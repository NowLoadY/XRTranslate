//! Fullscreen onboarding wizard and initial setup steps.
//!
//! Provides the step-by-step setup flow for model provider configuration,
//! optional TTS voice cloning, and centralized resource download / runtime installation.

use eframe::egui::{self, Align, Color32, CornerRadius, Frame, Layout, Margin, RichText, Stroke};
use xrtranslate_assets::{ModelCapability, ModelLevel};

use crate::{
    i18n,
    model_install::{
        NativeModelPackage, NativeModelTaskState, catalog_model_packages,
        configured_model_packages, model_asset_is_present, model_level_packages_for_provider,
        set_model_level,
    },
    ui::{components, theme},
};

const STEPS: [&'static str; 4] = ["Welcome", "Configure models", "Optional TTS", "Download"];

pub fn render_onboarding_fullscreen(app: &mut crate::XRTranslateApp, ui: &mut egui::Ui) {
    let total_pages = STEPS.len();
    if app.onboarding_page >= total_pages {
        app.onboarding_page = total_pages - 1;
    }
    let requirement = crate::onboarding::evaluate_step_requirement(
        app.onboarding_page,
        &app.project_root(),
        &app.service_config,
        &app.backend_manager,
        &app.model_task_manager,
        &app.runtime_installer,
    );

    let viewport_focused = ui.input(|input| input.viewport().focused.unwrap_or(true));

    egui::Panel::bottom("onboarding_bottom_nav")
        .show_separator_line(false)
        .frame(
            Frame::new()
                .fill(theme::content_backdrop(viewport_focused))
                .inner_margin(Margin::symmetric(36, 14))
                .stroke(Stroke::NONE),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                if app.onboarding_page > 0
                    && components::animated_button(ui, i18n::tr(app.ui_language, "Back")).clicked()
                {
                    app.onboarding_page -= 1;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if app.onboarding_page + 1 == total_pages {
                        let open_translation = components::primary_button_enabled(
                            ui,
                            i18n::tr(app.ui_language, "Open Translation"),
                            requirement.is_none(),
                        )
                        .clicked();
                        ui.add_space(8.0);
                        if ui
                            .link(
                                RichText::new(i18n::tr(app.ui_language, "Usage Guidelines"))
                                    .size(12.5),
                            )
                            .clicked()
                        {
                            app.modal_dialog =
                                crate::ui::modal::ModalDialog::usage_guidelines(app.ui_language);
                        }
                        if open_translation {
                            app.finish_onboarding();
                        }
                    } else if components::primary_button_enabled(
                        ui,
                        i18n::tr(app.ui_language, "Continue"),
                        requirement.is_none(),
                    )
                    .clicked()
                    {
                        app.onboarding_page += 1;
                    }
                    if let Some(hint) = requirement {
                        ui.label(
                            RichText::new(i18n::tr(app.ui_language, hint))
                                .size(12.0)
                                .color(theme::text_weak()),
                        );
                    }
                });
            });
        });

    egui::CentralPanel::default()
        .frame(
            Frame::new()
                .fill(theme::content_backdrop(viewport_focused))
                .inner_margin(Margin::symmetric(36, 20))
                .stroke(Stroke::NONE),
        )
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("XRTranslate")
                            .size(14.0)
                            .color(theme::primary())
                            .strong(),
                    );
                    ui.add_space(6.0);
                    let github_icon = egui::Image::new(egui::include_image!(
                        "../../../resources/icons/github.svg"
                    ))
                    .fit_to_exact_size(egui::vec2(20.0, 20.0))
                    .tint(theme::text_weak());

                    let github_btn = ui
                        .add(egui::Button::image(github_icon).frame(false))
                        .on_hover_text("GitHub: NowLoadY/XRTranslate")
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                    if github_btn.clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(
                            "https://github.com/NowLoadY/XRTranslate",
                        ));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!(
                                "{}/{}",
                                i18n::tr(
                                    app.ui_language,
                                    match app.onboarding_page {
                                        0 => "Step 1",
                                        1 => "Step 2",
                                        2 => "Step 3",
                                        _ => "Step 4",
                                    }
                                ),
                                total_pages
                            ))
                            .size(13.0)
                            .color(theme::text_weak()),
                        );
                        ui.add_space(8.0);
                        let mut language = app.ui_language;
                        if components::language_selector(
                            ui,
                            "onboarding_ui_language",
                            &mut language,
                        ) {
                            app.set_ui_language(language);
                        }
                        ui.add_space(8.0);
                        let mut proxy = app.download_proxy_url.clone();
                        let proxy_response = components::singleline_input(
                            ui,
                            &mut proxy,
                            i18n::tr(app.ui_language, "Proxy, e.g. http://127.0.0.1:7890"),
                            220.0,
                            false,
                        );
                        if proxy_response.lost_focus() || proxy_response.changed() {
                            app.set_download_proxy_url(proxy);
                        }
                    });
                });
                ui.add_space(8.0);
                ui.label(
                    RichText::new(i18n::tr(
                        app.ui_language,
                        "A calm start, one step at a time",
                    ))
                    .size(22.0)
                    .color(theme::text_strong())
                    .strong(),
                );
                ui.add_space(20.0);

                render_onboarding_steps(ui, app.ui_language, &STEPS, app.onboarding_page);

                ui.add_space(28.0);

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .id_salt("onboarding_content_scroll")
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        let cur = app.onboarding_page;
                        crate::ui::animation::AnimationSystem::render_page_flip_transition(
                            ui,
                            cur,
                            |ui| match cur {
                                0 => render_onboarding_welcome(app.ui_language, ui),
                                1 => render_onboarding_models(app, ui),
                                2 => render_onboarding_tts(app, ui),
                                _ => render_onboarding_download(app, ui),
                            },
                        );
                    });
            });
        });
}

fn render_onboarding_steps(
    ui: &mut egui::Ui,
    language: i18n::UiLanguage,
    steps: &[&'static str],
    current: usize,
) {
    ui.horizontal(|ui| {
        for (i, title) in steps.iter().enumerate() {
            let active = i == current;
            let visited = i < current;
            let fill = if active {
                theme::primary()
            } else if visited {
                Color32::from_rgb(219, 234, 254)
            } else {
                Color32::from_rgb(241, 245, 249)
            };
            let text_color = if active {
                Color32::WHITE
            } else if visited {
                theme::primary_dark()
            } else {
                theme::text_weak()
            };
            let stroke = if active {
                Stroke::NONE
            } else if visited {
                Stroke::new(1.0, Color32::from_rgb(147, 197, 253))
            } else {
                Stroke::new(1.0, theme::border())
            };
            Frame::new()
                .fill(fill)
                .stroke(stroke)
                .corner_radius(CornerRadius::same(12))
                .inner_margin(Margin::symmetric(14, 8))
                .show(ui, |ui| {
                    let label = i18n::tr(language, *title);
                    ui.label(
                        RichText::new(format!("{} {}", i + 1, label))
                            .size(12.5)
                            .color(text_color)
                            .strong(),
                    );
                });

            if i + 1 < steps.len() {
                ui.add_space(4.0);
            }
        }
    });
}

fn onboarding_title(
    ui: &mut egui::Ui,
    language: crate::i18n::UiLanguage,
    title: &'static str,
    body: Option<&'static str>,
) {
    ui.label(
        RichText::new(crate::i18n::tr(language, title))
            .size(28.0)
            .color(theme::text_strong())
            .strong(),
    );
    if let Some(subtitle) = body {
        ui.add_space(6.0);
        ui.label(
            RichText::new(crate::i18n::tr(language, subtitle))
                .size(14.0)
                .color(theme::text_weak()),
        );
    }
    ui.add_space(20.0);
}

fn onboarding_feature_card(
    ui: &mut egui::Ui,
    title: &'static str,
    description: &'static str,
    stroke_color: Color32,
    language: crate::i18n::UiLanguage,
) {
    let border_id = ui.make_persistent_id(("onboarding_feature_border", title));
    crate::ui::organic_border::show(
        ui,
        border_id,
        Frame::new()
            .fill(theme::surface_control())
            .corner_radius(CornerRadius::same(14))
            .inner_margin(Margin::symmetric(22, 20)),
        14.0,
        stroke_color,
        |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height(108.0);

            ui.label(
                RichText::new(crate::i18n::tr(language, title))
                    .size(16.0)
                    .color(theme::text_strong())
                    .strong(),
            );

            ui.add_space(8.0);
            ui.label(
                RichText::new(crate::i18n::tr(language, description))
                    .size(13.0)
                    .color(theme::text_weak())
                    .line_height(Some(19.0)),
            );
        },
    );
}

fn render_onboarding_welcome(language: crate::i18n::UiLanguage, ui: &mut egui::Ui) {
    onboarding_title(
        ui,
        language,
        "Welcome to XRTranslate",
        Some("Your modular, real-time speech recognition, translation, and VR immersion platform."),
    );
    ui.columns(3, |columns| {
        onboarding_feature_card(
            &mut columns[0],
            "Audio Input",
            "Microphone & desktop audio capture with AI noise suppression and VAD detection.",
            Color32::from_rgb(59, 130, 246),
            language,
        );
        onboarding_feature_card(
            &mut columns[1],
            "Recognition & Translation",
            "High-accuracy real-time speech translation powered by local models or cloud APIs.",
            Color32::from_rgb(16, 185, 129),
            language,
        );
        onboarding_feature_card(
            &mut columns[2],
            "Plugins & Integrations",
            "VRChat OSC sync, desktop floating subtitles, and meeting minutes recording.",
            Color32::from_rgb(245, 158, 11),
            language,
        );
    });
}

// ---------------------------------------------------------------------------
// Step 2: Configure Models (Pure configuration without download buttons)
// ---------------------------------------------------------------------------

fn render_onboarding_models(app: &mut crate::XRTranslateApp, ui: &mut egui::Ui) {
    let language = app.ui_language;
    onboarding_title(
        ui,
        language,
        "Configure model providers",
        Some(
            "Select local models or cloud APIs for speech recognition and translation. Required model packages will be downloaded in the final step.",
        ),
    );
    let project_root = app.project_root();
    let packages = match configured_model_packages(&project_root) {
        Ok(packages) => packages,
        Err(error) => {
            app.last_error = Some(error);
            Vec::new()
        }
    };
    let mut level_change = None;
    let mut provider_change = None;
    let mut remote_fields = None;
    let mut delete_model = None;
    let local_availability = app.runtime_installer.local_model_availability();
    let capabilities = [
        ("asr", ModelCapability::Asr, "Speech Recognition Model"),
        (
            "translation",
            ModelCapability::Translation,
            "Translation Model",
        ),
    ];
    ui.columns(capabilities.len(), |columns| {
        for (index, (category, capability, title)) in capabilities.iter().enumerate() {
            let package = packages
                .iter()
                .find(|package| package.capability == *capability);
            let provider = app.service_config.onboarding_provider_state(category);
            let levels = package.map_or_else(Vec::new, |package| {
                model_level_packages_for_provider(package.provider, package.capability)
            });
            let result = onboarding_model_config_card(
                &mut columns[index],
                language,
                category,
                title,
                &project_root,
                provider,
                package.map(|package| package.level),
                &levels,
                !app.model_task_manager.is_busy(),
                &local_availability,
                if index % 2 == 0 {
                    Color32::from_rgb(59, 130, 246)
                } else {
                    Color32::from_rgb(16, 185, 129)
                },
            );
            if let Some(level) = result.selected_level {
                level_change = Some((*capability, level));
            }
            if let Some(provider) = result.selected_provider {
                provider_change = Some((*category, provider));
            }
            if let Some(fields) = result.remote_fields {
                remote_fields = Some((*category, fields));
            }
            if let Some(asset_id) = result.delete_asset {
                delete_model = Some(asset_id);
            }
        }
    });
    if let Some(asset_id) = delete_model {
        app.request_model_resource_deletion(asset_id);
    }
    if let Some((category, provider)) = provider_change {
        app.service_config
            .select_onboarding_provider(category, &provider);
        let result = app.service_config.save_onboarding_configuration();
        handle_onboarding_save(app, result);
    }
    if let Some((category, fields)) = remote_fields {
        app.service_config
            .set_onboarding_remote_fields(category, fields.model, fields.api_key);
        if fields.commit {
            let result = app.service_config.save_onboarding_configuration();
            handle_onboarding_save(app, result);
        }
    }
    if let Some(message) = app.service_config.onboarding_message() {
        ui.add_space(10.0);
        ui.label(
            RichText::new(message)
                .size(12.0)
                .color(Color32::from_rgb(220, 38, 38)),
        );
    }
    if let Some((capability, level)) = level_change {
        match set_model_level(&project_root, capability, level) {
            Ok(()) => {
                app.model_task_manager.invalidate_discovery();
                app.backend_manager.shutdown();
                app.last_error = None;
            }
            Err(error) => app.last_error = Some(error),
        }
    }
}

#[derive(Default)]
struct ModelConfigCardResult {
    selected_level: Option<ModelLevel>,
    selected_provider: Option<String>,
    remote_fields: Option<RemoteProviderFields>,
    delete_asset: Option<xrtranslate_assets::ModelAssetId>,
}

struct RemoteProviderFields {
    model: String,
    api_key: String,
    commit: bool,
}

fn local_model_warning_icon(
    ui: &mut egui::Ui,
    language: i18n::UiLanguage,
    availability: &crate::runtime_install::LocalModelAvailability,
) {
    let tooltip = match availability {
        crate::runtime_install::LocalModelAvailability::InsufficientVram {
            gpu,
            memory_bytes,
            required_bytes,
        } => format!(
            "{}\n\n{gpu}: {:.1} GiB / {:.0} GiB",
            i18n::tr(
                language,
                "Your GPU has less than 8 GiB of VRAM. Local models require at least 8 GiB, so this option is disabled."
            ),
            *memory_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
            *required_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
        ),
        crate::runtime_install::LocalModelAvailability::Unavailable(reason) => format!(
            "{}\n\n{}",
            i18n::tr(
                language,
                "Local models are unavailable on this device. You can continue with an online API, or update the NVIDIA driver or GPU and try again."
            ),
            reason
        ),
        crate::runtime_install::LocalModelAvailability::Detecting
        | crate::runtime_install::LocalModelAvailability::Available { .. } => return,
    };
    let icon = egui::Image::new(egui::include_image!(
        "../../../resources/icons/alert-triangle.svg"
    ))
    .fit_to_exact_size(egui::vec2(18.0, 18.0))
    .tint(Color32::from_rgb(217, 119, 6))
    .sense(egui::Sense::hover());
    ui.add(icon).on_hover_text(tooltip);
}

fn onboarding_model_config_card(
    ui: &mut egui::Ui,
    language: i18n::UiLanguage,
    category: &'static str,
    title: &'static str,
    project_root: &std::path::Path,
    mut provider: Option<crate::service_config::OnboardingProviderState>,
    selected_level: Option<ModelLevel>,
    levels: &[NativeModelPackage],
    delete_enabled: bool,
    local_availability: &crate::runtime_install::LocalModelAvailability,
    stroke_color: Color32,
) -> ModelConfigCardResult {
    let mut result = ModelConfigCardResult::default();
    let border_id = ui.make_persistent_id(("onboarding_model_config_border", category));
    crate::ui::organic_border::show(
        ui,
        border_id,
        Frame::new()
            .fill(theme::surface_control())
            .corner_radius(CornerRadius::same(16))
            .inner_margin(Margin::same(18)),
        16.0,
        stroke_color,
        |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new(i18n::tr(language, title))
                    .size(16.0)
                    .color(theme::text_strong())
                    .strong(),
            );
            ui.add_space(10.0);
            let Some(provider) = provider.as_mut() else {
                ui.label(i18n::tr(language, "No providers configured"));
                return;
            };

            ui.horizontal(|ui| {
                ui.label(i18n::tr(language, "Mode"));
                let mut remote = provider.remote;
                let mode_text = i18n::tr(
                    language,
                    if remote { "Online API" } else { "Local model" },
                );
                components::combobox_ui(ui, (category, "provider_mode"), mode_text, |ui| {
                        let local_available = matches!(
                            local_availability,
                            crate::runtime_install::LocalModelAvailability::Available { .. }
                        );
                        ui.add_enabled_ui(local_available, |ui| {
                            ui.selectable_value(
                                &mut remote,
                                false,
                                i18n::tr(language, "Local model"),
                            );
                        });
                        ui.selectable_value(&mut remote, true, i18n::tr(language, "Online API"));
                    });
                local_model_warning_icon(ui, language, local_availability);
                if remote != provider.remote
                    && let Some(choice) = provider
                        .choices
                        .iter()
                        .find(|choice| choice.remote == remote)
                {
                    result.selected_provider = Some(choice.name.clone());
                }

                if !provider.remote {
                    let Some(mut level) = selected_level else {
                        return;
                    };
                    ui.add_space(12.0);
                    ui.label(i18n::tr(language, "Level"));
                    let selected_present = levels
                        .iter()
                        .find(|package| package.level == level)
                        .is_some_and(|package| {
                            model_asset_is_present(project_root, package.id).unwrap_or(false)
                        });
                    let selected_label = if selected_present {
                        format!(
                            "{} · {}",
                            i18n::tr(language, level.as_str()),
                            i18n::tr(language, "Installed")
                        )
                    } else {
                        i18n::tr(language, level.as_str()).to_owned()
                    };
                    let local_available = matches!(
                        local_availability,
                        crate::runtime_install::LocalModelAvailability::Available { .. }
                    );
                    ui.add_enabled_ui(local_available, |ui| {
                        components::combobox_ui(ui, (category, "model_level"), selected_label, |ui| {
                                for package in levels {
                                    let present = model_asset_is_present(project_root, package.id)
                                        .unwrap_or(false);
                                    ui.horizontal(|ui| {
                                        let label = if present {
                                            format!(
                                                "{} · {}",
                                                i18n::tr(language, package.level.as_str()),
                                                i18n::tr(language, "Installed")
                                            )
                                        } else {
                                            i18n::tr(language, package.level.as_str()).to_owned()
                                        };
                                        ui.selectable_value(&mut level, package.level, label);
                                        if present
                                            && ui
                                                .add_enabled_ui(delete_enabled, |ui| {
                                                    components::resource_delete_button(
                                                        ui,
                                                        package.id,
                                                        language,
                                                    )
                                                })
                                                .inner
                                                .clicked()
                                        {
                                            result.delete_asset = Some(package.id);
                                            ui.close();
                                        }
                                    });
                                }
                            });
                    });
                    if Some(level) != selected_level {
                        result.selected_level = Some(level);
                    }
                } else {
                    ui.add_space(12.0);
                    ui.label(i18n::tr(language, "Provider:"));
                    components::combobox_ui(ui, (category, "online_provider"), &provider.selected, |ui| {
                            for choice in &provider.choices {
                                if choice.remote
                                    && ui
                                        .selectable_label(
                                            provider.selected == choice.name,
                                            &choice.name,
                                        )
                                        .clicked()
                                {
                                    result.selected_provider = Some(choice.name.clone());
                                }
                            }
                        });
                }
            });

            if provider.remote {
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.label(i18n::tr(language, "Model"));
                    let model_response = components::singleline_input(
                        ui,
                        &mut provider.model,
                        i18n::tr(language, "Model"),
                        (ui.available_width() - 60.0).max(160.0),
                        false,
                    );
                    if model_response.changed() || model_response.lost_focus() {
                        result.remote_fields = Some(RemoteProviderFields {
                            model: provider.model.clone(),
                            api_key: provider.api_key.clone(),
                            commit: model_response.lost_focus(),
                        });
                    }
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(i18n::tr(language, "API key"));
                    let key_response = components::singleline_input(
                        ui,
                        &mut provider.api_key,
                        i18n::tr(language, "API key"),
                        (ui.available_width() - 70.0).max(160.0),
                        true,
                    );
                    let commit = key_response.lost_focus()
                        || ui.input(|input| input.key_pressed(egui::Key::Enter));
                    if key_response.changed() || commit {
                        result.remote_fields = Some(RemoteProviderFields {
                            model: provider.model.clone(),
                            api_key: provider.api_key.clone(),
                            commit,
                        });
                    }
                });
            } else {
                match local_availability {
                    crate::runtime_install::LocalModelAvailability::InsufficientVram { .. } => {}
                    crate::runtime_install::LocalModelAvailability::Unavailable(_) => {}
                    crate::runtime_install::LocalModelAvailability::Detecting => {
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new(i18n::tr(language, "Detecting NVIDIA GPU…"))
                                .size(11.5)
                                .color(theme::text_weak()),
                        );
                    }
                    crate::runtime_install::LocalModelAvailability::Available { .. } => {}
                }
            }
        },
    );
    result
}

// ---------------------------------------------------------------------------
// Step 3: Optional TTS (Pure configuration without download button)
// ---------------------------------------------------------------------------

fn render_onboarding_tts(app: &mut crate::XRTranslateApp, ui: &mut egui::Ui) {
    let language = app.ui_language;
    onboarding_title(
        ui,
        language,
        "Optional text-to-speech",
        Some(
            "Choose Skip to keep translated text only, or select a voice-cloning provider. The model will be downloaded in the final step.",
        ),
    );
    let provider = app.service_config.onboarding_provider_state("tts");
    let mut selected_provider = None;
    let project_root = app.project_root();
    let mut delete_tts = None;
    let mut model_change = None;
    let mut voice_change = None;

    let tts_border_color = Color32::from_rgb(244, 63, 94);
    let border_id = ui.make_persistent_id("onboarding_tts_border");
    crate::ui::organic_border::show(
        ui,
        border_id,
        Frame::new()
            .fill(theme::surface_control())
            .corner_radius(CornerRadius::same(16))
            .inner_margin(Margin::same(18)),
        16.0,
        tts_border_color,
        |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new(i18n::tr(language, "Voice cloning & speech synthesis"))
                    .size(16.0)
                    .color(theme::text_strong())
                    .strong(),
            );
            ui.add_space(10.0);
            let Some(provider) = provider else {
                ui.label(i18n::tr(language, "No providers configured"));
                return;
            };
            ui.horizontal(|ui| {
                ui.label(i18n::tr(language, "Provider:"));
                let local_availability = app.runtime_installer.local_model_availability();
                let selected_label = provider
                    .choices
                    .iter()
                    .find(|choice| choice.name == provider.selected)
                    .map(|choice| provider_choice_resource(choice, &project_root, language).0)
                    .unwrap_or_else(|| provider.selected.clone());
                components::combobox_ui(ui, ("tts", "provider"), selected_label, |ui| {
                        for choice in &provider.choices {
                            let (label, _asset_id, _present) =
                                provider_choice_resource(choice, &project_root, language);
                            let local_available = matches!(
                                &local_availability,
                                crate::runtime_install::LocalModelAvailability::Available { .. }
                            );
                            ui.add_enabled_ui(
                                choice.name == "none" || choice.remote || local_available,
                                |ui| {
                                    if ui
                                        .selectable_label(provider.selected == choice.name, label)
                                        .clicked()
                                    {
                                        selected_provider = Some(choice.name.clone());
                                    }
                                },
                            );
                        }
                    });
                local_model_warning_icon(ui, language, &local_availability);
            });
            ui.add_space(10.0);
            if provider.selected == "none" {
                ui.label(
                    RichText::new(i18n::tr(
                        language,
                        "TTS disabled. Translated subtitles will be displayed on screen without voice playback.",
                    ))
                    .size(12.5)
                    .color(theme::text_weak()),
                );
            } else {
                ui.label(
                    RichText::new(i18n::tr(
                        language,
                        "The selected provider supplies local voice cloning and real-time speech playback.",
                    ))
                    .size(12.5)
                    .color(theme::text_weak()),
                );
                let selected_choice = provider
                    .choices
                    .iter()
                    .find(|choice| choice.name == provider.selected);
                let packages = crate::model_install::model_packages_for_provider(
                    &provider.selected,
                    ModelCapability::Tts,
                );
                if !packages.is_empty() {
                    ui.add_space(5.0);
                    ui.label(
                        RichText::new(i18n::tr(language, "Synthesis language models:"))
                            .size(12.5)
                            .color(theme::text_weak()),
                    );
                    let selected_assets = selected_choice
                        .map(|choice| choice.model_assets.as_slice())
                        .unwrap_or_default();
                    let availability = app.runtime_installer.local_model_availability();
                    for package in packages {
                        let checked = selected_assets
                            .iter()
                            .any(|asset| asset == package.id.as_str());
                        let package_enabled = matches!(
                            (&availability, package.hardware.accelerator),
                            (
                                crate::runtime_install::LocalModelAvailability::Available {
                                    memory_bytes,
                                    ..
                                },
                                xrtranslate_assets::ModelAccelerator::NvidiaCuda
                            ) if *memory_bytes >= package.hardware.minimum_memory_bytes
                        );
                        let may_toggle = package_enabled
                            && (!checked || selected_assets.len() > 1)
                            && !app.model_task_manager.is_busy();
                        ui.horizontal(|ui| {
                            let mut next = checked;
                            let language_pack = if package.languages.is_empty() {
                                package.label.to_owned()
                            } else {
                                format!("{} — {}", package.languages.join(", "), package.label)
                            };
                            if ui
                                .add_enabled(
                                    may_toggle,
                                    egui::Checkbox::new(&mut next, language_pack),
                                )
                                .changed()
                            {
                                model_change = Some((
                                    provider.selected.clone(),
                                    package.id.as_str().to_owned(),
                                    next,
                                ));
                            }
                            let present =
                                model_asset_is_present(&project_root, package.id).unwrap_or(false);
                            if present {
                                ui.label(
                                    RichText::new(i18n::tr(language, "Installed"))
                                        .size(11.5)
                                        .color(theme::text_weak()),
                                );
                                if components::resource_delete_button(
                                    ui,
                                    package.id,
                                    language,
                                )
                                .clicked()
                                {
                                    delete_tts = Some(package.id);
                                }
                            }
                        });
                    }
                    if let Some(choice) = selected_choice {
                        for asset in &choice.model_assets {
                            let Some(id) = xrtranslate_assets::ModelAssetId::from_config_key(asset)
                            else {
                                continue;
                            };
                            let manifest = xrtranslate_assets::manifest_for(id);
                            let mut voice_languages = manifest
                                .voice_presets
                                .iter()
                                .map(|preset| preset.language)
                                .collect::<Vec<_>>();
                            voice_languages.sort_unstable();
                            voice_languages.dedup();
                            for voice_language in voice_languages {
                                let choices = manifest
                                    .voice_presets
                                    .iter()
                                    .filter(|preset| preset.language == voice_language)
                                    .collect::<Vec<_>>();
                                let Some(default) = choices
                                    .iter()
                                    .copied()
                                    .find(|preset| preset.is_default)
                                    .or_else(|| choices.first().copied())
                                else {
                                    continue;
                                };
                                let configured = choice
                                    .voices
                                    .get(voice_language)
                                    .and_then(|key| {
                                        choices.iter().copied().find(|preset| preset.key == key)
                                    })
                                    .unwrap_or(default);
                                let mut selected_key = configured.key.to_owned();
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(format!(
                                            "{} ({voice_language}):",
                                            i18n::tr(language, "Base voice / accent")
                                        ))
                                        .size(12.0)
                                        .color(theme::text_weak()),
                                    );
                                    components::combobox_ui(
                                        ui,
                                        ("onboarding_tts_voice", &provider.selected, id),
                                        configured.label,
                                        |ui| {
                                            for preset in &choices {
                                                ui.selectable_value(
                                                    &mut selected_key,
                                                    preset.key.to_owned(),
                                                    preset.label,
                                                );
                                            }
                                        },
                                    );
                                });
                                if selected_key != configured.key {
                                    voice_change = Some((
                                        provider.selected.clone(),
                                        voice_language.to_owned(),
                                        selected_key,
                                    ));
                                }
                            }
                        }
                    }
                    match availability {
                        crate::runtime_install::LocalModelAvailability::InsufficientVram {
                            ..
                        } => {}
                        crate::runtime_install::LocalModelAvailability::Unavailable(_) => {}
                        crate::runtime_install::LocalModelAvailability::Detecting => {
                            ui.label(
                                RichText::new(i18n::tr(language, "Detecting NVIDIA GPU…"))
                                    .size(11.5)
                                    .color(theme::text_weak()),
                            );
                        }
                        crate::runtime_install::LocalModelAvailability::Available { .. } => {}
                    }
                } else if let Some(languages) = selected_choice
                    .map(|choice| choice.supported_languages.as_slice())
                    .filter(|languages| !languages.is_empty())
                {
                    ui.add_space(5.0);
                    ui.label(
                        RichText::new(format!(
                            "{} {}",
                            i18n::tr(language, "Supported synthesis languages:"),
                            languages.join(", ")
                        ))
                        .size(12.5)
                        .color(theme::text_weak()),
                    );
                }
            }
        },
    );

    if let Some(asset_id) = delete_tts {
        app.request_model_resource_deletion(asset_id);
    }

    if let Some((provider, asset, enabled)) = model_change {
        app.service_config
            .set_onboarding_model_enabled("tts", &provider, &asset, enabled);
        let result = app.service_config.save_onboarding_configuration();
        handle_onboarding_save(app, result);
    }
    if let Some((provider, voice_language, preset)) = voice_change {
        app.service_config
            .set_onboarding_voice_preset(&provider, &voice_language, &preset);
        let result = app.service_config.save_onboarding_configuration();
        handle_onboarding_save(app, result);
    }

    if let Some(selected) = selected_provider {
        app.service_config
            .select_onboarding_provider("tts", &selected);
        let result = app.service_config.save_onboarding_configuration();
        handle_onboarding_save(app, result);
    }
}

fn handle_onboarding_save(
    app: &mut crate::XRTranslateApp,
    result: Result<crate::service_config::OnboardingSaveOutcome, String>,
) {
    use crate::service_config::OnboardingSaveOutcome;

    match result {
        Ok(OnboardingSaveOutcome::Saved { resolved_error }) => {
            if resolved_error.as_ref() == app.last_error.as_ref() {
                app.last_error = None;
            }
        }
        Ok(OnboardingSaveOutcome::IncompleteRemoteProvider) => {}
        Err(error) => app.last_error = Some(error),
    }
}

fn provider_choice_resource(
    choice: &crate::service_config::OnboardingProviderChoice,
    project_root: &std::path::Path,
    language: i18n::UiLanguage,
) -> (String, Option<xrtranslate_assets::ModelAssetId>, bool) {
    if choice.name == "none" {
        return (i18n::tr(language, "Skip").to_owned(), None, false);
    }
    let asset_id = choice
        .model_asset
        .as_deref()
        .and_then(xrtranslate_assets::ModelAssetId::from_config_key);
    let base = asset_id.map_or_else(
        || choice.name.clone(),
        |asset_id| xrtranslate_assets::manifest_for(asset_id).label.to_owned(),
    );
    let present = asset_id
        .is_some_and(|asset_id| model_asset_is_present(project_root, asset_id).unwrap_or(false));
    let label = if present {
        format!("{base} · {}", i18n::tr(language, "Installed"))
    } else {
        base
    };
    (label, asset_id, present)
}

// ---------------------------------------------------------------------------
// Step 4: Centralized Download (Dynamic model cards & runtime acceleration)
// ---------------------------------------------------------------------------

struct DownloadItem {
    id: xrtranslate_assets::ModelAssetId,
    category_title: &'static str,
    detail: String,
    download_bytes: u64,
    installed_bytes: u64,
    installed: bool,
    hardware_available: bool,
    stroke_color: Color32,
}

fn render_onboarding_download(app: &mut crate::XRTranslateApp, ui: &mut egui::Ui) {
    let language = app.ui_language;
    let project_root = app.project_root();

    if app.model_task_manager.needs_discovery()
        && let Err(error) = app
            .model_task_manager
            .discover_existing(project_root.clone())
    {
        app.last_error = Some(error);
    }

    let requirements = app.service_config.runtime_requirements();
    if !app.runtime_installer.is_busy()
        && !app.runtime_installer.plan_matches(requirements)
        && let Err(error) = app
            .runtime_installer
            .prepare_for(project_root.clone(), requirements)
    {
        app.last_error = Some(error);
    }

    onboarding_title(
        ui,
        language,
        "Download required resources",
        Some(
            "Download the configured model packages and native inference runtime for your system.",
        ),
    );

    let busy = app.model_task_manager.is_busy();
    let mut installs = Vec::new();
    let mut delete_model = None;

    // 1. Model packages (ASR, MT, TTS)
    let mut packages = match configured_model_packages(&project_root) {
        Ok(packages) => packages,
        Err(error) => {
            app.last_error = Some(error);
            Vec::new()
        }
    };
    if let Ok(catalog) = catalog_model_packages(&project_root) {
        for package in catalog {
            if !packages.iter().any(|active| active.id == package.id)
                && model_asset_is_present(&project_root, package.id).unwrap_or(false)
            {
                packages.push(package);
            }
        }
    }

    let local_availability = app.runtime_installer.local_model_availability();
    let download_items = packages
        .iter()
        .map(|package| {
            let installed = app.model_task_manager.is_model_present(package.id)
                || app.model_task_manager.is_model_ready(package.id)
                || model_asset_is_present(&project_root, package.id).unwrap_or(false);
            let (category_title, stroke_color) = match package.capability {
                ModelCapability::Asr => {
                    ("Speech Recognition Model", Color32::from_rgb(59, 130, 246))
                }
                ModelCapability::Translation => {
                    ("Translation Model", Color32::from_rgb(16, 185, 129))
                }
                ModelCapability::Tts => {
                    ("Voice Cloning & TTS Model", Color32::from_rgb(244, 63, 94))
                }
            };
            DownloadItem {
                id: package.id,
                category_title,
                detail: format!(
                    "{} · {}",
                    package.label,
                    i18n::tr(language, package.level.as_str())
                ),
                download_bytes: package.download_bytes,
                installed_bytes: package.installed_bytes,
                installed,
                hardware_available: matches!(
                    (&local_availability, package.hardware.accelerator),
                    (
                        crate::runtime_install::LocalModelAvailability::Available {
                            memory_bytes,
                            ..
                        },
                        xrtranslate_assets::ModelAccelerator::NvidiaCuda
                    ) if *memory_bytes >= package.hardware.minimum_memory_bytes
                ),
                stroke_color,
            }
        })
        .collect::<Vec<_>>();

    if !download_items.is_empty() {
        let missing = download_items
            .iter()
            .filter(|item| !item.installed && item.hardware_available)
            .collect::<Vec<_>>();
        let missing_download_bytes = missing.iter().map(|item| item.download_bytes).sum::<u64>();
        let missing_installed_bytes = missing.iter().map(|item| item.installed_bytes).sum::<u64>();
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(i18n::tr(language, "Model Packages"))
                    .size(15.0)
                    .color(theme::text_strong())
                    .strong(),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let previous_source = app.model_task_manager.use_mirror();
                let mut use_mirror = previous_source;
                components::download_mirror_toggle(ui, language, &mut use_mirror);
                if use_mirror != previous_source
                    && let Err(error) = app
                        .model_task_manager
                        .switch_download_source(project_root.clone(), use_mirror)
                {
                    app.last_error = Some(error);
                }
            });
        });
        if !missing.is_empty() {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let label = format!(
                    "{} ({}) · {}",
                    i18n::tr(language, "Download all required models"),
                    missing.len(),
                    components::format_file_size(missing_download_bytes),
                );
                if components::primary_button_enabled(ui, &label, !app.runtime_installer.is_busy())
                    .clicked()
                {
                    installs.extend(missing.iter().map(|item| item.id));
                }
                ui.label(
                    RichText::new(format!(
                        "{}: {}",
                        i18n::tr(language, "Installed size"),
                        components::format_file_size(missing_installed_bytes)
                    ))
                    .size(12.0)
                    .color(theme::text_weak()),
                );
            });
        }
        ui.add_space(8.0);

        for item in &download_items {
            let batch = app.model_task_manager.batch_snapshot();
            let is_active =
                batch.as_ref().and_then(|batch| batch.current_asset_id) == Some(item.id) && busy;
            let queued_position = batch.as_ref().and_then(|batch| {
                batch
                    .queued_packages
                    .iter()
                    .position(|id| *id == item.id)
                    .map(|position| position + 1)
            });
            let failed = batch.as_ref().and_then(|batch| batch.failed_asset_id) == Some(item.id);
            let action = if item.installed {
                "Installed"
            } else if is_active {
                "Downloading"
            } else if queued_position.is_some() {
                "Queued"
            } else if failed {
                "Retry"
            } else {
                "Download"
            };
            let (clicked, delete_clicked) = render_download_card(
                ui,
                language,
                item,
                action,
                !item.installed
                    && !is_active
                    && queued_position.is_none()
                    && item.hardware_available
                    && !app.runtime_installer.is_busy(),
                !busy,
                queued_position,
                is_active.then_some(app.model_task_manager.state()),
            );
            if clicked {
                installs.push(item.id);
            }
            if delete_clicked {
                delete_model = Some(item.id);
            }
            ui.add_space(8.0);
        }
    } else {
        Frame::new()
            .fill(theme::surface_control())
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::same(16))
            .stroke(Stroke::new(1.5, Color32::from_rgb(16, 185, 129)))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new(i18n::tr(
                        language,
                        "All services use cloud APIs. No local models or inference runtimes are required.",
                    ))
                    .size(13.5)
                    .color(Color32::from_rgb(4, 120, 87))
                    .strong(),
                );
            });
    }

    if !installs.is_empty()
        && let Err(error) = app
            .model_task_manager
            .enqueue_many(project_root.clone(), installs)
    {
        app.last_error = Some(error);
    }
    if let Some(asset_id) = delete_model {
        app.request_model_resource_deletion(asset_id);
    }

    render_model_task_state(
        ui,
        language,
        app.model_task_manager.state(),
        app.model_task_manager.batch_snapshot().as_ref(),
        &download_items,
    );

    // 2. Inference Runtime & Hardware Acceleration (if local models are configured)
    let requires_runtime = requirements.llama_cpp || requirements.onnx_tts;
    if requires_runtime {
        ui.add_space(16.0);
        ui.label(
            RichText::new(i18n::tr(
                language,
                "Inference Runtime & Hardware Acceleration",
            ))
            .size(15.0)
            .color(theme::text_strong())
            .strong(),
        );
        ui.add_space(8.0);

        render_runtime_installation_section(app, ui, language, &project_root);
    }
}

fn render_download_card(
    ui: &mut egui::Ui,
    language: i18n::UiLanguage,
    item: &DownloadItem,
    action: &'static str,
    enabled: bool,
    delete_enabled: bool,
    queued_position: Option<usize>,
    active_state: Option<&NativeModelTaskState>,
) -> (bool, bool) {
    let mut clicked = false;
    let mut delete_clicked = false;
    let border_id = ui.make_persistent_id(("onboarding_download_border", item.id));
    crate::ui::organic_border::show(
        ui,
        border_id,
        Frame::new()
            .fill(theme::surface_control())
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::symmetric(16, 10)),
        12.0,
        item.stroke_color,
        |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(i18n::tr(language, item.category_title))
                            .size(14.5)
                            .color(theme::text_strong())
                            .strong(),
                    );
                    ui.label(
                        RichText::new(&item.detail)
                            .size(13.0)
                            .color(theme::text_weak()),
                    );
                    ui.label(
                        RichText::new(format!(
                            "· {} {} · {} {}",
                            i18n::tr(language, "Download"),
                            components::format_file_size(item.download_bytes),
                            i18n::tr(language, "Installed size"),
                            components::format_file_size(item.installed_bytes),
                        ))
                        .size(12.0)
                        .color(theme::text_weak()),
                    );
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if item.installed {
                        if ui
                            .add_enabled_ui(delete_enabled, |ui| {
                                components::resource_delete_button(
                                    ui,
                                    item.id,
                                    language,
                                )
                            })
                            .inner
                            .clicked()
                        {
                            delete_clicked = true;
                        }
                        Frame::new()
                            .fill(Color32::from_rgba_unmultiplied(16, 185, 129, 20))
                            .stroke(Stroke::new(1.0, Color32::from_rgba_unmultiplied(16, 185, 129, 100)))
                            .corner_radius(CornerRadius::same(8))
                            .inner_margin(Margin::symmetric(14, 5))
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(i18n::tr(language, "Installed"))
                                        .color(Color32::from_rgb(5, 150, 105))
                                        .size(12.5)
                                        .strong(),
                                );
                            });
                    } else {
                        let action_label = queued_position.map_or_else(
                            || {
                                format!(
                                    "{} · {}",
                                    i18n::tr(language, action),
                                    components::format_file_size(item.download_bytes),
                                )
                            },
                            |position| format!("{} #{}", i18n::tr(language, action), position),
                        );
                        if components::primary_button_enabled_with_id(
                            ui,
                            ("download_card", item.id),
                            &action_label,
                            enabled,
                        )
                        .clicked()
                        {
                            clicked = true;
                        }
                    }
                });
            });
            if let Some(NativeModelTaskState::Installing {
                relative_path,
                downloaded_bytes,
                total_bytes,
                ..
            }) = active_state
            {
                ui.add_space(5.0);
                if *total_bytes > 0 {
                    let finishing = downloaded_bytes >= total_bytes;
                    ui.add(
                        egui::ProgressBar::new(
                            (*downloaded_bytes as f64 / *total_bytes as f64).clamp(0.0, 1.0) as f32,
                        )
                        .text(if finishing {
                            i18n::tr(language, "Verifying and activating…").to_owned()
                        } else {
                            format!(
                                "{} / {}{}",
                                components::format_file_size(*downloaded_bytes),
                                components::format_file_size(*total_bytes),
                                relative_path
                                    .as_deref()
                                    .map(|path| format!(" · {path}"))
                                    .unwrap_or_default(),
                            )
                        }),
                    );
                }
            }
        },
    );
    (clicked, delete_clicked)
}

fn render_runtime_installation_section(
    app: &mut crate::XRTranslateApp,
    ui: &mut egui::Ui,
    language: i18n::UiLanguage,
    project_root: &std::path::Path,
) {
    let state = app.runtime_installer.state().clone();
    let download_size = app.runtime_installer.download_size_bytes();
    let backend_name = app
        .runtime_installer
        .backend_label()
        .unwrap_or("CPU")
        .to_owned();
    let cuda_version = app.runtime_installer.cuda_version_label();
    let backend = cuda_version.map_or(backend_name.clone(), |version| {
        format!("{backend_name} {version}")
    });

    let default_runtime = app
        .backend_manager
        .runtime_layout()
        .runtime_root()
        .to_path_buf();
    let is_custom = !app.backend_manager.runtime_directory.trim().is_empty();
    let mut runtime_dir = app.backend_manager.runtime_directory.clone();
    let managed_runtime_present = app
        .runtime_installer
        .managed_resources_are_present(project_root);

    // Option A: Automatic Setup
    Frame::new()
        .fill(theme::surface_control())
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(16))
        .stroke(Stroke::new(
            1.5,
            if !is_custom {
                Color32::from_rgb(16, 185, 129)
            } else {
                Color32::from_rgb(110, 231, 183)
            },
        ))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new(i18n::tr(
                    language,
                    "Option A: Automatic Setup (Recommended)",
                ))
                .size(14.5)
                .color(Color32::from_rgb(4, 120, 87))
                .strong(),
            );
            ui.add_space(8.0);
            let ready = app.runtime_installer.plan_is_ready() && !app.runtime_installer.is_busy();
            let mut use_mirror = app.runtime_installer.use_mirror();
            let previous_source = use_mirror;
            let mut delete_runtime = false;
            ui.horizontal(|ui| {
                if ready {
                    Frame::new()
                        .fill(Color32::from_rgba_unmultiplied(16, 185, 129, 20))
                        .stroke(Stroke::new(1.0, Color32::from_rgba_unmultiplied(16, 185, 129, 100)))
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(Margin::symmetric(14, 5))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(format!(
                                    "{} · {}",
                                    i18n::tr(language, "Installed"),
                                    backend
                                ))
                                .color(Color32::from_rgb(5, 150, 105))
                                .size(12.5)
                                .strong(),
                            );
                        });
                } else {
                    let button_text = download_size.map_or_else(
                        || i18n::tr(language, "Preparing download…").to_owned(),
                        |bytes| {
                            format!(
                                "{} {} · {}",
                                i18n::tr(language, "Download"),
                                backend,
                                components::format_file_size(bytes)
                            )
                        },
                    );
                    let install_clicked = components::primary_button_enabled(
                        ui,
                        &button_text,
                        download_size.is_some()
                            && !app.runtime_installer.is_busy()
                            && !app.model_task_manager.is_busy(),
                    )
                    .clicked();
                    if install_clicked {
                        app.backend_manager.runtime_directory.clear();
                        if let Err(error) = app.backend_manager.save_runtime_directory() {
                            app.last_error = Some(error);
                        }
                        if let Err(error) = app
                            .runtime_installer
                            .install_recommended(project_root.to_path_buf())
                        {
                            app.last_error = Some(error);
                        }
                    }
                }
                if managed_runtime_present
                    && components::resource_delete_button(
                        ui,
                        "managed_runtime",
                        language,
                    )
                    .clicked()
                {
                    delete_runtime = true;
                }
                components::download_mirror_toggle(ui, language, &mut use_mirror);
            });
            if use_mirror != previous_source
                && let Err(error) = app
                    .runtime_installer
                    .switch_download_source(project_root.to_path_buf(), use_mirror)
            {
                app.last_error = Some(error);
            }
            if delete_runtime {
                app.request_runtime_resource_deletion();
            }

            components::render_runtime_fallback_notice(ui, language, &app.runtime_installer);

            components::render_runtime_task_state(
                ui,
                language,
                &state,
                "Extracting native runtime...",
                "The native runtime is installed and ready.",
            );
        });

    ui.add_space(12.0);

    // Option B: Custom Runtime Directory
    Frame::new()
        .fill(theme::surface_control())
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(16))
        .stroke(Stroke::new(
            1.5,
            if is_custom {
                theme::primary()
            } else {
                theme::border()
            },
        ))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new(i18n::tr(
                    language,
                    "Option B: Choose Existing Runtime Directory",
                ))
                .size(14.5)
                .color(theme::text_strong())
                .strong(),
            );
            ui.add_space(8.0);

            ui.horizontal(|ui| {
                ui.label(i18n::tr(language, "Runtime Directory:"));
                let response = components::singleline_input(
                    ui,
                    &mut runtime_dir,
                    i18n::tr(language, "Path to runtime folder"),
                    (ui.available_width() - 80.0).max(200.0),
                    false,
                );
                if response.changed() || response.lost_focus() {
                    app.backend_manager.runtime_directory = runtime_dir.clone();
                    if let Err(error) = app.backend_manager.save_runtime_directory() {
                        app.last_error = Some(error);
                    }
                    let requirements = app.service_config.runtime_requirements();
                    let _ = app
                        .runtime_installer
                        .prepare_for(project_root.to_path_buf(), requirements);
                }
                if components::animated_button(ui, i18n::tr(language, "Browse...")).clicked()
                    && let Some(path) = rfd::FileDialog::new().pick_folder()
                {
                    app.backend_manager.runtime_directory = path.to_string_lossy().to_string();
                    if let Err(error) = app.backend_manager.save_runtime_directory() {
                        app.last_error = Some(error);
                    }
                    let requirements = app.service_config.runtime_requirements();
                    let _ = app
                        .runtime_installer
                        .prepare_for(project_root.to_path_buf(), requirements);
                }
            });

            if !is_custom {
                ui.add_space(4.0);
                ui.label(
                    RichText::new(format!(
                        "{} {}",
                        i18n::tr(language, "Default:"),
                        default_runtime.display()
                    ))
                    .size(12.0)
                    .color(theme::text_weak()),
                );
            }
        });
}

fn render_model_task_state(
    ui: &mut egui::Ui,
    language: i18n::UiLanguage,
    state: &NativeModelTaskState,
    batch: Option<&crate::model_install::NativeModelBatchSnapshot>,
    items: &[DownloadItem],
) {
    match state {
        NativeModelTaskState::Idle => {}
        NativeModelTaskState::Discovering => {
            ui.add_space(6.0);
            ui.label(
                RichText::new(i18n::tr(language, "Scanning local models..."))
                    .size(12.0)
                    .color(theme::text_weak()),
            );
        }
        NativeModelTaskState::Detected { .. } => {
            ui.add_space(6.0);
            ui.label(
                RichText::new(i18n::tr(language, "Model packages detected."))
                    .size(12.0)
                    .color(Color32::from_rgb(5, 150, 105)),
            );
        }
        NativeModelTaskState::Installing { .. } => {
            ui.add_space(6.0);
            if let Some(batch) = batch
                && batch.total_bytes > 0
            {
                let current_label = batch
                    .current_asset_id
                    .and_then(|id| items.iter().find(|item| item.id == id))
                    .map_or("model package", |item| item.detail.as_str());
                let current_number = (batch.completed_packages + 1).min(batch.total_packages);
                ui.add(
                    egui::ProgressBar::new(
                        (batch.downloaded_bytes as f64 / batch.total_bytes as f64).clamp(0.0, 1.0)
                            as f32,
                    )
                    .text(format!(
                        "{} {}/{} · {} · {} / {}",
                        i18n::tr(language, "Model"),
                        current_number,
                        batch.total_packages,
                        current_label,
                        components::format_file_size(batch.downloaded_bytes),
                        components::format_file_size(batch.total_bytes),
                    )),
                );
            }
        }
        NativeModelTaskState::Installed { .. } => {
            ui.add_space(6.0);
            ui.label(
                RichText::new(i18n::tr(language, "A model package is ready."))
                    .size(12.0)
                    .color(Color32::from_rgb(5, 150, 105)),
            );
        }
        NativeModelTaskState::Failed(error) => {
            ui.add_space(6.0);
            ui.label(
                RichText::new(error)
                    .size(12.0)
                    .color(Color32::from_rgb(220, 38, 38)),
            );
        }
    }
}
