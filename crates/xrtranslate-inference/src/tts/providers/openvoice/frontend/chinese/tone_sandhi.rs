//! Mandarin word boundaries, neutral tones and contextual tone changes.
//!
//! Adapted from MeloTTS 209145371cff8fc3bd60d7be902ea69cbdb7965a,
//! melo/text/tone_sandhi.py (including neutral_words.txt).
//! Copyright (c) 2021 PaddlePaddle Authors. All Rights Reserved.
//! SPDX-License-Identifier: Apache-2.0
//! License: https://www.apache.org/licenses/LICENSE-2.0

use std::{collections::HashSet, sync::LazyLock};

use jieba_rs::{Jieba, Tag};

use super::{InferenceError, is_chinese_number_character, split_tone};

static NEUTRAL_WORDS: LazyLock<HashSet<&str>> = LazyLock::new(|| {
    include_str!("neutral_words.txt")
        .split_whitespace()
        .collect()
});

#[derive(Default)]
pub(super) struct ToneSandhi {
    segmenter: Jieba,
}

impl ToneSandhi {
    pub(super) fn apply(
        &self,
        characters: &[char],
        syllables: &mut [String],
    ) -> Result<(), InferenceError> {
        let text = characters.iter().collect::<String>();
        let mut tones = syllables
            .iter()
            .map(|syllable| split_tone(syllable).map(|(_, tone)| tone))
            .collect::<Result<Vec<_>, _>>()?;
        let mut words = self.segmenter.tag(&text, true);

        // Keep grammatical groups together before changing their tones.
        words = merge_words(&text, words, |left, _| left.word == "不", false);
        let mut index = 1;
        while index + 1 < words.len() {
            if words[index].word == "一"
                && words[index - 1].word == words[index + 1].word
                && words[index - 1].tag == "v"
            {
                let right = words.remove(index + 1);
                words.remove(index);
                join(&text, &mut words[index - 1], &right);
            } else {
                index += 1;
            }
        }
        words = merge_words(&text, words, |left, _| left.word == "一", false);
        words = merge_words(&text, words, |left, right| left.word == right.word, false);
        for all_third in [true, false] {
            words = merge_words(
                &text,
                words,
                |left, right| {
                    right.end - left.start <= 3
                        && !is_repeated(left.word)
                        && if all_third {
                            tones[left.start..right.end].iter().all(|tone| *tone == 3)
                        } else {
                            tones[left.end - 1] == 3 && tones[right.start] == 3
                        }
                },
                true,
            );
        }
        words = merge_words(&text, words, |_, right| right.word == "儿", false);

        for word in words {
            let chars = &characters[word.start..word.end];
            let tones = &mut tones[word.start..word.end];
            for index in 0..tones.len() {
                let next = tones.get(index + 1).copied();
                match chars[index] {
                    '不' if chars.len() == 3 && index == 1 => tones[index] = 5,
                    '不' if next == Some(4) => tones[index] = 2,
                    '一' if chars.iter().all(|ch| is_chinese_number_character(*ch)) => {}
                    '一' if chars.len() == 3 && index == 1 && chars[0] == chars[2] => {
                        tones[index] = 5;
                    }
                    '一' if index == 1 && chars[0] == '第' => tones[index] = 1,
                    '一' if next == Some(4) => tones[index] = 2,
                    '一' if next.is_some() => tones[index] = 4,
                    _ => {}
                }
            }
            let split = self.split(word.word);
            Self::neutral_tones(word.word, word.tag, chars, tones, split);
            Self::third_tones(tones, word.word[..split].chars().count());
        }
        for (syllable, tone) in syllables.iter_mut().zip(tones) {
            syllable.pop();
            syllable.push(char::from_digit(tone as u32, 10).expect("tone is 1..=5"));
        }
        Ok(())
    }

    fn neutral_tones(word: &str, pos: &str, chars: &[char], tones: &mut [i32], split: usize) {
        let exception = matches!(
            word,
            "男子"
                | "女子"
                | "分子"
                | "原子"
                | "量子"
                | "莲子"
                | "石子"
                | "瓜子"
                | "电子"
                | "人人"
                | "虎虎"
        );
        for index in 1..chars.len() {
            if chars[index] == chars[index - 1] && pos.starts_with(['n', 'v', 'a']) && !exception {
                tones[index] = 5;
            }
        }
        let last = chars.len() - 1;
        if "吧呢啊呐噻嘛吖嗨哦哒额滴哩哟喽啰耶喔诶的地得".contains(chars[last])
            || (last > 0 && "们子".contains(chars[last]) && matches!(pos, "r" | "n") && !exception)
            || (last > 0 && "上下里".contains(chars[last]) && matches!(pos, "s" | "l" | "f"))
            || (last > 0
                && "来去".contains(chars[last])
                && "上下进出回过起开".contains(chars[last - 1]))
        {
            tones[last] = 5;
        } else if let Some(index) = chars.iter().position(|ch| *ch == '个').filter(|index| {
            chars.len() == 1
                || *index > 0
                    && (is_chinese_number_character(chars[*index - 1])
                        || "几有两半多各整每做是".contains(chars[*index - 1]))
        }) {
            tones[index] = 5;
        } else if is_neutral(word) {
            tones[last] = 5;
        }
        if split > 0 && is_neutral(&word[..split]) {
            tones[word[..split].chars().count() - 1] = 5;
        }
        if split < word.len() && is_neutral(&word[split..]) {
            tones[last] = 5;
        }
    }

    fn split(&self, word: &str) -> usize {
        let part = self
            .segmenter
            .cut_for_search(word, true)
            .into_iter()
            .min_by_key(|part| part.word.chars().count())
            .expect("nonempty Chinese word");
        if word.starts_with(part.word) {
            part.word.len()
        } else {
            word.len() - part.word.len()
        }
    }

    fn third_tones(tones: &mut [i32], split: usize) {
        match tones.len() {
            2 => third_pair(tones),
            3 => {
                if tones.iter().all(|tone| *tone == 3) {
                    if matches!(split, 1 | 2) {
                        tones[1] = 2;
                    }
                    if split == 2 {
                        tones[0] = 2;
                    }
                } else {
                    let (left, right) = tones.split_at_mut(split);
                    third_pair(left);
                    if right.len() == 2 && right == [3, 3] {
                        right[0] = 2;
                    } else if left.last() == Some(&3) && right.first() == Some(&3) {
                        *left.last_mut().unwrap() = 2;
                    }
                }
            }
            4 => tones.chunks_mut(2).for_each(third_pair),
            _ => {}
        }
    }
}

fn third_pair(tones: &mut [i32]) {
    if tones == [3, 3] {
        tones[0] = 2;
    }
}

fn is_repeated(word: &str) -> bool {
    let mut chars = word.chars();
    chars.next() == chars.next() && chars.next().is_none()
}

fn is_neutral(word: &str) -> bool {
    NEUTRAL_WORDS.contains(word)
        || word
            .char_indices()
            .rev()
            .nth(1)
            .is_some_and(|(index, _)| NEUTRAL_WORDS.contains(&word[index..]))
}

fn join<'a>(text: &'a str, left: &mut Tag<'a>, right: &Tag<'a>) {
    left.end = right.end;
    left.byte_end = right.byte_end;
    left.word = &text[left.byte_start..left.byte_end];
}

fn merge_words<'a>(
    text: &'a str,
    words: Vec<Tag<'a>>,
    merge: impl Fn(&Tag<'a>, &Tag<'a>) -> bool,
    pairs_only: bool,
) -> Vec<Tag<'a>> {
    let mut result: Vec<Tag<'a>> = Vec::with_capacity(words.len());
    let mut merged = false;
    for word in words {
        if let Some(left) = result.last_mut()
            && !(pairs_only && merged)
            && merge(left, &word)
        {
            if left.word == "不" {
                left.tag = word.tag;
            }
            join(text, left, &word);
            merged = true;
        } else {
            result.push(word);
            merged = false;
        }
    }
    result
}
