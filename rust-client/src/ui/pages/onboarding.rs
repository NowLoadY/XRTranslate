//! Fullscreen onboarding wizard and initial setup steps.
//!
//! Provides the step-by-step setup flow for model provider configuration,
//! optional TTS and OCR, and centralized resource download / runtime installation.

use eframe::egui::{self, Align, Color32, CornerRadius, Frame, Layout, Margin, RichText, Stroke};
use xrtranslate_assets::{ModelAssetId, ModelCapability, ModelLevel};

use crate::{
    i18n,
    model_install::{
        NativeModelPackage, NativeModelTaskState, catalog_model_packages,
        configured_model_packages, default_model_for_vram, model_asset_is_present,
        model_packages_for_provider, set_model_asset,
    },
    ui::{animation::AnimationSystem, components, model_comparison, theme},
};

const STEPS: [&str; 4] = [
    "Welcome",
    "Configure models",
    "Optional features",
    "Download",
];
const QQ_GROUP_NUMBER: &str = "1009732148";
const WELCOME_SHOWCASE_DELAY: f32 = 10.0;
const WELCOME_SHOWCASE_FADE: f32 = 0.55;

pub fn render_onboarding_fullscreen(
    app: &mut crate::XRTranslateApp,
    ui: &mut egui::Ui,
) -> crate::ui::companion::OnboardingLayout {
    let total_pages = STEPS.len();
    let narrow = ui.available_width() < 680.0;
    let compact_height = ui.available_height() < 480.0;
    let compact = narrow || compact_height;
    let margin = if compact { 16 } else { 36 };
    let footer_margin_y = if compact_height { 8 } else { 10 };
    let footer_gap = if compact_height { 8.0 } else { 12.0 };
    let page_rect = ui.available_rect_before_wrap();
    let content_margin_y = if compact_height { 8 } else { 20 };
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
    let custom_background = app.background_image.is_loaded();
    let welcome = app.onboarding_page == 0;
    let mut companion = crate::ui::companion::OnboardingLayout {
        requirement,
        ..Default::default()
    };

    let footer = egui::Area::new(egui::Id::new(("onboarding_bottom_nav", welcome)))
        .order(egui::Order::Middle)
        .constrain_to(page_rect)
        .anchor(
            if welcome {
                egui::Align2::RIGHT_BOTTOM
            } else {
                egui::Align2::CENTER_BOTTOM
            },
            [if welcome { -margin as f32 } else { 0.0 }, -footer_gap],
        )
        .fade_in(false)
        .show(ui.ctx(), |ui| {
            if welcome {
                let next = components::primary_button_enabled(
                    ui,
                    i18n::tr(app.ui_language, "Continue"),
                    requirement.is_none(),
                );
                companion.next = next.rect;
                if next.clicked() {
                    app.onboarding_page += 1;
                }
                return;
            }
            ui.set_width((page_rect.width() - 2.0 * margin as f32).min(720.0));
            Frame::new()
                .fill(if custom_background {
                    theme::surface_panel()
                } else {
                    theme::sidebar(viewport_focused)
                })
                .stroke(Stroke::new(1.0, theme::border()))
                .corner_radius(theme::container_radius(18))
                .shadow(egui::Shadow {
                    offset: [0, 3],
                    blur: 12,
                    spread: 0,
                    color: Color32::from_black_alpha(18),
                })
                .inner_margin(Margin::symmetric(16, footer_margin_y))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let final_page = app.onboarding_page + 1 == total_pages;
                    let tts_configured = app.service_config.tts_is_configured();
                    let agreement_satisfied = !tts_configured || app.usage_guidelines_accepted;
                    if final_page {
                        ui.horizontal_wrapped(|ui| {
                            if tts_configured
                                && components::checkbox(
                                    ui,
                                    &mut app.usage_guidelines_accepted,
                                    i18n::tr(app.ui_language, "I have read and agree"),
                                )
                                .changed()
                            {
                                app.save_settings();
                            }
                            if ui
                                .link(
                                    RichText::new(i18n::tr(app.ui_language, "Usage Guidelines"))
                                        .size(12.5),
                                )
                                .clicked()
                            {
                                app.modal_dialog = crate::ui::modal::ModalDialog::usage_guidelines(
                                    app.ui_language,
                                );
                            }
                        });
                    }
                    let hint = requirement.or_else(|| {
                        (final_page && !agreement_satisfied)
                            .then_some("Please agree to the Usage Guidelines to continue.")
                    });
                    companion.requirement = hint;
                    ui.horizontal(|ui| {
                        if app.onboarding_page > 0
                            && components::animated_button(ui, i18n::tr(app.ui_language, "Back"))
                                .clicked()
                        {
                            app.onboarding_page -= 1;
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let next = components::primary_button_enabled(
                                ui,
                                i18n::tr(
                                    app.ui_language,
                                    if final_page {
                                        "Open Translation"
                                    } else {
                                        "Continue"
                                    },
                                ),
                                requirement.is_none() && (!final_page || agreement_satisfied),
                            );
                            companion.next = next.rect;
                            let next = if let Some(hint) = hint {
                                next.on_disabled_hover_text(i18n::tr(app.ui_language, hint))
                            } else {
                                next
                            };
                            if next.clicked() {
                                if final_page {
                                    app.finish_onboarding();
                                } else {
                                    app.onboarding_page += 1;
                                }
                            }
                        });
                    });
                });
        });

    egui::CentralPanel::default()
        .frame(
            Frame::new()
                .fill(if custom_background {
                    Color32::TRANSPARENT
                } else {
                    theme::content_backdrop(viewport_focused)
                })
                .inner_margin(Margin::symmetric(margin, content_margin_y))
                .stroke(Stroke::NONE),
        )
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    render_onboarding_brand(ui, app.ui_language);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        render_onboarding_preferences(app, ui);
                    });
                });
                ui.add_space(if compact_height { 4.0 } else { 8.0 });
                let header_top = ui.cursor().top();
                let steps =
                    render_onboarding_steps(ui, app.ui_language, &STEPS, app.onboarding_page);
                let inline_guide = ui.max_rect().right() - steps.right() >= 280.0;
                ui.add_space(if inline_guide { 24.0 } else { 64.0 });
                companion.header = egui::Rect::from_min_max(
                    if inline_guide {
                        egui::pos2(steps.right() + 18.0, header_top - 8.0)
                    } else {
                        egui::pos2(ui.max_rect().left(), steps.bottom() + 8.0)
                    },
                    egui::pos2(ui.max_rect().right(), ui.cursor().top() - 4.0),
                );

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .id_salt(("onboarding_content_scroll", app.onboarding_page))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        let cur = app.onboarding_page;
                        crate::ui::animation::AnimationSystem::render_page_flip_transition(
                            ui,
                            cur,
                            |ui| match cur {
                                0 => {
                                    companion.features =
                                        Some(render_onboarding_welcome(app.ui_language, ui));
                                    companion.welcome_bottom = Some(ui.min_rect().bottom());
                                }
                                1 => render_onboarding_models(app, ui),
                                2 => render_onboarding_optional(app, ui),
                                _ => render_onboarding_download(app, ui),
                            },
                        );
                        ui.add_space(footer.response.rect.height() + footer_gap);
                    });
            });
        });

    render_qq_group_dialog(ui.ctx(), app.ui_language);
    companion.footer = footer.response.rect;
    companion
}

fn render_onboarding_social_icons(ui: &mut egui::Ui, language: i18n::UiLanguage) {
    for (icon, tooltip, url) in [
        (
            egui::include_image!("../../../resources/icons/github.svg"),
            "GitHub: NowLoadY/XRTranslate",
            "https://github.com/NowLoadY/XRTranslate",
        ),
        (
            egui::include_image!("../../../resources/icons/globe.svg"),
            "chatgpt.site: https://xrtranslate.nowloady.chatgpt.site",
            "https://xrtranslate.nowloady.chatgpt.site",
        ),
    ] {
        if onboarding_icon_button(ui, egui::Image::new(icon), tooltip) {
            ui.ctx().open_url(egui::OpenUrl::new_tab(url));
        }
        ui.add_space(4.0);
    }
    if onboarding_icon_button(
        ui,
        egui::Image::new(egui::include_image!("../../../resources/icons/qq.svg")),
        &i18n::tr(language, "QQ group"),
    ) {
        ui.ctx()
            .data_mut(|data| data.insert_temp(egui::Id::new("onboarding_qq_group"), true));
    }
}

fn render_onboarding_brand(ui: &mut egui::Ui, language: i18n::UiLanguage) {
    ui.label(
        RichText::new("XRTranslate")
            .size(15.0)
            .color(theme::primary())
            .strong(),
    );
    ui.add_space(6.0);
    render_onboarding_social_icons(ui, language);
}

fn render_onboarding_preferences(app: &mut crate::XRTranslateApp, ui: &mut egui::Ui) {
    let mut language = app.ui_language;
    if components::language_selector(ui, "onboarding_ui_language", &mut language) {
        app.set_ui_language(language);
    }
    ui.add_space(8.0);
    ui.menu_button("…", |ui| {
        render_onboarding_download_controls(app, ui);
    });
}

fn render_onboarding_download_controls(app: &mut crate::XRTranslateApp, ui: &mut egui::Ui) {
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
    ui.add_space(8.0);
    crate::ui::pages::settings::render_update_action_button(app, ui);
}

fn onboarding_icon_button(ui: &mut egui::Ui, icon: egui::Image<'_>, tooltip: &str) -> bool {
    let icon = icon
        .fit_to_exact_size(egui::vec2(20.0, 20.0))
        .tint(theme::text_weak());
    ui.add(
        egui::Button::image(icon)
            .frame(false)
            .min_size(egui::vec2(28.0, 28.0)),
    )
    .on_hover_text(tooltip)
    .on_hover_cursor(egui::CursorIcon::PointingHand)
    .clicked()
}

fn render_qq_group_dialog(ctx: &egui::Context, language: i18n::UiLanguage) {
    let id = egui::Id::new("onboarding_qq_group");
    if !ctx.data(|data| data.get_temp::<bool>(id).unwrap_or(false)) {
        return;
    }

    let mut close = false;
    let dismissed = crate::ui::modal::dialog(
        ctx,
        id,
        language,
        i18n::tr(language, "QQ group"),
        |ui| {
            ui.label(
                RichText::new(i18n::tr(language, "Group number"))
                    .small()
                    .color(theme::text_weak()),
            );
            ui.label(RichText::new(QQ_GROUP_NUMBER).size(20.0).strong());
        },
        |ui| {
            ui.horizontal_wrapped(|ui| {
                if components::copy_button(ui, i18n::tr(language, "Copy number")).clicked() {
                    ctx.copy_text(QQ_GROUP_NUMBER.to_owned());
                }
                close = components::secondary_button(ui, i18n::tr(language, "Close")).clicked();
            });
        },
    );
    if close || dismissed {
        ctx.data_mut(|data| data.insert_temp(id, false));
    }
}

fn render_onboarding_steps(
    ui: &mut egui::Ui,
    language: i18n::UiLanguage,
    steps: &[&'static str],
    current: usize,
) -> egui::Rect {
    let compact = ui.available_width() < 680.0 || ui.available_height() < 480.0;
    let mut bounds = egui::Rect::NOTHING;
    let padding = ui.spacing().button_padding;
    ui.spacing_mut().button_padding = if compact {
        egui::vec2(10.0, 6.0)
    } else {
        egui::vec2(14.0, 8.0)
    };
    ui.horizontal_wrapped(|ui| {
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
            let label = i18n::tr(language, *title);
            let text = if compact || !active {
                (i + 1).to_string()
            } else {
                format!("{} {}", i + 1, label)
            };
            let step = ui.add(
                egui::Button::new(RichText::new(text).size(13.0).color(text_color).strong())
                    .min_size(if compact {
                        egui::vec2(36.0, 32.0)
                    } else {
                        egui::Vec2::ZERO
                    })
                    .fill(fill)
                    .stroke(Stroke::NONE)
                    .corner_radius(12)
                    .wrap_mode(egui::TextWrapMode::Extend)
                    .sense(egui::Sense::hover()),
            );
            let step = step.on_hover_text(label);
            bounds = bounds.union(step.rect);

            if i + 1 < steps.len() {
                ui.add_space(if compact { 10.0 } else { 4.0 });
            }
        }
    });
    ui.spacing_mut().button_padding = padding;
    bounds
}

fn onboarding_feature(
    ui: &mut egui::Ui,
    title: &'static str,
    description: &'static str,
    color: Color32,
    icon: egui::ImageSource<'static>,
    language: crate::i18n::UiLanguage,
) -> egui::Rect {
    let response = Frame::new()
        .inner_margin(Margin::symmetric(2, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(40.0, 40.0), egui::Sense::hover());
                ui.painter()
                    .circle_filled(rect.center(), 20.0, color.gamma_multiply(0.35));
                egui::Image::new(icon)
                    .tint(color)
                    .paint_at(ui, rect.shrink(9.0));
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(i18n::tr(language, title))
                            .size(14.0)
                            .strong()
                            .color(theme::text_strong()),
                    );
                    ui.add(
                        egui::Label::new(
                            RichText::new(i18n::tr(language, description))
                                .size(12.0)
                                .color(theme::text_normal()),
                        )
                        .wrap(),
                    );
                });
            });
        });
    ui.ctx()
        .layer_transform_to_global(ui.layer_id())
        .unwrap_or_default()
        * response.response.rect
}

fn render_onboarding_welcome(
    language: crate::i18n::UiLanguage,
    ui: &mut egui::Ui,
) -> [egui::Rect; 3] {
    let id = ui.make_persistent_id("welcome_entrance");
    let frame = ui.ctx().cumulative_frame_nr();
    let now = ui.input(|input| input.time);
    let started = ui.data_mut(|data| {
        let state = data.get_temp_mut_or_insert_with(id, || (frame, now));
        if frame > state.0 + 1 {
            state.1 = now;
        }
        state.0 = frame;
        state.1
    });
    let elapsed = (now - started) as f32;
    AnimationSystem::render_staggered(ui, id.with("title"), elapsed, 0, |ui| {
        ui.label(
            RichText::new(i18n::tr(language, "Welcome to XRTranslate"))
                .size(26.0)
                .color(theme::text_strong())
                .strong(),
        );
    });
    ui.add_space(16.0);
    let cards = [
        (
            "Audio Input",
            "Microphone or desktop audio.",
            Color32::from_rgb(59, 130, 246),
            egui::include_image!("../../../resources/icons/microphone.svg"),
        ),
        (
            "Recognition & Translation",
            "Speak across languages.",
            Color32::from_rgb(16, 185, 129),
            egui::include_image!("../../../resources/icons/translation.svg"),
        ),
        (
            "Plugins & Integrations",
            "Subtitles, voice, VRChat & ChilloutVR.",
            Color32::from_rgb(245, 158, 11),
            egui::include_image!("../../../resources/plugins/game_plugins/icon.svg"),
        ),
    ];
    let mut bounds = [egui::Rect::NOTHING; 3];
    crate::ui::layout::responsive_columns(ui, cards.len(), 220.0, |ui, index| {
        let (title, description, color, icon) = &cards[index];
        bounds[index] =
            AnimationSystem::render_staggered(ui, id.with(index), elapsed, index + 1, |ui| {
                onboarding_feature(ui, title, description, *color, icon.clone(), language)
            });
    });

    ui.add_space(20.0);
    let showcase_elapsed = elapsed - WELCOME_SHOWCASE_DELAY;
    ui.scope(|ui| {
        if showcase_elapsed <= 0.0 {
            // Keep the layout and image loading ready while hiding all controls.
            ui.set_invisible();
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f32(
                    (-showcase_elapsed).max(0.0),
                ));
        } else {
            let progress = (showcase_elapsed / WELCOME_SHOWCASE_FADE).clamp(0.0, 1.0);
            ui.multiply_opacity(AnimationSystem::ease_out_cubic(progress));
            if progress < 1.0 {
                ui.ctx().request_repaint();
            }
        }
        components::showcase(ui, language, &crate::ui::previews::welcome());
    });
    bounds
}

// ---------------------------------------------------------------------------
// Step 2: Configure Models (Pure configuration without download buttons)
// ---------------------------------------------------------------------------

fn render_onboarding_models(app: &mut crate::XRTranslateApp, ui: &mut egui::Ui) {
    let language = app.ui_language;
    let project_root = app.project_root();
    let local_availability = app.runtime_installer.local_model_availability();
    if !app.model_defaults_initialized
        && let Some(memory_bytes) = local_availability.available_memory_bytes()
    {
        // Existing explicit choices take precedence over the first-run hardware suggestion.
        let mut changed = false;
        let mut failed = false;
        if let Ok(current) = configured_model_packages(&project_root)
            && memory_bytes < xrtranslate_assets::MANAGED_LOCAL_MODEL_MINIMUM_VRAM_BYTES
        {
            for package in current.iter().filter(|package| {
                package.level == ModelLevel::Normal
                    && matches!(
                        package.capability,
                        ModelCapability::Asr | ModelCapability::Translation
                    )
            }) {
                if let Some(asset_id) =
                    default_model_for_vram(package.provider, package.capability, memory_bytes)
                {
                    match set_model_asset(&project_root, package.capability, asset_id) {
                        Ok(()) => changed = true,
                        Err(error) => {
                            app.last_error = Some(error);
                            failed = true;
                        }
                    }
                }
            }
        }
        if changed {
            if let Err(error) = app.service_config.reload() {
                app.last_error = Some(error);
                failed = true;
            }
            app.model_task_manager.invalidate_discovery();
            let requirements = app.service_config.runtime_requirements();
            if let Err(error) = app
                .runtime_installer
                .prepare_for(project_root.clone(), requirements)
            {
                app.last_error = Some(error);
                failed = true;
            }
        }
        if !failed {
            app.model_defaults_initialized = true;
            app.save_settings();
        }
    }
    let packages = match configured_model_packages(&project_root) {
        Ok(packages) => packages,
        Err(error) => {
            app.last_error = Some(error);
            Vec::new()
        }
    };
    if let Some(new_gpu) = model_comparison::vram_budget(
        ui,
        language,
        &local_availability,
        &app.service_config.selected_model_asset_ids(),
    ) {
        app.set_preferred_gpu(&new_gpu);
    }
    ui.add_space(12.0);
    let mut model_change = None;
    let mut provider_change = None;
    let mut remote_fields = None;
    let mut delete_model = None;
    let capabilities = [
        ("asr", ModelCapability::Asr, "Speech Recognition Model"),
        (
            "translation",
            ModelCapability::Translation,
            "Translation Model",
        ),
    ];
    crate::ui::layout::responsive_columns(ui, capabilities.len(), 300.0, |ui, index| {
        let (category, capability, title) = &capabilities[index];
        let package = packages
            .iter()
            .find(|package| package.capability == *capability);
        let provider = app.service_config.onboarding_provider_state(category);
        let levels = package.map_or_else(Vec::new, |package| {
            model_packages_for_provider(package.provider, package.capability)
        });
        let result = onboarding_model_config_card(
            ui,
            language,
            category,
            *capability,
            title,
            &project_root,
            provider,
            package.map(|package| package.id),
            &levels,
            !app.model_task_manager.is_busy(),
            &local_availability,
            if index % 2 == 0 {
                Color32::from_rgb(59, 130, 246)
            } else {
                Color32::from_rgb(16, 185, 129)
            },
        );
        ui.add_space(12.0);
        model_comparison::model_chart(ui, language, *capability, package.map(|package| package.id));
        if let Some(asset_id) = result.selected_asset {
            model_change = Some((*capability, asset_id));
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
    });
    if let Some(asset_id) = delete_model {
        app.request_model_resource_deletion(asset_id);
    }
    if let Some((category, provider)) = provider_change {
        app.service_config
            .select_onboarding_provider(category, &provider);
        let result = app.service_config.save_onboarding_configuration();
        let saved = matches!(
            &result,
            Ok(crate::service_config::OnboardingSaveOutcome::Saved { .. })
        );
        handle_onboarding_save(app, result);
        if saved {
            app.apply_language_route();
        }
        let capability = match category {
            "asr" => Some(ModelCapability::Asr),
            "translation" => Some(ModelCapability::Translation),
            _ => None,
        };
        if saved
            && capability.is_some_and(|capability| {
                !model_packages_for_provider(&provider, capability).is_empty()
            })
        {
            app.model_defaults_initialized = false;
            app.save_settings();
        }
    }
    if let Some((category, fields)) = remote_fields {
        app.service_config
            .set_onboarding_remote_fields(category, fields.model, fields.api_key);
        let result = app.service_config.save_onboarding_configuration();
        handle_onboarding_save(app, result);
    }
    if let Some(message) = app.service_config.onboarding_message() {
        ui.add_space(10.0);
        components::error_notice(ui, language, message);
    }
    if let Some((capability, asset_id)) = model_change {
        match set_model_asset(&project_root, capability, asset_id) {
            Ok(()) => {
                if let Err(error) = app.service_config.reload() {
                    app.last_error = Some(error);
                }
                app.apply_language_route();
                app.model_task_manager.invalidate_discovery();
                app.backend_manager.invalidate_runtime();
                let requirements = app.service_config.runtime_requirements();
                if let Err(error) = app
                    .runtime_installer
                    .prepare_for(project_root.clone(), requirements)
                {
                    app.last_error = Some(error);
                }
                app.model_defaults_initialized = true;
                app.save_settings();
            }
            Err(error) => app.last_error = Some(error),
        }
    }
}

#[derive(Default)]
struct ModelConfigCardResult {
    selected_asset: Option<ModelAssetId>,
    selected_provider: Option<String>,
    remote_fields: Option<RemoteProviderFields>,
    delete_asset: Option<xrtranslate_assets::ModelAssetId>,
}

struct RemoteProviderFields {
    model: String,
    api_key: String,
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
            ..
        } => format!(
            "{}\n\n{gpu}: {:.1} GiB / {:.0} GiB",
            i18n::tr(
                language,
                "Your GPU has less than 1 GiB of VRAM. GPU models require at least 1 GiB, so this option is disabled."
            ),
            *memory_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
            *required_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
        ),
        crate::runtime_install::LocalModelAvailability::Unavailable(reason) => format!(
            "{}\n\n{}",
            i18n::tr(
                language,
                "Local models are unavailable on this device. You can continue with an online API, or update the graphics driver or GPU and try again."
            ),
            reason
        ),
        crate::runtime_install::LocalModelAvailability::Detecting
        | crate::runtime_install::LocalModelAvailability::Available { .. } => return,
        #[cfg(target_os = "android")]
        crate::runtime_install::LocalModelAvailability::Cpu { .. } => return,
    };
    let icon = egui::Image::new(egui::include_image!(
        "../../../resources/icons/alert-triangle.svg"
    ))
    .fit_to_exact_size(egui::vec2(18.0, 18.0))
    .tint(Color32::from_rgb(217, 119, 6))
    .sense(egui::Sense::hover());
    ui.add(icon).on_hover_text(tooltip);
}

fn model_hardware_hint(
    language: i18n::UiLanguage,
    hardware: xrtranslate_assets::ModelHardwareRequirements,
    availability: &crate::runtime_install::LocalModelAvailability,
) -> String {
    let uses_ram = hardware.accelerator == xrtranslate_assets::ModelAccelerator::Cpu
        || (availability.is_cpu()
            && hardware.accelerator == xrtranslate_assets::ModelAccelerator::LlamaGpu);
    let (device, memory) = if uses_ram {
        ("CPU", "RAM")
    } else {
        (hardware.accelerator.label(), "VRAM")
    };
    format!(
        "{} {} · {:.0} GiB {memory}",
        i18n::tr(language, "Requires at least"),
        device,
        hardware.minimum_memory_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
    )
}

fn local_model_choices(
    ui: &mut egui::Ui,
    language: i18n::UiLanguage,
    project_root: &std::path::Path,
    selected: &mut ModelAssetId,
    packages: &[NativeModelPackage],
    delete_enabled: bool,
    availability: &crate::runtime_install::LocalModelAvailability,
) -> Option<ModelAssetId> {
    let groups: Vec<_> = [
        ModelLevel::Small,
        ModelLevel::Normal,
        ModelLevel::Big,
        ModelLevel::Ultra,
    ]
    .into_iter()
    .filter(|level| packages.iter().any(|package| package.level == *level))
    .collect();
    if groups.is_empty() {
        return None;
    }
    let width = (groups.len() as f32 * 240.0).min(ui.ctx().content_rect().width() - 40.0);
    let columns = ((width / 210.0) as usize).clamp(1, groups.len());
    ui.set_width(width);
    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
    ui.spacing_mut().item_spacing.x = 24.0;
    let mut delete = None;
    for row in groups.chunks(columns) {
        let rects = ui.columns(columns, |columns| {
            columns
                .iter_mut()
                .zip(row)
                .map(|(ui, level)| {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(i18n::tr(language, level.as_str()))
                            .strong()
                            .color(theme::primary()),
                    );
                    ui.add_space(8.0);
                    for package in packages.iter().filter(|package| package.level == *level) {
                        let manifest = xrtranslate_assets::manifest_for(package.id);
                        let present =
                            model_asset_is_present(project_root, package.id).unwrap_or(false);
                        let available = availability.supports(package.hardware);
                        let choice = ui.add_enabled(
                            available,
                            egui::Button::selectable(
                                *selected == package.id,
                                RichText::new(package.label).size(12.0),
                            )
                            .wrap()
                            .min_size(egui::vec2(ui.available_width(), 32.0)),
                        );
                        if choice.clicked() {
                            *selected = package.id;
                            ui.close();
                        }
                        choice.on_disabled_hover_text(model_hardware_hint(
                            language,
                            package.hardware,
                            availability,
                        ));
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            ui.label(
                                RichText::new(format!(
                                    "≈ {:.1} GiB",
                                    manifest.estimated_vram_bytes as f64
                                        / (1024.0 * 1024.0 * 1024.0)
                                ))
                                .size(10.5)
                                .weak(),
                            );
                            if present {
                                ui.label(
                                    RichText::new(i18n::tr(language, "Installed"))
                                        .size(10.5)
                                        .color(Color32::from_rgb(16, 150, 110)),
                                );
                            }
                            if let Some(license) = manifest.required_files.iter().find(|file| {
                                file.role == xrtranslate_assets::ModelFileRole::License
                            }) {
                                ui.hyperlink_to(
                                    RichText::new(i18n::tr(language, "Model license")).size(10.5),
                                    manifest.source.file_url(license.relative_path),
                                );
                            }
                            if present
                                && ui
                                    .add_enabled_ui(delete_enabled, |ui| {
                                        components::resource_delete_button(ui, package.id, language)
                                    })
                                    .inner
                                    .clicked()
                            {
                                delete = Some(package.id);
                                ui.close();
                            }
                        });
                        ui.add_space(12.0);
                    }
                    ui.min_rect()
                })
                .collect::<Vec<_>>()
        });
        let bottom = rects.iter().map(egui::Rect::bottom).fold(0.0, f32::max);
        for pair in rects.windows(2) {
            let x = (pair[0].right() + pair[1].left()) * 0.5;
            let height = bottom - pair[0].top();
            let dashes = ((height * 0.65).min(96.0) / 14.0) as usize;
            let top = pair[0].top() + (height - (dashes as f32 * 14.0 - 6.0)) * 0.5;
            for dash in 0..dashes {
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(
                        egui::pos2(x - 2.0, top + dash as f32 * 14.0),
                        egui::vec2(4.0, 8.0),
                    ),
                    2.0,
                    theme::border().gamma_multiply(0.7),
                );
            }
        }
    }
    delete
}

fn onboarding_model_config_card(
    ui: &mut egui::Ui,
    language: i18n::UiLanguage,
    category: &'static str,
    capability: ModelCapability,
    title: &'static str,
    project_root: &std::path::Path,
    mut provider: Option<crate::service_config::OnboardingProviderState>,
    selected_asset: Option<ModelAssetId>,
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
            let guide_url = provider.as_ref().and_then(|p| {
                if p.remote {
                    p.choices
                        .iter()
                        .find(|c| c.name == p.selected)
                        .and_then(|c| c.guide_url.as_deref())
                } else {
                    None
                }
            });

            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(i18n::tr(language, title))
                        .size(16.0)
                        .color(theme::text_strong())
                        .strong(),
                );
                if let Some(guide_url) = guide_url {
                    let guide_btn = ui.hyperlink_to(
                        RichText::new(format!("{} ↗", i18n::tr(language, "API Key Guide")))
                            .size(12.5)
                            .color(theme::primary()),
                        guide_url,
                    );
                    guide_btn.on_hover_text(format!(
                        "{}\n{}",
                        i18n::tr(language, "Open official documentation to get an API key"),
                        guide_url
                    ));
                }
            });
            ui.label(
                RichText::new(i18n::tr(
                    language,
                    if capability == ModelCapability::Asr {
                        "Speech → text"
                    } else {
                        "Text → translation"
                    },
                ))
                .size(12.0)
                .color(theme::text_weak()),
            );
            ui.add_space(10.0);
            let Some(provider) = provider.as_mut() else {
                ui.label(i18n::tr(language, "No providers configured"));
                return;
            };

            crate::ui::layout::responsive_columns(ui, 2, 200.0, |ui, index| {
                if index == 0 {
                    ui.label(i18n::tr(language, "Mode"));
                    let mut remote = provider.remote;
                    // Online providers have no active local package. Resolve
                    // the return option from configured choices and the catalogue.
                    let local_choice = provider.choices.iter().find(|choice| {
                        !choice.remote
                            && xrtranslate_assets::manifests_for_capability(capability).any(
                                |manifest| {
                                    manifest.provider == choice.name
                                        && local_availability.supports(manifest.hardware)
                                },
                            )
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.add_enabled_ui(local_choice.is_some(), |ui| {
                            ui.selectable_value(
                                &mut remote,
                                false,
                                i18n::tr(language, "Local model"),
                            );
                        });
                        ui.selectable_value(&mut remote, true, i18n::tr(language, "Online API"));
                    });
                    if local_choice.is_none() {
                        local_model_warning_icon(ui, language, local_availability);
                    }
                    if remote != provider.remote {
                        let choice = if remote {
                            provider.choices.iter().find(|choice| choice.remote)
                        } else {
                            local_choice
                        };
                        result.selected_provider = choice.map(|choice| choice.name.clone());
                    }
                } else if !provider.remote {
                    let Some(mut asset_id) = selected_asset else {
                        return;
                    };
                    ui.label(i18n::tr(language, "Model"));
                    let selected_package = levels.iter().find(|package| package.id == asset_id);
                    let selected_present = selected_package.is_some_and(|package| {
                        model_asset_is_present(project_root, package.id).unwrap_or(false)
                    });
                    let selected_name = selected_package.map_or_else(
                        || asset_id.as_str().to_owned(),
                        |package| {
                            format!(
                                "{} · {}",
                                i18n::tr(language, package.level.as_str()),
                                package.label
                            )
                        },
                    );
                    let selected_label = if selected_present {
                        format!("{} · {}", selected_name, i18n::tr(language, "Installed"))
                    } else {
                        selected_name
                    };
                    let local_available = levels
                        .iter()
                        .any(|package| local_availability.supports(package.hardware));
                    ui.add_enabled_ui(local_available, |ui| {
                        components::combobox_ui(
                            ui,
                            (category, "model_level"),
                            selected_label,
                            |ui| {
                                result.delete_asset = local_model_choices(
                                    ui,
                                    language,
                                    project_root,
                                    &mut asset_id,
                                    levels,
                                    delete_enabled,
                                    local_availability,
                                );
                            },
                        );
                    });
                    if Some(asset_id) != selected_asset {
                        result.selected_asset = Some(asset_id);
                    }
                } else {
                    ui.label(i18n::tr(language, "Provider:"));
                    let selected_label =
                        crate::service_config::provider_display_label(&provider.selected, language);
                    components::combobox_ui(
                        ui,
                        (category, "online_provider"),
                        selected_label,
                        |ui| {
                            for choice in &provider.choices {
                                if choice.remote {
                                    let label = crate::service_config::provider_display_label(
                                        &choice.name,
                                        language,
                                    );
                                    if ui
                                        .selectable_label(provider.selected == choice.name, label)
                                        .clicked()
                                    {
                                        result.selected_provider = Some(choice.name.clone());
                                    }
                                }
                            }
                        },
                    );
                }
            });

            if !provider.remote {
                let asset_id = result.selected_asset.or(selected_asset);
                if let Some(asset_id) = asset_id {
                    let manifest = xrtranslate_assets::manifest_for(asset_id);
                    ui.add_space(9.0);
                    components::language_flags(ui, language, manifest.languages);
                }
            }

            if provider.remote {
                result.remote_fields = remote_provider_fields(ui, category, provider, language);
            } else {
                match local_availability {
                    crate::runtime_install::LocalModelAvailability::InsufficientVram { .. } => {}
                    crate::runtime_install::LocalModelAvailability::Unavailable(_) => {}
                    crate::runtime_install::LocalModelAvailability::Detecting => {
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new(i18n::tr(language, "Detecting GPU…"))
                                .size(11.5)
                                .color(theme::text_weak()),
                        );
                    }
                    crate::runtime_install::LocalModelAvailability::Available { .. } => {}
                    #[cfg(target_os = "android")]
                    crate::runtime_install::LocalModelAvailability::Cpu { .. } => {}
                }
            }
        },
    );
    result
}

// ---------------------------------------------------------------------------
// Step 3: Optional capabilities (downloads are handled in the final step)
// ---------------------------------------------------------------------------

fn render_onboarding_optional(app: &mut crate::XRTranslateApp, ui: &mut egui::Ui) {
    ui.heading(i18n::tr(app.ui_language, "Optional features"));
    ui.add_space(16.0);
    #[cfg(target_os = "android")]
    render_onboarding_tts(app, ui);
    #[cfg(not(target_os = "android"))]
    crate::ui::layout::responsive_columns(ui, 2, 340.0, |ui, index| match index {
        0 => render_onboarding_tts(app, ui),
        _ => render_onboarding_ocr(app, ui),
    });
}

fn onboarding_surface(ui: &egui::Ui, color: Color32) -> Frame {
    Frame::new()
        .fill(theme::panel_fill(ui.ctx(), theme::surface_tinted(color)))
        .corner_radius(CornerRadius::same(20))
        .inner_margin(Margin::same(18))
}

fn remote_provider_fields(
    ui: &mut egui::Ui,
    category: &str,
    provider: &mut crate::service_config::OnboardingProviderState,
    language: i18n::UiLanguage,
) -> Option<RemoteProviderFields> {
    let previous_model = provider.model.clone();
    ui.add_space(12.0);
    ui.label(i18n::tr(language, "Model"));
    components::combobox_ui(ui, (category, "online_model"), &previous_model, |ui| {
        for model in &provider.models {
            ui.selectable_value(&mut provider.model, model.clone(), model);
        }
    });
    ui.add_space(8.0);
    ui.label(i18n::tr(language, "API key"));
    let key = components::singleline_input(
        ui,
        &mut provider.api_key,
        i18n::tr(language, "API key"),
        (ui.available_width() - 16.0).max(0.0),
        true,
    );
    (key.changed() || provider.model != previous_model).then(|| RemoteProviderFields {
        model: provider.model.clone(),
        api_key: provider.api_key.clone(),
    })
}

fn render_onboarding_tts(app: &mut crate::XRTranslateApp, ui: &mut egui::Ui) {
    let language = app.ui_language;
    let provider = app.service_config.onboarding_provider_state("tts");
    let mut selected_provider = None;
    let project_root = app.project_root();
    let mut delete_tts = None;
    let mut model_change = None;
    let mut voice_change = None;
    let mut remote_fields = None;

    let color = Color32::from_rgb(244, 113, 136);
    onboarding_surface(ui, color).show(ui, |ui| {
        ui.set_width(ui.available_width());
        onboarding_feature(
            ui,
            "Voice Playback",
            "Read translations aloud.",
            color,
            egui::include_image!("../../../resources/icons/tts-center.svg"),
            language,
        );
        ui.add_space(10.0);
        let Some(mut provider) = provider else {
            ui.label(i18n::tr(language, "No providers configured"));
            return;
        };
        selected_provider = optional_provider_selector(
            ui,
            "tts",
            &provider,
            &project_root,
            language,
            &app.runtime_installer.local_model_availability(),
        );
        ui.add_space(10.0);
        if provider.remote {
            if let Some(url) = provider
                .choices
                .iter()
                .find(|choice| choice.name == provider.selected)
                .and_then(|choice| choice.guide_url.as_deref())
            {
                ui.hyperlink_to(format!("{} ↗", i18n::tr(language, "API Key Guide")), url);
            }
            remote_fields = remote_provider_fields(ui, "tts", &mut provider, language);
        } else if provider.selected != "none" {
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
                    RichText::new(i18n::tr(language, "Languages"))
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
                    let package_enabled = availability.supports(package.hardware);
                    let may_toggle = package_enabled
                        && (!checked || selected_assets.len() > 1)
                        && !app.model_task_manager.is_busy();
                    ui.horizontal_wrapped(|ui| {
                        let mut next = checked;
                        let language_pack = if package.languages.is_empty() {
                            package.label.to_owned()
                        } else {
                            format!("{} — {}", package.languages.join(", "), package.label)
                        };
                        if ui
                            .add_enabled(may_toggle, egui::Checkbox::new(&mut next, language_pack))
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
                            if components::resource_delete_button(ui, package.id, language)
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
                            ui.horizontal_wrapped(|ui| {
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
                    crate::runtime_install::LocalModelAvailability::InsufficientVram { .. } => {}
                    crate::runtime_install::LocalModelAvailability::Unavailable(_) => {}
                    crate::runtime_install::LocalModelAvailability::Detecting => {
                        ui.label(
                            RichText::new(i18n::tr(language, "Detecting GPU…"))
                                .size(11.5)
                                .color(theme::text_weak()),
                        );
                    }
                    crate::runtime_install::LocalModelAvailability::Available { .. } => {}
                    #[cfg(target_os = "android")]
                    crate::runtime_install::LocalModelAvailability::Cpu { .. } => {}
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
    });

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

    if let Some(fields) = remote_fields {
        app.service_config
            .set_onboarding_remote_fields("tts", fields.model, fields.api_key);
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

fn optional_provider_selector(
    ui: &mut egui::Ui,
    category: &str,
    provider: &crate::service_config::OnboardingProviderState,
    project_root: &std::path::Path,
    language: i18n::UiLanguage,
    availability: &crate::runtime_install::LocalModelAvailability,
) -> Option<String> {
    let mut selected_provider = None;
    ui.horizontal_wrapped(|ui| {
        ui.label(i18n::tr(language, "Provider:"));

        let selected_label = provider
            .choices
            .iter()
            .find(|choice| choice.name == provider.selected)
            .map(|choice| provider_choice_resource(choice, project_root, language).0)
            .unwrap_or_else(|| provider.selected.clone());
        components::combobox_ui(ui, (category, "provider"), selected_label, |ui| {
            for choice in &provider.choices {
                let (label, _asset_id, _present) =
                    provider_choice_resource(choice, project_root, language);
                let local_available = !choice.model_assets.is_empty()
                    && choice.model_assets.iter().all(|key| {
                        ModelAssetId::from_config_key(key).is_some_and(|id| {
                            availability.supports(xrtranslate_assets::manifest_for(id).hardware)
                        })
                    });
                let response = ui.add_enabled_ui(
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
                if let Some(model) = choice
                    .model_assets
                    .iter()
                    .filter_map(|key| ModelAssetId::from_config_key(key))
                    .map(xrtranslate_assets::manifest_for)
                    .find(|model| !availability.supports(model.hardware))
                {
                    response
                        .response
                        .on_disabled_hover_text(model_hardware_hint(
                            language,
                            model.hardware,
                            availability,
                        ));
                }
            }
        });
        if provider
            .choices
            .iter()
            .find(|choice| choice.name == provider.selected)
            .is_some_and(|choice| {
                choice
                    .model_assets
                    .iter()
                    .filter_map(|key| ModelAssetId::from_config_key(key))
                    .any(|id| !availability.supports(xrtranslate_assets::manifest_for(id).hardware))
            })
        {
            local_model_warning_icon(ui, language, availability);
        }
    });
    selected_provider
}

#[cfg(not(target_os = "android"))]
fn render_onboarding_ocr(app: &mut crate::XRTranslateApp, ui: &mut egui::Ui) {
    let language = app.ui_language;
    let project_root = app.project_root();
    let provider = app.service_config.onboarding_provider_state("ocr");
    let mut selected_provider = None;
    let mut delete_model = None;
    let color = Color32::from_rgb(155, 125, 239);
    onboarding_surface(ui, color).show(ui, |ui| {
        ui.set_width(ui.available_width());
        onboarding_feature(
            ui,
            "Screen text recognition",
            "Translate text on your screen.",
            color,
            egui::include_image!("../../../resources/icons/image.svg"),
            language,
        );
        ui.add_space(10.0);
        let Some(provider) = provider else { return };
        selected_provider = optional_provider_selector(
            ui,
            "ocr",
            &provider,
            &project_root,
            language,
            &app.runtime_installer.local_model_availability(),
        );
        ui.add_space(10.0);
        if let Some(choice) = provider
            .choices
            .iter()
            .find(|choice| choice.name == provider.selected)
        {
            let (_, asset, installed) = provider_choice_resource(choice, &project_root, language);
            if let Some(asset) = asset {
                let manifest = xrtranslate_assets::manifest_for(asset);
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(components::format_file_size(manifest.download_bytes()))
                            .size(12.0)
                            .color(theme::text_weak()),
                    );
                    if installed
                        && components::resource_delete_button(ui, asset, language).clicked()
                    {
                        delete_model = Some(asset);
                    }
                });
            }
        }
    });
    if let Some(asset) = delete_model {
        app.request_model_resource_deletion(asset);
    }
    if let Some(selected) = selected_provider {
        app.service_config
            .select_onboarding_provider("ocr", &selected);
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
            app.model_task_manager.invalidate_discovery();
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
    ui.heading(i18n::tr(language, "Download required resources"));
    ui.add_space(16.0);

    if app.model_task_manager.needs_discovery()
        && let Err(error) = app
            .model_task_manager
            .discover_existing(project_root.clone())
    {
        app.last_error = Some(error);
    }

    let requirements = app.service_config.runtime_requirements();
    let model_assets = app.service_config.selected_model_asset_ids();
    if !app.runtime_installer.is_busy()
        && !app
            .runtime_installer
            .plan_matches(requirements, &model_assets)
        && let Err(error) = app
            .runtime_installer
            .prepare_for(project_root.clone(), requirements)
    {
        app.last_error = Some(error);
    }

    let busy = app.model_task_manager.is_busy();
    let mut installs = Vec::new();
    let mut pauses = Vec::new();
    let mut delete_model = None;

    let configured = configured_model_packages(&project_root);
    let configuration_loaded = configured.is_ok();
    let mut packages = match configured {
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
            let installed = app.model_task_manager.is_model_ready(package.id)
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
                ModelCapability::Ocr => ("OCR Model", Color32::from_rgb(139, 92, 246)),
                ModelCapability::SourceSeparation => {
                    ("Vocal Separation Model", Color32::from_rgb(16, 185, 129))
                }
                ModelCapability::NoiseSuppression => {
                    ("Speech Denoising Model", Color32::from_rgb(59, 130, 246))
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
                hardware_available: local_availability.supports(package.hardware),
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
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(i18n::tr(language, "Model Packages"))
                    .size(15.0)
                    .color(theme::text_strong())
                    .strong(),
            );
            if !missing.is_empty() {
                let active_downloads = missing.iter().filter(|item| {
                    !app.model_task_manager.is_model_paused(item.id)
                        && (app.model_task_manager.is_model_queued(item.id)
                            || matches!(app.model_task_manager.state(),
                                NativeModelTaskState::Installing { asset_id, .. } if *asset_id == item.id))
                }).map(|item| item.id).collect::<Vec<_>>();
                let action = if !active_downloads.is_empty() {
                    "Pause"
                } else if missing.iter().any(|item| app.model_task_manager.is_model_paused(item.id)) {
                    "Resume"
                } else {
                    "Download"
                };
                let label = format!(
                    "{} ({}) · {}",
                    i18n::tr(language, action),
                    missing.len(),
                    components::format_file_size(missing_download_bytes),
                );
                let downloaded = missing.iter().filter_map(|item| {
                    app.model_task_manager.model_download_progress(item.id).map(|(bytes, _)| bytes)
                }).sum::<u64>();
                let progress = (missing_download_bytes > 0 && downloaded > 0)
                    .then(|| downloaded as f32 / missing_download_bytes as f32);
                let clicked = ui.scope(|ui| {
                    ui.set_max_width(220.0);
                    components::download_progress_button(
                        ui,
                        &label,
                        false,
                        !app.runtime_installer.is_busy(),
                        progress,
                    ).clicked()
                }).inner;
                if clicked {
                    if active_downloads.is_empty() {
                        installs.extend(missing.iter().map(|item| item.id));
                    } else {
                        pauses.extend(active_downloads);
                    }
                }
            }
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
        if !missing.is_empty() {
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!(
                    "{}: {}",
                    i18n::tr(language, "Installed size"),
                    components::format_file_size(missing_installed_bytes)
                ))
                .size(12.0)
                .color(theme::text_weak()),
            );
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
            let paused = app.model_task_manager.is_model_paused(item.id);
            let action = if item.installed {
                "Installed"
            } else if paused {
                "Resume"
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
                !item.installed && item.hardware_available && !app.runtime_installer.is_busy(),
                !busy,
                if paused { None } else { queued_position },
                app.model_task_manager.model_download_progress(item.id),
            );
            if clicked {
                if !paused && (is_active || queued_position.is_some()) {
                    pauses.push(item.id);
                } else {
                    installs.push(item.id);
                }
            }
            if delete_clicked {
                delete_model = Some(item.id);
            }
            ui.add_space(8.0);
        }
    }

    for asset_id in pauses {
        if let Err(error) = app.model_task_manager.pause(asset_id) {
            app.last_error = Some(error);
        }
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

    render_model_task_state(ui, language, app.model_task_manager.state());

    let requires_runtime = requirements.llama_cpp || requirements.onnx_tts || requirements.onnx_cpu;
    if requires_runtime {
        ui.add_space(16.0);
        render_runtime_installation_section(app, ui, language, &project_root);
    } else if configuration_loaded && download_items.is_empty() && !requirements.missing_api_key {
        onboarding_surface(ui, Color32::from_rgb(16, 185, 129)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.add(egui::Image::new(egui::include_image!("../../../resources/icons/check.svg"))
                    .fit_to_exact_size(egui::vec2(24.0, 24.0)));
                ui.label(i18n::tr(language, "Your selected online providers are ready. No local model packages are required."));
            });
        });
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
    progress: Option<(u64, u64)>,
) -> (bool, bool) {
    let mut clicked = false;
    let mut delete_clicked = false;
    onboarding_surface(ui, item.stroke_color)
        .inner_margin(Margin::symmetric(18, 14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let compact = ui.available_width() < 680.0;
            let layout = if compact {
                Layout::top_down(Align::Min)
            } else {
                Layout::left_to_right(Align::Min)
            };
            // `with_layout` inherits the scroll area's remaining height. A centered
            // horizontal layout then pushes the first card's contents down into
            // that space. Start with no requested height and grow with the content.
            let row_size = egui::vec2(ui.available_width(), 0.0);
            ui.allocate_ui_with_layout(row_size, layout, |ui| {
                let details_width = if compact {
                    ui.available_width()
                } else {
                    ui.available_width() - 200.0 - ui.spacing().item_spacing.x
                };
                ui.allocate_ui_with_layout(
                    egui::vec2(details_width, 0.0),
                    Layout::top_down(Align::Min),
                    |ui| {
                        // Allocation alone only caps this column's width; reserve
                        // it so short descriptions keep actions in their own column.
                        ui.set_width(details_width);
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
                        )
                        .on_hover_text(format!(
                            "{}: {}",
                            i18n::tr(language, "Installed size"),
                            components::format_file_size(item.installed_bytes),
                        ));
                    },
                );
                ui.horizontal(|ui| {
                    if item.installed {
                        if ui
                            .add_enabled_ui(delete_enabled, |ui| {
                                components::resource_delete_button(ui, item.id, language)
                            })
                            .inner
                            .clicked()
                        {
                            delete_clicked = true;
                        }
                        Frame::new()
                            .fill(Color32::from_rgba_unmultiplied(16, 185, 129, 20))
                            .corner_radius(CornerRadius::same(8))
                            .inner_margin(Margin::symmetric(14, 5))
                            .show(ui, |ui| {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(i18n::tr(language, "Installed"))
                                            .color(Color32::from_rgb(5, 150, 105))
                                            .size(12.5)
                                            .strong(),
                                    )
                                    .truncate(),
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
                        let response = ui
                            .push_id(("download_card", item.id), |ui| {
                                ui.set_max_width(200.0);
                                components::download_progress_button(
                                    ui,
                                    &action_label,
                                    false,
                                    enabled,
                                    progress.map(|(bytes, total)| {
                                        if total > 0 {
                                            bytes as f32 / total as f32
                                        } else {
                                            0.0
                                        }
                                    }),
                                )
                            })
                            .inner;
                        let response = if matches!(action, "Downloading" | "Queued") {
                            response.on_hover_text(i18n::tr(language, "Pause"))
                        } else {
                            response
                        };
                        if response.clicked() {
                            clicked = true;
                        }
                    }
                });
            });
        });
    (clicked, delete_clicked)
}

fn render_runtime_installation_section(
    app: &mut crate::XRTranslateApp,
    ui: &mut egui::Ui,
    language: i18n::UiLanguage,
    project_root: &std::path::Path,
) {
    use crate::runtime_install::RuntimeInstallState;

    let state = app.runtime_installer.state().clone();
    let paused = app.runtime_installer.is_paused();
    let downloading = matches!(state, RuntimeInstallState::Downloading { .. }) && !paused;
    let runtime_busy = app.runtime_installer.is_busy();
    let download_progress = app.runtime_installer.download_progress();
    let download_size = app.runtime_installer.download_size_bytes();
    let planned_downloads = app.runtime_installer.planned_downloads().to_vec();
    let backend_name = app
        .runtime_installer
        .backend_label()
        .unwrap_or("CPU")
        .to_owned();
    let cuda_version = app.runtime_installer.cuda_version_label();
    let backend = cuda_version.map_or(backend_name.clone(), |version| {
        format!("{backend_name} {version}")
    });

    let managed_runtime_present = app
        .runtime_installer
        .managed_resources_are_present(project_root);

    onboarding_surface(ui, Color32::from_rgb(16, 185, 129)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(
            RichText::new(i18n::tr(language, "Runtime"))
                .size(14.5)
                .color(Color32::from_rgb(4, 120, 87))
                .strong(),
        );
        ui.add_space(8.0);
        let ready = app.runtime_installer.plan_is_ready() && !runtime_busy && !paused;
        let mut use_mirror = app.runtime_installer.use_mirror();
        let previous_source = use_mirror;
        let mut delete_runtime = false;
        ui.horizontal_wrapped(|ui| {
            if ready {
                Frame::new()
                    .fill(Color32::from_rgba_unmultiplied(16, 185, 129, 20))
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
                let button_text = match (&state, download_size) {
                    _ if paused => i18n::tr(language, "Resume").to_owned(),
                    (RuntimeInstallState::Downloading { .. }, _) => {
                        i18n::tr(language, "Downloading").to_owned()
                    }
                    (RuntimeInstallState::Extracting, _) => {
                        i18n::tr(language, "Preparing…").to_owned()
                    }
                    (_, None) => i18n::tr(language, "Preparing download…").to_owned(),
                    (_, Some(0)) => i18n::tr(language, "Configure runtime").to_owned(),
                    (_, Some(bytes)) => format!(
                        "{} · {}",
                        i18n::tr(language, "Download"),
                        components::format_file_size(bytes)
                    ),
                };
                let response = ui
                    .scope(|ui| {
                        ui.set_max_width(200.0);
                        components::download_progress_button(
                            ui,
                            &button_text,
                            false,
                            downloading
                                || (download_size.is_some()
                                    && !runtime_busy
                                    && !app.model_task_manager.is_busy()),
                            download_progress.map(|(bytes, total)| {
                                if total > 0 {
                                    bytes as f32 / total as f32
                                } else {
                                    0.0
                                }
                            }),
                        )
                    })
                    .inner;
                let response =
                    if downloading && let RuntimeInstallState::Downloading { asset, .. } = &state {
                        response.on_hover_text(format!("{}\n{asset}", i18n::tr(language, "Pause")))
                    } else {
                        response
                    };
                if response.clicked() {
                    let result = if downloading {
                        app.runtime_installer.pause()
                    } else {
                        app.runtime_installer
                            .install_recommended(project_root.to_path_buf())
                    };
                    if let Err(error) = result {
                        app.last_error = Some(error);
                    }
                }
            }
            if managed_runtime_present
                && ui
                    .add_enabled_ui(!runtime_busy && !app.model_task_manager.is_busy(), |ui| {
                        components::resource_delete_button(ui, "managed_runtime", language)
                    })
                    .inner
                    .clicked()
            {
                delete_runtime = true;
            }
            components::download_mirror_toggle(ui, language, &mut use_mirror);
            if matches!(
                state,
                RuntimeInstallState::Detecting | RuntimeInstallState::Extracting
            ) {
                ui.spinner();
            }
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

        if !planned_downloads.is_empty() {
            ui.add_space(8.0);
            ui.collapsing(i18n::tr(language, "Details"), |ui| {
                render_runtime_download_plan(ui, &planned_downloads);
            });
        }

        components::render_runtime_fallback_notice(ui, language, &app.runtime_installer);

        if let RuntimeInstallState::Failed(error) = &state {
            ui.add_space(6.0);
            components::error_notice(ui, language, error);
        }
    });
}

fn render_runtime_download_plan(
    ui: &mut egui::Ui,
    downloads: &[crate::runtime_install::RuntimeDownload],
) {
    for download in downloads {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(&download.label).size(12.0))
                .on_hover_text(&download.archive_name);
            ui.label(
                RichText::new(components::format_file_size(download.bytes))
                    .size(11.5)
                    .color(theme::text_weak()),
            );
        });
    }
}

fn render_model_task_state(
    ui: &mut egui::Ui,
    language: i18n::UiLanguage,
    state: &NativeModelTaskState,
) {
    if let NativeModelTaskState::Failed(error) = state {
        ui.add_space(6.0);
        components::error_notice(ui, language, error);
    }
}
