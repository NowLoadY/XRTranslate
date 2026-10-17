//! A small in-app view of the same lookup task used by the native shortcut.
use crate::{
    history::archive::vocabulary::DEFAULT_FOLDER,
    i18n::{UiLanguage, tr, tr_dynamic},
    overlay_ipc::{QuickTranslationAction as Action, QuickTranslationState},
    ui::components,
};
use eframe::egui;
use xrtranslate_engine::language::{LanguageCapabilities, LanguageSelection};
use xrtranslate_prompt::{ContextFacts, SurroundingSource, TaskContext};

pub(super) struct Lookup {
    pub context: String,
    pub context_offset: usize,
    anchor: egui::Rect,
    folders_open: bool,
}

impl Lookup {
    pub fn new(context: String, context_offset: usize, anchor: egui::Rect) -> Self {
        Self {
            context,
            context_offset,
            anchor,
            folders_open: false,
        }
    }
}

pub(super) enum Interaction {
    Action(Action),
    Retry,
}

/// A history lookup owns its language intent. Script hints only choose a useful
/// explanation language; the service detects the actual language of the word.
pub(super) fn languages(
    text: &str,
    context: &str,
    language: UiLanguage,
    host_route: [&str; 2],
    capabilities: LanguageCapabilities,
) -> Result<LanguageSelection, String> {
    let capabilities = capabilities.for_text();
    let mut detected = xrtranslate_engine::detect_text_language(text);
    if detected == Some("en") {
        // Latin script alone is not evidence of English.
        detected = None;
    }
    if detected == Some("zh")
        && xrtranslate_engine::detect_text_language(context) == Some("ja")
        && !xrtranslate_engine::has_distinct_chinese_markers(text)
    {
        detected = Some("ja");
    }
    let preferred = match language {
        UiLanguage::English => "en",
        UiLanguage::Chinese => "zh",
        UiLanguage::Japanese => "ja",
        UiLanguage::Korean => "ko",
        UiLanguage::Russian => "ru",
    };
    let target = std::iter::once(preferred)
        .chain(host_route.into_iter().flat_map(|route| route.split(',')))
        .chain(
            capabilities
                .targets()
                .iter()
                .map(|language| language.code()),
        )
        .find(|target| {
            detected.is_none_or(|source| !crate::languages_conflict(source, target))
                && capabilities.select("auto", target).is_ok()
        })
        .ok_or("No different translation language is available.")?;
    capabilities.select("auto", target)
}

pub(super) fn task_context(
    text: &str,
    context: &str,
    context_offset: usize,
    source_language: &str,
) -> TaskContext {
    let Some(end) = context_offset.checked_add(text.len()) else {
        return TaskContext::default();
    };
    if text.is_empty() || context.get(context_offset..end) != Some(text) {
        return TaskContext::default();
    }
    let (before, after) = (&context[..context_offset], &context[end..]);
    let surrounding_source =
        (!before.trim().is_empty() || !after.trim().is_empty()).then(|| SurroundingSource {
            speaker_id: String::new(),
            source_language: source_language.to_owned(),
            before: before.to_owned(),
            after: after.to_owned(),
        });
    TaskContext::snapshot(ContextFacts {
        surrounding_source,
        ..Default::default()
    })
}

pub(super) fn show(
    view: &QuickTranslationState,
    lookup: &mut Lookup,
    language: UiLanguage,
    presentation_dark: Option<bool>,
    ctx: &egui::Context,
) -> Option<Interaction> {
    let bounds = ctx.content_rect().shrink(10.0);
    let width = 340.0_f32.min((bounds.width() - 24.0).max(120.0));
    let above = lookup.anchor.center().y > bounds.center().y;
    let position = if above {
        lookup.anchor.left_top() - egui::vec2(0.0, 8.0)
    } else {
        lookup.anchor.left_bottom() + egui::vec2(0.0, 8.0)
    };
    let mut interaction = None;
    let had_popup = egui::Popup::is_any_open(ctx);
    let area = egui::Area::new(egui::Id::new("history_lookup"))
        .order(egui::Order::Foreground)
        .pivot(if above {
            egui::Align2::LEFT_BOTTOM
        } else {
            egui::Align2::LEFT_TOP
        })
        .fixed_pos(position)
        .constrain_to(bounds)
        .show(ctx, |ui| {
            if let Some(dark) = presentation_dark {
                *ui.visuals_mut() = if dark {
                    egui::Visuals::dark()
                } else {
                    egui::Visuals::light()
                };
            }
            egui::Frame::popup(ui.style())
                .inner_margin(12)
                .show(ui, |ui| {
                    ui.set_width(width);
                    ui.label(egui::RichText::new(&view.source).strong());
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        let mut target = view.target_lang.clone();
                        ui.add_enabled_ui(!view.targets.is_empty(), |ui| {
                            egui::ComboBox::from_id_salt("lookup_target")
                                .width((width - 122.0).max(80.0))
                                .selected_text(crate::language_label(language, &target))
                                .show_ui(ui, |ui| {
                                    for code in &view.targets {
                                        ui.selectable_value(
                                            &mut target,
                                            code.clone(),
                                            crate::language_label(language, code),
                                        );
                                    }
                                })
                                .response
                                .on_hover_text(tr(language, "Target language"));
                        });
                        if target != view.target_lang {
                            lookup.folders_open = false;
                            interaction = Some(Interaction::Action(Action::Target(target)));
                        }
                        let ready = !view.busy && view.error.is_none();
                        let copied = view.copy_count > 0 && view.copy_error.is_none();
                        if ui
                            .add_enabled_ui(ready && !view.translated.is_empty(), |ui| {
                                components::icon_button(
                                    ui,
                                    tr(language, if copied { "Copied" } else { "Copy" }),
                                    if copied {
                                        egui::include_image!("../../resources/icons/check.svg")
                                    } else {
                                        egui::include_image!("../../resources/icons/copy.svg")
                                    },
                                )
                            })
                            .inner
                            .clicked()
                        {
                            interaction = Some(Interaction::Action(Action::Copy));
                        }
                        let saved = view.folders.iter().any(|folder| folder.saved);
                        let save_label = if view.saving {
                            "Saving…"
                        } else if saved {
                            "Save to another wordbook"
                        } else {
                            "Save to vocabulary"
                        };
                        if ui
                            .add_enabled_ui(ready && view.can_save && !view.saving, |ui| {
                                components::icon_button(
                                    ui,
                                    tr(language, save_label),
                                    if saved {
                                        egui::include_image!(
                                            "../../resources/icons/star-filled.svg"
                                        )
                                    } else {
                                        egui::include_image!("../../resources/icons/star.svg")
                                    },
                                )
                            })
                            .inner
                            .clicked()
                        {
                            if saved {
                                lookup.folders_open = !lookup.folders_open;
                                if lookup.folders_open {
                                    interaction = Some(Interaction::Action(Action::Folders));
                                }
                            } else {
                                interaction =
                                    Some(Interaction::Action(Action::Save(DEFAULT_FOLDER)));
                            }
                        }
                        if components::icon_button(
                            ui,
                            tr(language, "Close"),
                            egui::include_image!("../../resources/icons/x.svg"),
                        )
                        .clicked()
                        {
                            interaction = Some(Interaction::Action(Action::Close));
                        }
                    });
                    ui.add_space(8.0);
                    egui::ScrollArea::vertical()
                        .id_salt("lookup_result")
                        .max_height((bounds.height() * 0.55).max(40.0))
                        .show(ui, |ui| {
                            if view.busy {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.weak(tr(language, "Translating…"));
                                });
                            } else if let Some(error) = &view.error {
                                ui.colored_label(
                                    ui.visuals().error_fg_color,
                                    tr_dynamic(language, error),
                                );
                                if components::icon_button(
                                    ui,
                                    tr(language, "Retry"),
                                    egui::include_image!("../../resources/icons/reset.svg"),
                                )
                                .clicked()
                                {
                                    interaction = Some(Interaction::Retry);
                                }
                            } else {
                                ui.add(egui::Label::new(&view.translated).wrap().selectable(true));
                            }
                            for error in [&view.copy_error, &view.save_error].into_iter().flatten()
                            {
                                ui.colored_label(
                                    ui.visuals().error_fg_color,
                                    tr_dynamic(language, error),
                                );
                            }
                            if view.saving {
                                ui.weak(tr(language, "Saving…"));
                            } else if view.folders.iter().any(|folder| folder.saved) {
                                ui.weak(tr(language, "Saved to vocabulary"));
                            }
                            if lookup.folders_open {
                                ui.separator();
                                for folder in &view.folders {
                                    if ui
                                        .add_enabled(
                                            !folder.saved && !view.saving,
                                            egui::Button::selectable(
                                                folder.saved,
                                                folder.label(language),
                                            ),
                                        )
                                        .clicked()
                                    {
                                        interaction =
                                            Some(Interaction::Action(Action::Save(folder.id)));
                                    }
                                }
                            }
                        });
                });
        });
    if interaction.is_none()
        && !had_popup
        && !egui::Popup::is_any_open(ctx)
        && ctx.input(|input| {
            input.pointer.primary_pressed()
                && input
                    .pointer
                    .interact_pos()
                    .is_some_and(|position| !area.response.rect.contains(position))
        })
    {
        interaction = Some(Interaction::Action(Action::Close));
    }
    interaction
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(
        ctx: &egui::Context,
        lookup: &mut Lookup,
        events: Vec<egui::Event>,
    ) -> Option<Interaction> {
        let view = QuickTranslationState {
            source: "bank".into(),
            translated: "银行".into(),
            source_lang: "en".into(),
            target_lang: "zh".into(),
            targets: vec!["zh".into(), "ja".into()],
            can_save: true,
            ..Default::default()
        };
        let mut action = None;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                events,
                ..Default::default()
            },
            |ui| action = show(&view, lookup, UiLanguage::Chinese, Some(true), ui.ctx()),
        );
        output.textures_delta.clear();
        action
    }

    #[test]
    fn lookup_stays_on_screen_and_only_outside_presses_dismiss_it() {
        for (outside, popup_open, expected_close) in [
            (true, false, true),
            (false, false, false),
            (true, true, false),
        ] {
            let ctx = egui::Context::default();
            egui_extras::install_image_loaders(&ctx);
            let mut lookup = Lookup::new(
                "bank".into(),
                0,
                egui::Rect::from_min_size(egui::pos2(770.0, 560.0), egui::vec2(20.0, 20.0)),
            );
            frame(&ctx, &mut lookup, Vec::new());
            frame(&ctx, &mut lookup, Vec::new());
            let card = egui::containers::AreaState::load(&ctx, egui::Id::new("history_lookup"))
                .unwrap()
                .rect();
            assert!(
                egui::Rect::from_min_max(egui::pos2(10.0, 10.0), egui::pos2(790.0, 590.0))
                    .contains_rect(card)
            );
            if popup_open {
                egui::Popup::open_id(&ctx, egui::Id::new("language_menu"));
            }
            let position = if outside {
                egui::pos2(30.0, 30.0)
            } else {
                card.center()
            };
            let action = frame(
                &ctx,
                &mut lookup,
                vec![
                    egui::Event::PointerMoved(position),
                    egui::Event::PointerButton {
                        pos: position,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
            assert_eq!(
                matches!(action, Some(Interaction::Action(Action::Close))),
                expected_close
            );
        }
    }

    #[test]
    fn lookup_context_keeps_unicode_surroundings_separate_from_selected_text() {
        let TaskContext::Snapshot { facts } =
            task_context("银行", "去银行取钱。", "去".len(), "zh")
        else {
            panic!("lookup must have an independent context snapshot");
        };
        let surrounding = facts.surrounding_source.unwrap();
        assert_eq!(surrounding.before, "去");
        assert_eq!(surrounding.after, "取钱。");
        assert_eq!(surrounding.source_language, "zh");
    }

    #[test]
    fn lookup_omits_unrelated_or_empty_context() {
        for (text, context, offset) in [
            ("bank", "bank", 0),
            ("bank", "", 0),
            ("bank", "unrelated text", 0),
            ("银行", "去银行取钱。", 1),
            ("bank", "bank", usize::MAX),
        ] {
            let TaskContext::Snapshot { facts } = task_context(text, context, offset, "en") else {
                unreachable!();
            };
            assert!(facts.surrounding_source.is_none());
        }
    }

    #[test]
    fn lookup_language_is_independent_of_the_live_translation_direction() {
        for (text, ui, route, expected) in [
            ("银行", UiLanguage::Chinese, ["en", "zh"], "en"),
            ("bank", UiLanguage::Chinese, ["zh", "en"], "zh"),
            ("银行", UiLanguage::English, ["en", "zh"], "en"),
            ("bank", UiLanguage::English, ["zh", "en"], "en"),
            ("은행", UiLanguage::Chinese, ["auto", "en,ja"], "zh"),
            ("bonjour", UiLanguage::English, ["en", "fr"], "en"),
        ] {
            let selected =
                languages(text, text, ui, route, LanguageCapabilities::default()).unwrap();
            assert_eq!(selected.wire(), ("auto".into(), expected.into()));
        }
    }

    #[test]
    fn japanese_context_disambiguates_a_selected_kanji_word() {
        let selected = languages(
            "銀行",
            "銀行に行きます",
            UiLanguage::Chinese,
            ["en", "zh"],
            LanguageCapabilities::default(),
        )
        .unwrap();
        assert_eq!(selected.wire(), ("auto".into(), "zh".into()));
    }

    #[test]
    fn lookup_context_uses_the_selected_occurrence_of_a_repeated_word() {
        let sentence = "bank by the river bank";
        let offset = sentence.rfind("bank").unwrap();
        let TaskContext::Snapshot { facts } = task_context("bank", sentence, offset, "auto") else {
            unreachable!();
        };
        let surrounding = facts.surrounding_source.unwrap();
        assert_eq!(surrounding.before, "bank by the river ");
        assert!(surrounding.after.is_empty());
    }

    #[test]
    fn lookup_respects_supported_languages_and_requires_a_different_target() {
        let capabilities = |codes: &[&str]| LanguageCapabilities {
            translation: Some(xrtranslate_engine::language::LanguageSet::from_codes(
                codes, false,
            )),
            recognition: None,
        };
        let selected = languages(
            "银行",
            "去银行",
            UiLanguage::Korean,
            ["en", "zh"],
            capabilities(&["zh", "ja"]),
        )
        .unwrap();
        assert_eq!(selected.wire(), ("auto".into(), "ja".into()));
        for (text, codes) in [("银行", vec!["zh"]), ("銀行です", vec![])] {
            assert!(
                languages(
                    text,
                    text,
                    UiLanguage::Chinese,
                    ["en", "zh"],
                    capabilities(&codes)
                )
                .is_err()
            );
        }
    }
}
