//! Optional recognition spellings, edited beside their canonical language value.
use super::*;
use std::collections::BTreeMap;

#[derive(Clone, Default)]
pub(super) struct Draft {
    text: Option<BTreeMap<String, String>>,
    selected: Option<usize>,
}

impl Draft {
    pub fn render(
        &mut self,
        ui: &mut egui::Ui,
        node: &mut GraphNode,
        language: UiLanguage,
    ) -> bool {
        let text = self.text.get_or_insert_with(|| {
            node.recognition_aliases
                .iter()
                .map(|(language, aliases)| (language.clone(), aliases.join("\n")))
                .collect()
        });
        let mut changed = false;
        egui::CollapsingHeader::new(tr(language, "Common misrecognitions (optional)"))
            .id_salt(("recognition_aliases", &node.id))
            .default_open(!node.recognition_aliases.is_empty())
            .show(ui, |ui| {
                ui.weak(tr(
                    language,
                    "Applied when this term is active. Hotwords use the correct spelling.",
                ));
                let available: Vec<_> = (0..LANGUAGE_CODES.len())
                    .filter(|&index| {
                        !node.values[index].trim().is_empty()
                            || text
                                .get(LANGUAGE_CODES[index])
                                .is_some_and(|value| !value.trim().is_empty())
                    })
                    .collect();
                if available.is_empty() {
                    ui.weak(tr(language, "Enter the correct word first."));
                    return;
                }
                if self
                    .selected
                    .is_none_or(|index| !available.contains(&index))
                {
                    self.selected = available
                        .iter()
                        .copied()
                        .find(|&index| {
                            text.get(LANGUAGE_CODES[index])
                                .is_some_and(|value| !value.trim().is_empty())
                        })
                        .or_else(|| {
                            available
                                .iter()
                                .copied()
                                .find(|&index| index == preferred_language_index(language))
                        })
                        .or_else(|| available.first().copied());
                }
                let selected = self.selected.as_mut().unwrap();
                ui.horizontal_wrapped(|ui| {
                    egui::ComboBox::from_id_salt(("alias_language", &node.id))
                        .selected_text(editor::language_name(language, *selected))
                        .show_ui(ui, |ui| {
                            for &index in &available {
                                ui.selectable_value(
                                    selected,
                                    index,
                                    editor::language_name(language, index),
                                );
                            }
                        });
                    ui.label(format!("→ {}", node.values[*selected].trim()));
                });
                let code = LANGUAGE_CODES[*selected];
                let value = text.entry(code.into()).or_default();
                if ui
                    .add(
                        egui::TextEdit::multiline(value)
                            .id_salt(("aliases", code))
                            .desired_width(ui.available_width())
                            .desired_rows(2)
                            .hint_text(tr(language, "One spelling per line")),
                    )
                    .changed()
                {
                    let aliases: Vec<_> = value
                        .lines()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(str::to_owned)
                        .collect();
                    if aliases.is_empty() {
                        node.recognition_aliases.remove(code);
                    } else {
                        node.recognition_aliases.insert(code.into(), aliases);
                    }
                    changed = true;
                }
            });
        changed
    }
}

pub(super) fn valid(node: &GraphNode) -> bool {
    node.recognition_aliases.iter().all(|(language, aliases)| {
        LANGUAGE_CODES
            .iter()
            .position(|code| code == language)
            .is_some_and(|index| !node.values[index].trim().is_empty())
            && aliases
                .iter()
                .all(|value| !value.contains([',', '\r', '\n']) && value.chars().count() <= 512)
    })
}
