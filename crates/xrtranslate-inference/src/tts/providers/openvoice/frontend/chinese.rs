//! Chinese-with-English-code-switch MeloTTS frontend.

use std::{collections::HashMap, path::Path};

use half::f16;
use ndarray::Array2;
use ort::{session::Session, value::Value};
use serde::Deserialize;

use crate::InferenceError;

use super::{
    MeloInputs, distribute, english::EnglishFrontend, intersperse, transpose_phone_features,
};

const CHINESE_LANGUAGE_ID: i32 = 3;
const ENGLISH_TONE_OFFSET: i32 = 7;

#[derive(Deserialize)]
struct ChineseLexicon {
    characters: HashMap<String, String>,
    phrases: HashMap<String, Vec<String>>,
}

pub(in crate::tts::providers::openvoice) struct ChineseFrontend {
    resources: EnglishFrontend,
    lexicon: ChineseLexicon,
    max_phrase_chars: usize,
    pinyin_to_phones: HashMap<String, Vec<String>>,
}

impl ChineseFrontend {
    pub(super) fn load(model_dir: &Path, symbols: &[String]) -> Result<Self, InferenceError> {
        let resources = EnglishFrontend::load(model_dir, symbols)?;
        let lexicon: ChineseLexicon = serde_json::from_slice(
            &std::fs::read(model_dir.join("frontend/chinese_lexicon.json"))
                .map_err(|error| frontend_error(error.to_string()))?,
        )
        .map_err(|error| frontend_error(error.to_string()))?;
        let max_phrase_chars = lexicon
            .phrases
            .keys()
            .map(|phrase| phrase.chars().count())
            .max()
            .unwrap_or(1);
        let mapping = std::fs::read_to_string(model_dir.join("frontend/opencpop-strict.txt"))
            .map_err(|error| frontend_error(error.to_string()))?;
        let pinyin_to_phones = mapping
            .lines()
            .filter_map(|line| {
                let (pinyin, phones) = line.split_once('\t')?;
                Some((
                    pinyin.to_owned(),
                    phones.split_whitespace().map(str::to_owned).collect(),
                ))
            })
            .collect();
        Ok(Self {
            resources,
            lexicon,
            max_phrase_chars,
            pinyin_to_phones,
        })
    }

    pub(super) fn encode(
        &self,
        bert_session: &mut Session,
        text: &str,
    ) -> Result<MeloInputs, InferenceError> {
        let normalized = normalize_text(text);
        if normalized.is_empty() {
            return Err(frontend_error("synthesis text is empty"));
        }
        let encoding = self
            .resources
            .tokenizer()
            .encode(normalized, true)
            .map_err(|error| frontend_error(error.to_string()))?;
        if encoding.len() < 3 || encoding.len() > 512 {
            return Err(frontend_error(format!(
                "BERT token count {} is outside 3..=512",
                encoding.len()
            )));
        }

        let tokens = encoding.get_tokens();
        let inner = &tokens[1..tokens.len() - 1];
        let mut phones = vec!["_".to_owned()];
        let mut tones = vec![0_i32];
        let mut word2phone = vec![1_usize];
        let mut cursor = 0;
        while cursor < inner.len() {
            if is_han_token(&inner[cursor]) {
                let begin = cursor;
                while cursor < inner.len() && is_han_token(&inner[cursor]) {
                    cursor += 1;
                }
                let characters = inner[begin..cursor]
                    .iter()
                    .filter_map(|token| token.chars().next())
                    .collect::<Vec<_>>();
                let syllables = self.pronounce_characters(&characters)?;
                for syllable in syllables {
                    let (base, tone) = split_tone(&syllable)?;
                    let mapped = self.pinyin_to_phones.get(base).ok_or_else(|| {
                        frontend_error(format!("no OpenCPOP mapping for pinyin {base:?}"))
                    })?;
                    word2phone.push(mapped.len());
                    phones.extend(mapped.iter().cloned());
                    tones.extend(std::iter::repeat_n(tone, mapped.len()));
                }
                continue;
            }

            let begin = cursor;
            cursor += 1;
            while cursor < inner.len() && inner[cursor].starts_with("##") {
                cursor += 1;
            }
            let group = &inner[begin..cursor];
            let word = group
                .iter()
                .enumerate()
                .map(|(index, token)| {
                    if index == 0 {
                        token.as_str()
                    } else {
                        token.strip_prefix("##").unwrap_or(token)
                    }
                })
                .collect::<String>();
            let (group_phones, group_tones) = self.resources.pronounce(&word);
            word2phone.extend(distribute(group_phones.len(), group.len()));
            phones.extend(group_phones);
            tones.extend(
                group_tones
                    .into_iter()
                    .map(|tone| tone + ENGLISH_TONE_OFFSET),
            );
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
            .try_extract_tensor::<f16>()
            .map_err(ort_error)?;
        if shape.as_ref() != [encoding.len() as i64, 768] {
            return Err(frontend_error(format!(
                "unexpected multilingual BERT output shape {shape:?}"
            )));
        }

        let mut phone_ids = phones
            .iter()
            .map(|phone| self.resources.phone_id(phone))
            .collect::<Result<Vec<_>, _>>()?;
        phone_ids = intersperse(phone_ids, 0);
        tones = intersperse(tones, 0);
        let language_ids = intersperse(vec![CHINESE_LANGUAGE_ID; phones.len()], 0);
        let mut expanded_bert = Vec::with_capacity(phone_ids.len() * 768);
        for (token_index, phone_count) in word2phone.iter().copied().enumerate() {
            let repeats = phone_count * 2 + usize::from(token_index == 0);
            let feature = &features[token_index * 768..(token_index + 1) * 768];
            for _ in 0..repeats {
                expanded_bert.extend_from_slice(feature);
            }
        }
        if expanded_bert.len() != phone_ids.len() * 768 {
            return Err(frontend_error(
                "BERT feature expansion did not match blanked phones",
            ));
        }
        Ok(MeloInputs {
            phone_ids,
            tones,
            language_ids,
            bert: transpose_phone_features(&expanded_bert),
        })
    }

    fn pronounce_characters(&self, characters: &[char]) -> Result<Vec<String>, InferenceError> {
        let mut result = Vec::with_capacity(characters.len());
        let mut cursor = 0;
        while cursor < characters.len() {
            let remaining = characters.len() - cursor;
            let mut matched = None;
            for length in (2..=remaining.min(self.max_phrase_chars)).rev() {
                let phrase = characters[cursor..cursor + length]
                    .iter()
                    .collect::<String>();
                if let Some(syllables) = self.lexicon.phrases.get(&phrase) {
                    matched = Some((length, syllables.clone()));
                    break;
                }
            }
            if let Some((length, syllables)) = matched {
                result.extend(syllables);
                cursor += length;
            } else {
                let character = characters[cursor].to_string();
                result.push(
                    self.lexicon
                        .characters
                        .get(&character)
                        .cloned()
                        .ok_or_else(|| {
                            frontend_error(format!(
                                "Chinese pronunciation is unavailable for {character:?}"
                            ))
                        })?,
                );
                cursor += 1;
            }
        }
        apply_tone_sandhi(characters, &mut result)?;
        Ok(result)
    }
}

fn is_han_token(token: &str) -> bool {
    let mut characters = token.chars();
    matches!(
        (characters.next(), characters.next()),
        (Some(character), None) if matches!(character as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF)
    )
}

fn split_tone(syllable: &str) -> Result<(&str, i32), InferenceError> {
    let (base, tone) = syllable.split_at(syllable.len().saturating_sub(1));
    let tone = tone
        .parse::<i32>()
        .ok()
        .filter(|tone| (1..=5).contains(tone))
        .ok_or_else(|| frontend_error(format!("invalid numbered pinyin {syllable:?}")))?;
    Ok((base, tone))
}

fn apply_tone_sandhi(characters: &[char], syllables: &mut [String]) -> Result<(), InferenceError> {
    let mut tones = syllables
        .iter()
        .map(|syllable| split_tone(syllable).map(|(_, tone)| tone))
        .collect::<Result<Vec<_>, _>>()?;
    for index in 0..tones.len().saturating_sub(1) {
        match characters[index] {
            '不' if tones[index + 1] == 4 => tones[index] = 2,
            '一' if is_numeric_yi(characters, index)
                || index > 0 && characters[index - 1] == '第' => {}
            '一' if tones[index + 1] == 4 => tones[index] = 2,
            '一' if matches!(tones[index + 1], 1..=3) => tones[index] = 4,
            _ => {}
        }
    }

    // Upstream uses jieba word boundaries to distinguish the 2+1 and 1+2
    // readings of three-or-more consecutive third tones. Without that lexical
    // information only a run of exactly two third tones is unambiguous.
    let mut begin = 0;
    while begin < tones.len() {
        if tones[begin] != 3 {
            begin += 1;
            continue;
        }
        let mut end = begin + 1;
        while end < tones.len() && tones[end] == 3 {
            end += 1;
        }
        if end - begin == 2 {
            tones[begin] = 2;
        }
        begin = end;
    }
    for (syllable, tone) in syllables.iter_mut().zip(tones) {
        syllable.pop();
        syllable.push(char::from_digit(tone as u32, 10).expect("tone is 1..=5"));
    }
    Ok(())
}

fn is_numeric_yi(characters: &[char], index: usize) -> bool {
    characters[index] == '一'
        && (index > 0 && is_chinese_number_character(characters[index - 1])
            || characters
                .get(index + 1)
                .is_some_and(|character| is_chinese_number_character(*character)))
}

fn is_chinese_number_character(character: char) -> bool {
    matches!(
        character,
        '零' | '〇'
            | '一'
            | '二'
            | '两'
            | '三'
            | '四'
            | '五'
            | '六'
            | '七'
            | '八'
            | '九'
            | '十'
            | '百'
            | '千'
            | '万'
            | '亿'
            | '兆'
            | '点'
    )
}

fn normalize_text(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let characters = text.trim().chars().collect::<Vec<_>>();
    let mut index = 0;
    while index < characters.len() {
        if characters[index].is_ascii_digit() {
            let integer_begin = index;
            while index < characters.len() && characters[index].is_ascii_digit() {
                index += 1;
            }
            output.push_str(&integer_to_chinese(&characters[integer_begin..index]));
            if index + 1 < characters.len()
                && characters[index] == '.'
                && characters[index + 1].is_ascii_digit()
            {
                output.push('点');
                index += 1;
                while index < characters.len() && characters[index].is_ascii_digit() {
                    output.push(chinese_digit(characters[index]));
                    index += 1;
                }
            }
            continue;
        }
        let normalized = match characters[index] {
            '：' | '；' | '，' | '、' | '·' => ',',
            '。' => '.',
            '！' => '!',
            '？' => '?',
            '“' | '”' | '‘' | '’' | '（' | '）' | '(' | ')' | '《' | '》' | '【' | '】' | '['
            | ']' => '\'',
            '—' | '～' | '~' => '-',
            character => character,
        };
        if normalized.is_alphanumeric()
            || matches!(normalized, '!' | '?' | '…' | ',' | '.' | '\'' | '-' | ' ')
            || is_han_token(&normalized.to_string())
        {
            output.push(normalized);
        }
        index += 1;
    }
    output.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn integer_to_chinese(digits: &[char]) -> String {
    let first_nonzero = digits.iter().position(|digit| *digit != '0');
    let Some(first_nonzero) = first_nonzero else {
        return "零".to_owned();
    };
    let digits = &digits[first_nonzero..];
    // 兆 is ample for ordinary synthesis text. Very long identifiers are
    // safer read digit-by-digit than assigned a guessed higher place value.
    if digits.len() > 16 {
        return digits.iter().copied().map(chinese_digit).collect();
    }

    let value = digits.iter().fold(0_u64, |value, digit| {
        value * 10 + digit.to_digit(10).expect("ASCII digit") as u64
    });
    let mut groups = Vec::new();
    let mut remaining = value;
    while remaining > 0 {
        groups.push((remaining % 10_000) as u16);
        remaining /= 10_000;
    }

    const GROUP_UNITS: [&str; 4] = ["", "万", "亿", "兆"];
    let mut output = String::new();
    let mut zero_between_groups = false;
    for group_index in (0..groups.len()).rev() {
        let group = groups[group_index];
        if group == 0 {
            if !output.is_empty() {
                zero_between_groups = true;
            }
            continue;
        }
        if !output.is_empty() && (zero_between_groups || group < 1_000) {
            output.push('零');
        }
        output.push_str(&four_digit_group(
            group,
            output.is_empty() && (10..20).contains(&group),
        ));
        output.push_str(GROUP_UNITS[group_index]);
        zero_between_groups = false;
    }
    output
}

fn four_digit_group(value: u16, omit_leading_one: bool) -> String {
    const PLACES: [(u16, &str); 4] = [(1_000, "千"), (100, "百"), (10, "十"), (1, "")];
    let mut output = String::new();
    let mut zero_before_next_digit = false;
    for (place, unit) in PLACES {
        let digit = value / place % 10;
        if digit == 0 {
            if !output.is_empty() && value % place != 0 {
                zero_before_next_digit = true;
            }
            continue;
        }
        if zero_before_next_digit {
            output.push('零');
            zero_before_next_digit = false;
        }
        if !(omit_leading_one && place == 10 && output.is_empty() && digit == 1) {
            output.push(chinese_digit(
                char::from_digit(digit as u32, 10).expect("digit is 1..=9"),
            ));
        }
        output.push_str(unit);
    }
    output
}

fn chinese_digit(digit: char) -> char {
    match digit {
        '0' => '零',
        '1' => '一',
        '2' => '二',
        '3' => '三',
        '4' => '四',
        '5' => '五',
        '6' => '六',
        '7' => '七',
        '8' => '八',
        '9' => '九',
        _ => unreachable!("caller supplies an ASCII digit"),
    }
}

fn ort_error(error: ort::Error) -> InferenceError {
    frontend_error(error.to_string())
}

fn frontend_error(message: impl Into<String>) -> InferenceError {
    InferenceError::InvalidConfiguration {
        field: "tts.providers.openvoice.frontend.chinese",
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chinese_normalization_preserves_code_switching() {
        assert_eq!(normalize_text("你好，OpenVoice 2！"), "你好,OpenVoice 二!");
    }

    #[test]
    fn chinese_normalization_uses_place_values_and_decimal_digits() {
        assert_eq!(normalize_text("12 和 2.8"), "十二 和 二点八");
        assert_eq!(
            normalize_text("10, 20, 101, 2026"),
            "十, 二十, 一百零一, 二千零二十六"
        );
        assert_eq!(normalize_text("0.05"), "零点零五");
    }

    #[test]
    fn tone_sandhi_distinguishes_bu_yi_and_third_tones() {
        let characters = "不是一段很好".chars().collect::<Vec<_>>();
        let mut syllables = vec!["bu4", "shi4", "yi1", "duan4", "hen3", "hao3"]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        apply_tone_sandhi(&characters, &mut syllables).unwrap();
        assert_eq!(syllables, ["bu2", "shi4", "yi2", "duan4", "hen2", "hao3"]);

        let characters = "一天".chars().collect::<Vec<_>>();
        let mut syllables = ["yi1", "tian1"].map(str::to_owned);
        apply_tone_sandhi(&characters, &mut syllables).unwrap();
        assert_eq!(syllables, ["yi4", "tian1"]);
    }

    #[test]
    fn yi_sandhi_preserves_numbers_and_ordinals() {
        let characters = "一百零二".chars().collect::<Vec<_>>();
        let mut syllables = ["yi1", "bai3", "ling2", "er4"].map(str::to_owned);
        apply_tone_sandhi(&characters, &mut syllables).unwrap();
        assert_eq!(syllables, ["yi1", "bai3", "ling2", "er4"]);

        let characters = "第一名".chars().collect::<Vec<_>>();
        let mut syllables = ["di4", "yi1", "ming2"].map(str::to_owned);
        apply_tone_sandhi(&characters, &mut syllables).unwrap();
        assert_eq!(syllables, ["di4", "yi1", "ming2"]);
    }

    #[test]
    fn third_tone_triplets_remain_conservative_without_word_segmentation() {
        let characters = "纸老虎".chars().collect::<Vec<_>>();
        let mut syllables = ["zhi3", "lao3", "hu3"].map(str::to_owned);
        apply_tone_sandhi(&characters, &mut syllables).unwrap();
        assert_eq!(syllables, ["zhi3", "lao3", "hu3"]);
    }
}
