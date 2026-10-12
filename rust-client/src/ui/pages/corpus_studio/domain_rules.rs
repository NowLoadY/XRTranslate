//! Shared activation is configured once per domain and inherited by its vocabulary.
use super::*;

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Inherit,
    Direct,
    Cues,
}

pub(super) struct Draft {
    domain: GraphDomain,
    mode: Mode,
    cues: String,
    inherit_existing: bool,
}

impl Draft {
    pub fn new(domain: &GraphDomain) -> Self {
        Self {
            domain: domain.clone(),
            mode: match domain.cue_words.as_ref() {
                None => Mode::Inherit,
                Some(words) if words.is_empty() => Mode::Direct,
                Some(_) => Mode::Cues,
            },
            cues: domain
                .cue_words
                .as_ref()
                .map_or(String::new(), |words| words.join("\n")),
            inherit_existing: false,
        }
    }
}

pub(super) fn summary(snapshot: &GraphSnapshot, id: &str, language: UiLanguage) -> String {
    let path = domain_path(snapshot, id);
    match path.iter().rev().find(|domain| domain.cue_words.is_some()) {
        Some(domain)
            if domain
                .cue_words
                .as_ref()
                .is_some_and(|words| !words.is_empty()) =>
        {
            format!(
                "{} · {}: {}",
                tr_dynamic(language, &domain.title),
                tr(language, "Cue words"),
                {
                    let words = domain.cue_words.as_ref().unwrap();
                    let mut preview = words
                        .iter()
                        .take(3)
                        .map(|word| {
                            let prefix: String = word.chars().take(28).collect();
                            if prefix.len() < word.len() {
                                format!("{prefix}…")
                            } else {
                                prefix
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(" / ");
                    if words.len() > 3 {
                        preview.push_str(&format!(" +{}", words.len() - 3));
                    }
                    preview
                }
            )
        }
        _ => tr(language, "Match directly").to_owned(),
    }
}

pub(super) fn render(
    controller: &mut CorpusStudioController,
    ctx: &egui::Context,
    language: UiLanguage,
) {
    let Some(mut draft) = controller.domain_rules.take() else {
        return;
    };
    let Some(snapshot) = controller.snapshot.clone() else {
        return;
    };
    let counts = controller
        .term_index
        .counts
        .get(&draft.domain.id)
        .copied()
        .unwrap_or_default();
    let mut save = false;
    let valid = std::cell::Cell::new(true);
    let close = crate::ui::modal::dialog(
        ctx,
        egui::Id::new("domain_rules"),
        language,
        &format!(
            "{} · {}",
            tr(language, "Domain rules"),
            tr_dynamic(language, &draft.domain.title)
        ),
        |ui| {
            ui.add_enabled_ui(!controller.pending, |ui| {
                if let Some(error) = &controller.error { components::error_notice(ui, language, error); }
                ui.label(tr(language, "Configure the domain once. Its terms and inheriting subdomains share these rules."));
                ui.small(format!("{} {} · {} {}", counts.0, tr(language, "terms"), counts.1, tr(language, "Custom rules")));
                ui.radio_value(&mut draft.mode, Mode::Inherit, tr(language, "Inherit parent rules"));
                ui.radio_value(&mut draft.mode, Mode::Direct, tr(language, "Match directly"));
                ui.radio_value(&mut draft.mode, Mode::Cues, tr(language, "Activate by topic"));
                match draft.mode {
                    Mode::Inherit => {
                        ui.weak(draft.domain.parent_id.as_ref().map_or_else(
                            || tr(language, "Match directly").to_owned(), |id| summary(&snapshot, id, language)));
                    }
                    Mode::Direct => { ui.weak(tr(language, "Relevant terms are selected within the model budget.")); }
                    Mode::Cues => {
                        ui.label(tr(language, "Topic words · one per line, any language"));
                        ui.add(egui::TextEdit::multiline(&mut draft.cues).desired_width(ui.available_width()).desired_rows(5));
                        ui.weak(tr(language, "Any topic word activates the domain. Matching terms keep the topic active; unrelated conversation lets it fade."));
                        valid.set(draft.cues.lines().any(|line| !line.trim().is_empty())
                            && draft.cues.lines().all(|line| line.trim().chars().count() <= 512 && !line.contains(',')));
                        if !valid.get() { ui.weak(tr(language, "Enter at least one topic word; use a new line between words.")); }
                    }
                }
                if counts.1 > 0 {
                    ui.separator();
                    ui.checkbox(&mut draft.inherit_existing, tr(language, "Apply domain rules to existing terms and subdomains"));
                    ui.weak(tr(language, "Existing individual rules stay unchanged unless this is selected."));
                }
            });
        },
        |ui| {
            // Re-evaluate here: egui modal bodies and footers are separate closures.
            let valid = valid.get() && !controller.pending;
            save = components::primary_button_enabled(ui, tr(language, "Save"), valid).clicked();
        },
    );
    if save {
        let mut seen = HashSet::new();
        draft.domain.cue_words = match draft.mode {
            Mode::Inherit => None,
            Mode::Direct => Some(Vec::new()),
            Mode::Cues => Some(
                draft
                    .cues
                    .lines()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .filter(|s| seen.insert(s.to_lowercase()))
                    .map(str::to_owned)
                    .collect(),
            ),
        };
        controller.send(Command::SaveDomainRules(GraphDomainEdit {
            domain: draft.domain.clone(),
            inherit_existing: draft.inherit_existing,
        }));
    }
    if !close || controller.pending {
        controller.domain_rules = Some(draft);
    }
}
