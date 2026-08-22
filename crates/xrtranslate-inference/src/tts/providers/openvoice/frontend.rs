//! English MeloTTS frontend for NVIDIA's OpenVoice v3 ONNX package.

use std::{collections::HashMap, path::Path};

use half::f16;
use ndarray::Array2;
use ort::{session::Session, value::Value};
use tokenizers::{
    Model, Tokenizer, decoders, models::wordpiece::WordPiece, normalizers::BertNormalizer,
    pre_tokenizers::bert::BertPreTokenizer, processors::bert::BertProcessing,
};

use crate::InferenceError;

const ENGLISH_LANGUAGE_ID: i32 = 2;
const ENGLISH_TONE_OFFSET: i32 = 7;

pub(super) struct EnglishInputs {
    pub(super) phone_ids: Vec<i32>,
    pub(super) tones: Vec<i32>,
    pub(super) language_ids: Vec<i32>,
    pub(super) bert: Vec<f16>,
}

pub(super) struct EnglishFrontend {
    tokenizer: Tokenizer,
    dictionary: HashMap<String, Vec<String>>,
    symbol_ids: HashMap<String, i32>,
}

impl EnglishFrontend {
    pub(super) fn load(model_dir: &Path, symbols: &[String]) -> Result<Self, InferenceError> {
        let vocab_path = model_dir.join("frontend/bert_vocab.txt");
        let wordpiece = WordPiece::from_file(&vocab_path.to_string_lossy())
            .build()
            .map_err(|error| frontend_error(error.to_string()))?;
        let vocabulary = wordpiece.get_vocab();
        let sep_id = *vocabulary
            .get("[SEP]")
            .ok_or_else(|| frontend_error("BERT vocabulary has no [SEP] token"))?;
        let cls_id = *vocabulary
            .get("[CLS]")
            .ok_or_else(|| frontend_error("BERT vocabulary has no [CLS] token"))?;
        let mut tokenizer = Tokenizer::new(wordpiece);
        tokenizer.with_normalizer(Some(BertNormalizer::default()));
        tokenizer.with_pre_tokenizer(Some(BertPreTokenizer));
        tokenizer.with_decoder(Some(decoders::wordpiece::WordPiece::default()));
        tokenizer.with_post_processor(Some(BertProcessing::new(
            ("[SEP]".to_owned(), sep_id),
            ("[CLS]".to_owned(), cls_id),
        )));
        let dictionary = serde_json::from_slice(
            &std::fs::read(model_dir.join("frontend/cmudict.json"))
                .map_err(|error| frontend_error(error.to_string()))?,
        )
        .map_err(|error| frontend_error(error.to_string()))?;
        let symbol_ids = symbols
            .iter()
            .enumerate()
            .map(|(index, symbol)| (symbol.clone(), index as i32))
            .collect();
        Ok(Self {
            tokenizer,
            dictionary,
            symbol_ids,
        })
    }

    pub(super) fn encode(
        &self,
        bert_session: &mut Session,
        text: &str,
    ) -> Result<EnglishInputs, InferenceError> {
        let normalized = normalize_text(text);
        if normalized.is_empty() {
            return Err(frontend_error("synthesis text is empty"));
        }
        let encoding = self
            .tokenizer
            .encode(normalized, true)
            .map_err(|error| frontend_error(error.to_string()))?;
        if encoding.len() < 3 || encoding.len() > 512 {
            return Err(frontend_error(format!(
                "BERT token count {} is outside 3..=512",
                encoding.len()
            )));
        }
        let tokens = encoding.get_tokens();
        let groups = wordpiece_groups(&tokens[1..tokens.len() - 1]);
        let mut phones = vec!["_".to_owned()];
        let mut tones = vec![0_i32];
        let mut word2phone = vec![1_usize];
        for group in groups {
            let word = group.tokens.concat();
            let (group_phones, group_tones) = self.pronounce(&word);
            word2phone.extend(distribute(group_phones.len(), group.tokens.len()));
            phones.extend(group_phones);
            tones.extend(group_tones);
        }
        phones.push("_".to_owned());
        tones.push(0);
        word2phone.push(1);
        if word2phone.len() != encoding.len() {
            return Err(frontend_error(format!(
                "BERT/phoneme alignment mismatch: {} tokens, {} groups",
                encoding.len(),
                word2phone.len()
            )));
        }

        let bert_ids = Array2::from_shape_vec(
            (1, encoding.len()),
            encoding.get_ids().iter().map(|id| *id as i32).collect(),
        )
        .map_err(|error| frontend_error(error.to_string()))?;
        let outputs = bert_session
            .run(ort::inputs![
                "input_ids" => Value::from_array(bert_ids).map_err(ort_error)?
            ])
            .map_err(ort_error)?;
        let (shape, features) = outputs["logits"]
            .try_extract_tensor::<f32>()
            .map_err(ort_error)?;
        if shape.as_ref() != [encoding.len() as i64, 768] {
            return Err(frontend_error(format!(
                "unexpected BERT output shape {shape:?}"
            )));
        }

        let mut phone_ids = phones
            .iter()
            .map(|phone| {
                self.symbol_ids
                    .get(phone)
                    .copied()
                    .or_else(|| self.symbol_ids.get("UNK").copied())
                    .ok_or_else(|| frontend_error(format!("unknown Melo symbol {phone:?}")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        // MeloTTS inserts blank symbol 0 between every phone and doubles the
        // token-aligned BERT repetition counts. The first token receives the
        // leading blank, matching MeloTTS `word2ph[0] += 1`.
        phone_ids = intersperse(phone_ids, 0);
        tones = intersperse(
            tones
                .into_iter()
                .map(|tone| tone + ENGLISH_TONE_OFFSET)
                .collect(),
            0,
        );
        let language_ids = intersperse(vec![ENGLISH_LANGUAGE_ID; phones.len()], 0);
        let mut expanded_bert = Vec::with_capacity(phone_ids.len() * 768);
        for (token_index, phone_count) in word2phone.iter().copied().enumerate() {
            let repeats = phone_count * 2 + usize::from(token_index == 0);
            let feature = &features[token_index * 768..(token_index + 1) * 768];
            for _ in 0..repeats {
                expanded_bert.extend(feature.iter().copied().map(f16::from_f32));
            }
        }
        if expanded_bert.len() != phone_ids.len() * 768 {
            return Err(frontend_error(
                "BERT feature expansion did not match blanked phones",
            ));
        }
        Ok(EnglishInputs {
            phone_ids,
            tones,
            language_ids,
            bert: transpose_phone_features(&expanded_bert),
        })
    }

    fn pronounce(&self, word: &str) -> (Vec<String>, Vec<i32>) {
        if is_supported_punctuation(word) {
            return (vec![normalize_punctuation(word).to_owned()], vec![0]);
        }
        let pronunciation = self
            .dictionary
            .get(&word.to_ascii_lowercase())
            .and_then(|values| values.first())
            .cloned()
            .or_else(|| spell_pronunciation(word, &self.dictionary));
        let Some(pronunciation) = pronunciation else {
            return (vec!["UNK".to_owned()], vec![0]);
        };
        let (phones, tones): (Vec<_>, Vec<_>) = pronunciation
            .split_whitespace()
            .map(|phone| {
                let tone = phone
                    .as_bytes()
                    .last()
                    .filter(|value| value.is_ascii_digit())
                    .map_or(0, |value| i32::from(value - b'0') + 1);
                let phoneme = phone.trim_end_matches(|value: char| value.is_ascii_digit());
                (phoneme.to_ascii_lowercase(), tone)
            })
            .unzip();
        (phones, tones)
    }
}

struct WordpieceGroup<'a> {
    tokens: Vec<&'a str>,
}

fn wordpiece_groups(tokens: &[String]) -> Vec<WordpieceGroup<'_>> {
    let mut groups: Vec<WordpieceGroup<'_>> = Vec::new();
    for token in tokens {
        if let Some(suffix) = token.strip_prefix("##") {
            if let Some(group) = groups.last_mut() {
                group.tokens.push(suffix);
            } else {
                groups.push(WordpieceGroup {
                    tokens: vec![suffix],
                });
            }
        } else {
            groups.push(WordpieceGroup {
                tokens: vec![token],
            });
        }
    }
    groups
}

fn distribute(items: usize, slots: usize) -> Vec<usize> {
    if slots == 0 {
        return Vec::new();
    }
    let base = items / slots;
    let remainder = items % slots;
    (0..slots)
        .map(|index| base + usize::from(index < remainder))
        .collect()
}

fn intersperse(values: Vec<i32>, blank: i32) -> Vec<i32> {
    let mut output = Vec::with_capacity(values.len() * 2 + 1);
    output.push(blank);
    for value in values {
        output.push(value);
        output.push(blank);
    }
    output
}

fn transpose_phone_features(features: &[f16]) -> Vec<f16> {
    let phones = features.len() / 768;
    let mut transposed = vec![f16::ZERO; features.len()];
    for phone in 0..phones {
        for channel in 0..768 {
            transposed[channel * phones + phone] = features[phone * 768 + channel];
        }
    }
    transposed
}

fn normalize_text(text: &str) -> String {
    let mut normalized = text
        .trim()
        .to_ascii_lowercase()
        .replace('“', "\"")
        .replace('”', "\"")
        .replace('’', "'")
        .replace('…', "...");
    normalized = expand_numbers_and_symbols(&normalized);
    for (abbreviation, expansion) in [
        ("mr.", "mister"),
        ("mrs.", "misses"),
        ("dr.", "doctor"),
        ("st.", "saint"),
    ] {
        normalized = normalized.replace(abbreviation, expansion);
    }
    normalized.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Normalizes constructs which CMUdict intentionally does not cover. The NGC
/// package contains a G2P graph, but does not publish its grapheme-index
/// contract; driving that graph with guessed indices would be less reliable
/// than the deterministic dictionary fallback used here.
fn expand_numbers_and_symbols(text: &str) -> String {
    let characters = text.chars().collect::<Vec<_>>();
    let mut output = String::with_capacity(text.len());
    let mut index = 0;
    while index < characters.len() {
        if characters[index] == '$'
            && index + 1 < characters.len()
            && characters[index + 1].is_ascii_digit()
            && let Some(number) = parse_numeric_token(&characters, index + 1)
        {
            push_spoken_phrase(&mut output, &currency_words(&number));
            index = number.end;
            if characters
                .get(index)
                .is_some_and(|character| character.is_ascii_alphanumeric())
            {
                output.push(' ');
            }
            continue;
        }
        if characters[index].is_ascii_digit()
            && let Some(number) = parse_numeric_token(&characters, index)
        {
            push_spoken_phrase(&mut output, &numeric_words(&number));
            index = number.end;
            if characters
                .get(index)
                .is_some_and(|character| character.is_ascii_alphanumeric())
            {
                output.push(' ');
            }
            continue;
        }
        let phrase = match characters[index] {
            '&' => Some("and"),
            '%' => Some("percent"),
            '+' => Some("plus"),
            '=' => Some("equals"),
            '@' => Some("at"),
            _ => None,
        };
        if let Some(phrase) = phrase {
            push_spoken_phrase(&mut output, phrase);
            output.push(' ');
        } else {
            output.push(characters[index]);
        }
        index += 1;
    }
    output
}

struct NumericToken {
    end: usize,
    integer_digits: String,
    fractional_digits: Option<String>,
    ordinal: bool,
}

fn parse_numeric_token(characters: &[char], start: usize) -> Option<NumericToken> {
    let mut end = start;
    let mut integer_digits = String::new();
    while let Some(character) = characters.get(end) {
        if character.is_ascii_digit() {
            integer_digits.push(*character);
            end += 1;
        } else if *character == ','
            && !integer_digits.is_empty()
            && characters
                .get(end + 1)
                .is_some_and(|next| next.is_ascii_digit())
        {
            end += 1;
        } else {
            break;
        }
    }
    if integer_digits.is_empty() {
        return None;
    }
    let fractional_digits = if characters.get(end) == Some(&'.')
        && characters
            .get(end + 1)
            .is_some_and(|next| next.is_ascii_digit())
    {
        end += 1;
        let mut digits = String::new();
        while characters
            .get(end)
            .is_some_and(|character| character.is_ascii_digit())
        {
            digits.push(characters[end]);
            end += 1;
        }
        Some(digits)
    } else {
        None
    };
    let ordinal = if fractional_digits.is_none() && end + 1 < characters.len() {
        let suffix = [characters[end], characters[end + 1]]
            .into_iter()
            .collect::<String>();
        if matches!(suffix.as_str(), "st" | "nd" | "rd" | "th") {
            end += 2;
            true
        } else {
            false
        }
    } else {
        false
    };
    Some(NumericToken {
        end,
        integer_digits,
        fractional_digits,
        ordinal,
    })
}

fn numeric_words(number: &NumericToken) -> String {
    let mut words = cardinal_digits(&number.integer_digits);
    if let Some(fractional) = &number.fractional_digits {
        words.push_str(" point");
        for digit in fractional.bytes() {
            words.push(' ');
            words.push_str(digit_word(digit - b'0'));
        }
    } else if number.ordinal {
        words = ordinal_words(words);
    }
    words
}

fn currency_words(number: &NumericToken) -> String {
    let dollars = number.integer_digits.parse::<u64>().unwrap_or(0);
    let mut words = cardinal_digits(&number.integer_digits);
    words.push_str(if dollars == 1 { " dollar" } else { " dollars" });
    if let Some(fractional) = &number.fractional_digits
        && fractional.len() <= 2
    {
        let cents = if fractional.len() == 1 {
            fractional.parse::<u64>().unwrap_or(0) * 10
        } else {
            fractional.parse::<u64>().unwrap_or(0)
        };
        if cents != 0 {
            words.push_str(" and ");
            words.push_str(&cardinal_number(cents));
            words.push_str(if cents == 1 { " cent" } else { " cents" });
        }
    }
    words
}

fn cardinal_digits(digits: &str) -> String {
    if digits.len() > 1 && digits.starts_with('0') {
        return digits
            .bytes()
            .map(|digit| digit_word(digit - b'0'))
            .collect::<Vec<_>>()
            .join(" ");
    }
    digits.parse::<u64>().map_or_else(
        |_| {
            digits
                .bytes()
                .map(|digit| digit_word(digit - b'0'))
                .collect::<Vec<_>>()
                .join(" ")
        },
        cardinal_number,
    )
}

fn cardinal_number(value: u64) -> String {
    if value < 20 {
        return [
            "zero",
            "one",
            "two",
            "three",
            "four",
            "five",
            "six",
            "seven",
            "eight",
            "nine",
            "ten",
            "eleven",
            "twelve",
            "thirteen",
            "fourteen",
            "fifteen",
            "sixteen",
            "seventeen",
            "eighteen",
            "nineteen",
        ][value as usize]
            .to_owned();
    }
    if value < 100 {
        let tens = [
            "", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
        ][(value / 10) as usize];
        return if value % 10 == 0 {
            tens.to_owned()
        } else {
            format!("{tens} {}", cardinal_number(value % 10))
        };
    }
    for (scale, name) in [
        (1_000_000_000_000_000_000, "quintillion"),
        (1_000_000_000_000_000, "quadrillion"),
        (1_000_000_000_000, "trillion"),
        (1_000_000_000, "billion"),
        (1_000_000, "million"),
        (1_000, "thousand"),
        (100, "hundred"),
    ] {
        if value >= scale {
            let remainder = value % scale;
            let prefix = format!("{} {name}", cardinal_number(value / scale));
            return if remainder == 0 {
                prefix
            } else {
                format!("{prefix} {}", cardinal_number(remainder))
            };
        }
    }
    unreachable!("values below one hundred returned above")
}

fn ordinal_words(cardinal: String) -> String {
    let mut words = cardinal
        .split_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let Some(last) = words.last_mut() else {
        return cardinal;
    };
    *last = match last.as_str() {
        "one" => "first".to_owned(),
        "two" => "second".to_owned(),
        "three" => "third".to_owned(),
        "five" => "fifth".to_owned(),
        "eight" => "eighth".to_owned(),
        "nine" => "ninth".to_owned(),
        "twelve" => "twelfth".to_owned(),
        value if value.ends_with('y') => format!("{}ieth", &value[..value.len() - 1]),
        value => format!("{value}th"),
    };
    words.join(" ")
}

fn digit_word(digit: u8) -> &'static str {
    [
        "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
    ][usize::from(digit)]
}

fn push_spoken_phrase(output: &mut String, phrase: &str) {
    if output.chars().last().is_some_and(|character| {
        !character.is_whitespace() && !matches!(character, '(' | '[' | '{')
    }) {
        output.push(' ');
    }
    output.push_str(phrase);
}

fn is_supported_punctuation(value: &str) -> bool {
    matches!(value, "!" | "?" | "," | "." | "'" | "-" | ";" | ":")
}

fn normalize_punctuation(value: &str) -> &str {
    match value {
        ";" | ":" => ",",
        value => value,
    }
}

fn spell_pronunciation(word: &str, dictionary: &HashMap<String, Vec<String>>) -> Option<String> {
    let mut parts = Vec::new();
    for character in word
        .chars()
        .filter(|character| character.is_ascii_alphabetic())
    {
        let key = character.to_ascii_lowercase().to_string();
        parts.push(dictionary.get(&key)?.first()?.clone());
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

fn ort_error(error: ort::Error) -> InferenceError {
    frontend_error(error.to_string())
}

fn frontend_error(message: impl Into<String>) -> InferenceError {
    InferenceError::InvalidConfiguration {
        field: "tts.providers.openvoice.frontend",
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distributes_phone_features_without_losing_alignment() {
        assert_eq!(distribute(5, 2), vec![3, 2]);
        assert_eq!(distribute(2, 4), vec![1, 1, 0, 0]);
    }

    #[test]
    fn wordpieces_reconstruct_dictionary_words() {
        let tokens = vec!["x".to_owned(), "##r".to_owned(), ",".to_owned()];
        let groups = wordpiece_groups(&tokens);
        assert_eq!(groups[0].tokens.concat(), "xr");
        assert_eq!(groups[1].tokens.concat(), ",");
    }

    #[test]
    fn normalizes_numbers_ordinals_currency_and_common_symbols() {
        assert_eq!(
            normalize_text("Version 2.8 costs $12.50 on the 21st & is 100% native."),
            "version two point eight costs twelve dollars and fifty cents on the twenty first and is one hundred percent native."
        );
    }

    #[test]
    fn preserves_leading_zeroes_as_spoken_digits() {
        assert_eq!(normalize_text("Room 007"), "room zero zero seven");
    }
}
