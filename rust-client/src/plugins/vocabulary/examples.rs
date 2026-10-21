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
    Translation,
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
        let Some(prompt) = prompt(row, step) else {
            return;
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
                                if job.translation.example.sentence != text
                                    || job.translation.example.sentence_lang
                                        != job.translation.entry.target_lang
                                {
                                    job.translation.example.back_translation.clear();
                                }
                                job.translation.example.sentence = text;
                                job.translation.example.sentence_lang =
                                    job.translation.entry.target_lang.clone();
                            }
                            Step::Translation => job.translation.example.back_translation = text,
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
                        Step::Translation => "Could not translate the example.",
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

fn prompt(row: &Translation, step: Step) -> Option<String> {
    let source = SupportedLanguage::parse(&row.entry.source_lang)?;
    let target = SupportedLanguage::parse(&row.entry.target_lang)?;
    match step {
        Step::Sentence => Some(format!(
            "Write one natural sentence in {} using <word> exactly, with the meaning of <meaning> ({}). Output only the sentence.\n\n<word>{}</word>\n<meaning>{}</meaning>",
            target.name(),
            source.name(),
            row.entry.translated,
            row.entry.source
        )),
        Step::Translation if !row.example.sentence.is_empty() => {
            // Examples saved by previous versions can use either language.
            // Records without a language tag used the card's target language.
            let sentence_language = if row.example.sentence_lang.is_empty() {
                target
            } else {
                SupportedLanguage::parse(&row.example.sentence_lang)?
            };
            let translation_language = if sentence_language == source {
                target
            } else if sentence_language == target {
                source
            } else {
                return None;
            };
            Some(format!(
                "Translate this {} sentence into {}. Output only the translation.\n\n{}",
                sentence_language.name(),
                translation_language.name(),
                row.example.sentence
            ))
        }
        Step::Translation => None,
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
            "<source",
            "</source",
            "<meaning",
            "</meaning",
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

#[cfg(test)]
mod tests {
    use super::*;

    fn word() -> Translation {
        Translation {
            id: 1,
            entry: Entry {
                source: "resilient".into(),
                translated: "有韧性的".into(),
                source_lang: "en".into(),
                target_lang: "zh".into(),
            },
            example: Default::default(),
        }
    }

    #[test]
    fn generated_example_uses_the_card_word_and_language() {
        let row = word();
        let request = prompt(&row, Step::Sentence).unwrap();
        assert!(request.starts_with("Write one natural sentence in Chinese"));
        assert!(request.contains("<word>有韧性的</word>"));
        assert!(request.contains("<meaning>resilient</meaning>"));
        assert!(valid_sentence("这是一棵有韧性的树。", &row.entry));
        assert!(!valid_sentence("This tree is resilient.", &row.entry));
    }

    #[test]
    fn chinese_source_produces_english_and_japanese_card_examples() {
        for (tag, language, translated, sentence) in [
            (
                "en",
                "English",
                "electrical equipment",
                "The factory regularly maintains its electrical equipment.",
            ),
            (
                "ja",
                "Japanese",
                "電気機器",
                "工場の電気機器を定期的に点検します。",
            ),
        ] {
            let mut row = word();
            row.entry = Entry {
                source: "电气设备".into(),
                translated: translated.into(),
                source_lang: "zh".into(),
                target_lang: tag.into(),
            };
            let request = prompt(&row, Step::Sentence).unwrap();
            assert!(request.starts_with(&format!("Write one natural sentence in {language}")));
            assert!(request.contains(&format!("<word>{translated}</word>")));
            assert!(request.contains("<meaning>电气设备</meaning>"));
            assert!(valid_sentence(sentence, &row.entry));
            assert!(!valid_sentence(
                "工厂需要定期维护所有的电气设备。",
                &row.entry
            ));
            assert!(!valid_sentence(
                &format!("工厂需要维护{translated}。"),
                &row.entry
            ));
            assert!(!valid_sentence(translated, &row.entry));

            row.example.sentence = sentence.into();
            row.example.sentence_lang = tag.into();
            assert!(
                prompt(&row, Step::Translation)
                    .unwrap()
                    .starts_with(&format!("Translate this {language} sentence into Chinese."))
            );
        }
    }

    #[test]
    fn example_translation_respects_legacy_and_new_directions() {
        let mut row = word();
        row.example.sentence = "这是一棵有韧性的树。".into();
        let old_request = prompt(&row, Step::Translation).unwrap();
        assert!(old_request.starts_with("Translate this Chinese sentence into English."));

        row.example.sentence = "This tree is resilient.".into();
        row.example.sentence_lang = "en".into();
        let new_request = prompt(&row, Step::Translation).unwrap();
        assert!(new_request.starts_with("Translate this English sentence into Chinese."));

        // Shared scripts must not cause guessing based on the sentence's text.
        row.entry.target_lang = "fr".into();
        row.example.sentence_lang.clear();
        assert!(
            prompt(&row, Step::Translation)
                .unwrap()
                .starts_with("Translate this French sentence into English.")
        );
    }
}
