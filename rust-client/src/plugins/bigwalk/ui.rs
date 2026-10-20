use super::{BigWalkPlugin, BigWalkSnapshot, ConnectionStatus};
use crate::{
    i18n::{UiLanguage, tr, tr_dynamic},
    plugins::text_output::MessageFormat,
    ui::{
        components::{
            self,
            text_composer::{ComposerContext, TextAction, TextMode},
        },
        theme,
    },
};
use eframe::egui;
use xrtranslate_engine::language::LanguageCapabilities;

pub(crate) struct BigWalkPageContext<'a> {
    pub language: UiLanguage,
    pub preparing_text: bool,
    pub languages: LanguageCapabilities,
    pub source_lang: &'a str,
    pub target_lang: &'a str,
    pub additional_target_lang: Option<&'a str>,
    pub asr_only: bool,
}

pub(crate) enum BigWalkUiAction {
    SettingsChanged,
    SetLanguageRoute {
        source_lang: String,
        target_lang: String,
        asr_only: bool,
    },
    TranslateInput {
        text: String,
    },
}

impl BigWalkPlugin {
    pub(crate) fn render_content(
        &mut self,
        ui: &mut egui::Ui,
        context: BigWalkPageContext<'_>,
    ) -> Vec<BigWalkUiAction> {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(300));
        let mut actions = Vec::new();
        let snapshot = self.snapshot();
        let width = ui.available_width();
        if width >= 720.0 {
            let left_width = (width * 0.46).clamp(320.0, 420.0);
            let right_width = width - left_width - ui.spacing().item_spacing.x;
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(left_width, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| self.render_controls(ui, context.language, &snapshot, &mut actions),
                );
                ui.allocate_ui_with_layout(
                    egui::vec2(right_width, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| render_last_sent(ui, context.language, &snapshot),
                );
            });
        } else {
            self.render_controls(ui, context.language, &snapshot, &mut actions);
            ui.add_space(12.0);
            render_last_sent(ui, context.language, &snapshot);
        }
        ui.add_space(16.0);
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| self.render_composer(ui, &context, &snapshot, &mut actions),
        );
        actions
    }

    fn render_controls(
        &mut self,
        ui: &mut egui::Ui,
        language: UiLanguage,
        snapshot: &BigWalkSnapshot,
        actions: &mut Vec<BigWalkUiAction>,
    ) {
        let mut changed = false;
        components::card(ui, |ui| {
            ui.set_width(ui.available_width());
            let status = match snapshot.status {
                ConnectionStatus::Disabled => "Sending is off",
                ConnectionStatus::Connecting => "Open Big Walk with the XRT bridge.",
                ConnectionStatus::WaitingForPlayer => "Join a game to send messages.",
                ConnectionStatus::Ready => "Connected",
            };
            ui.label(tr(language, status));
            if let Some(error) = &snapshot.error {
                ui.label(
                    egui::RichText::new(tr_dynamic(language, error))
                        .small()
                        .color(theme::danger()),
                );
            }
            ui.hyperlink_to(
                tr(language, "Setup guide"),
                "https://github.com/NowLoadY/XRT-Avatar-Plugin/blob/main/docs/adapters/bigwalk/installation.md",
            );
            ui.add_space(12.0);
            changed |=
                components::audio_source_filter(ui, &mut self.settings.audio_sources, language);
            ui.add_space(8.0);
            ui.label(tr(language, "Format:"));
            changed |= components::combobox_ui(
                ui,
                "bigwalk_message_format",
                self.settings.format.label(language),
                |ui| {
                    let mut changed = false;
                    for format in [
                        MessageFormat::TargetOnly,
                        MessageFormat::BilingualTargetFirst,
                        MessageFormat::BilingualSourceFirst,
                        MessageFormat::Inline,
                    ] {
                        changed |= ui
                            .selectable_value(
                                &mut self.settings.format,
                                format,
                                format.label(language),
                            )
                            .changed();
                    }
                    changed
                },
            )
            .inner
            .unwrap_or(false);
        });
        if changed {
            actions.push(BigWalkUiAction::SettingsChanged);
        }
    }

    fn render_composer(
        &mut self,
        ui: &mut egui::Ui,
        context: &BigWalkPageContext<'_>,
        snapshot: &BigWalkSnapshot,
        actions: &mut Vec<BigWalkUiAction>,
    ) {
        let mut source = context.source_lang.to_owned();
        let mut target = context.target_lang.to_owned();
        let mut asr_only = context.asr_only;
        if self.composer.mode == TextMode::Translate
            && components::translation_primary_language_selector(
                ui,
                "bigwalk_languages",
                &mut source,
                &mut target,
                context.additional_target_lang,
                &mut asr_only,
                context.languages,
                context.language,
            )
        {
            actions.push(BigWalkUiAction::SetLanguageRoute {
                source_lang: source.clone(),
                target_lang: target.clone(),
                asr_only,
            });
        }
        match self.composer.render(
            ui,
            "bigwalk_text_composer",
            (&mut source, &mut target),
            ComposerContext {
                language: context.language,
                capabilities: context.languages,
                preparing: context.preparing_text,
                translation_enabled: snapshot.status == ConnectionStatus::Ready,
                allow_direct: true,
                direct_enabled: snapshot.status == ConnectionStatus::Ready,
                show_languages: false,
            },
        ) {
            Some(TextAction::Translate { text, .. }) => {
                actions.push(BigWalkUiAction::TranslateInput { text })
            }
            Some(TextAction::Direct(text)) => {
                if self.send_manual(&text) {
                    self.composer.text.clear();
                }
            }
            None => {}
        }
    }
}

fn render_last_sent(ui: &mut egui::Ui, language: UiLanguage, snapshot: &BigWalkSnapshot) {
    components::card(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.set_min_height(160.0);
        ui.label(egui::RichText::new(tr(language, "Last sent")).strong());
        ui.add_space(12.0);
        if snapshot.last_sent.is_empty() {
            ui.label(
                egui::RichText::new(tr(language, "Nothing sent yet")).color(theme::text_weak()),
            );
        } else {
            ui.add(egui::Label::new(&snapshot.last_sent).wrap());
        }
        crate::ui::automation::record_output(
            ui,
            "Big Walk last sent",
            crate::ui::automation::ElementValue::Text(snapshot.last_sent.clone()),
            ui.max_rect(),
        );
        if snapshot.queued > 0 || snapshot.skipped > 0 {
            ui.add_space(12.0);
            ui.horizontal_wrapped(|ui| {
                if snapshot.queued > 0 {
                    ui.weak(format!("{}: {}", tr(language, "Queued"), snapshot.queued));
                }
                if snapshot.skipped > 0 {
                    ui.weak(format!("{}: {}", tr(language, "Skipped"), snapshot.skipped));
                }
            });
        }
    });
}
