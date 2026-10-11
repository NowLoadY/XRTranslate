//! Only explicit detail-page actions submit Ask requests. Each completed stage is saved.
use crate::{
    ask,
    history::archive::{
        Archive,
        vocabulary::{Action, Entry, Reply, Translation},
    },
    i18n::{UiLanguage, tr},
    ui::notifications::Notifications,
};
use eframe::egui;
use xrtranslate_engine::language::{LanguageCapabilities, Script, SupportedLanguage};

#[derive(Clone, Copy)]
pub(super) enum Step {
    Sentence,
    BackTranslation,
}

enum Pending {
    Ask(ask::Reply),
    Save(Reply),
}
struct Job {
    translation: Translation,
    step: Step,
    pending: Pending,
}

#[derive(Default)]
pub(super) struct Requests {
    jobs: Vec<Job>,
}

pub(super) fn supported(entry: &Entry, capabilities: LanguageCapabilities) -> bool {
    SupportedLanguage::parse(&entry.source_lang).is_some()
        && SupportedLanguage::parse(&entry.target_lang).is_some()
        && capabilities
            .for_text()
            .select(&entry.source_lang, &entry.target_lang)
            .is_ok()
        && capabilities
            .for_text()
            .select(&entry.target_lang, &entry.source_lang)
            .is_ok()
}

impl Requests {
    pub fn busy(&self, entry: &Entry) -> bool {
        self.jobs.iter().any(|job| job.translation.entry == *entry)
    }
    pub fn start(
        &mut self,
        row: &Translation,
        step: Step,
        capabilities: LanguageCapabilities,
        ask: &mut ask::Ask,
        ctx: &egui::Context,
    ) {
        if self.busy(&row.entry) || !supported(&row.entry, capabilities) {
            return;
        }
        let source = SupportedLanguage::parse(&row.entry.source_lang).unwrap();
        let target = SupportedLanguage::parse(&row.entry.target_lang).unwrap();
        let prompt = match step {
            Step::Sentence => format!(
                "Write one natural sentence in {} using this exact word or phrase. Output only the sentence.\n\n<word>{}</word>",
                target.name(),
                row.entry.translated
            ),
            Step::BackTranslation if !row.example.sentence.is_empty() => {
                format!(
                    "Translate this {} sentence into {}. Output only the translation.\n\n{}",
                    target.name(),
                    source.name(),
                    row.example.sentence
                )
            }
            _ => return,
        };
        self.jobs.push(Job {
            translation: row.clone(),
            step,
            pending: Pending::Ask(ask.request(prompt, ctx)),
        });
    }
    pub fn poll(
        &mut self,
        archive: &Archive,
        notifications: &Notifications,
        language: UiLanguage,
        ctx: &egui::Context,
    ) {
        self.jobs.retain_mut(|job| {
            let result = match &job.pending {
                Pending::Ask(reply) => match reply.try_recv() {
                    Ok(Ok(text))
                        if matches!(job.step, Step::Sentence)
                            && !valid_sentence(&text, &job.translation.entry) =>
                    {
                        Some(Err(
                            "The model did not return a usable example sentence".into()
                        ))
                    }
                    Ok(Ok(text)) => {
                        match job.step {
                            Step::Sentence => {
                                if job.translation.example.sentence != text {
                                    job.translation.example.back_translation.clear();
                                }
                                job.translation.example.sentence = text;
                            }
                            Step::BackTranslation => {
                                job.translation.example.back_translation = text
                            }
                        }
                        job.pending = Pending::Save(archive.vocabulary(
                            Action::SaveExample {
                                entry: job.translation.entry.clone(),
                                example: job.translation.example.clone(),
                            },
                            ctx,
                        ));
                        return true;
                    }
                    Ok(Err(error)) => Some(Err(error)),
                    Err(crossbeam_channel::TryRecvError::Empty) => None,
                    Err(_) => Some(Err("Ask request was interrupted".into())),
                },
                Pending::Save(reply) => match reply.try_recv() {
                    Ok(result) => Some(result.map(|_| ())),
                    Err(crossbeam_channel::TryRecvError::Empty) => None,
                    Err(_) => Some(Err("Could not save the example".into())),
                },
            };
            if let Some(result) = result {
                if let Err(error) = result {
                    let message = match job.step {
                        Step::Sentence => "Could not generate an example.",
                        Step::BackTranslation => "Could not back-translate the example.",
                    };
                    notifications.error(tr(language, message), error);
                }
                ctx.request_repaint();
                false
            } else {
                true
            }
        });
    }
}

/// Reject obvious instruction echoes and non-sentences without another model request.
fn valid_sentence(text: &str, entry: &Entry) -> bool {
    let normalize = |value: &str| {
        value
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    let text = text.trim();
    let sentence = normalize(text);
    let word = normalize(&entry.translated);
    let script =
        SupportedLanguage::parse(&entry.target_lang).and_then(|language| language.script());
    let separate_words = matches!(
        script,
        Some(Script::Latin | Script::Cyrillic | Script::Devanagari)
    );
    if word.is_empty()
        || text.chars().count() > 600
        || text.lines().filter(|line| !line.trim().is_empty()).count() > 1
        || [
            "<word",
            "</word",
            "<think",
            "```",
            "output only the sentence",
            "write one natural sentence",
        ]
        .iter()
        .any(|marker| sentence.contains(marker))
        || xrtranslate_engine::sentence_end_offsets(text).len() > 1
        || !sentence.match_indices(&word).any(|(at, _)| {
            !separate_words
                || (!sentence[..at].ends_with(char::is_alphabetic)
                    && !sentence[at + word.len()..].starts_with(char::is_alphabetic))
        })
    {
        return false;
    }
    let rest = sentence.replace(&word, "");
    // The example must add language around the word, rather than merely echo it.
    if !rest.chars().any(char::is_alphabetic) {
        return false;
    }
    script.is_none_or(|script| xrtranslate_engine::observed_scripts(&rest).contains(&script))
}
