//! Deliberate, complete voice commands; ordinary conversation remains conversation.
use crate::i18n::UiLanguage;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Command {
    HideSubtitles,
    ShowSubtitles,
    ComeHere,
    ReturnHome,
}

impl Command {
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let normalized = text
            .trim_end_matches(|c: char| {
                c.is_whitespace() || matches!(c, '.' | '!' | '?' | '。' | '！' | '？' | '…')
            })
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        [
            Self::HideSubtitles,
            Self::ShowSubtitles,
            Self::ComeHere,
            Self::ReturnHome,
        ]
        .into_iter()
        .find(|command| command.words().0.contains(&normalized.as_str()))
    }

    pub(crate) fn reply(self, language: UiLanguage) -> &'static str {
        self.words().1[match language {
            UiLanguage::English => 0,
            UiLanguage::Chinese => 1,
            UiLanguage::Japanese => 2,
            UiLanguage::Korean => 3,
            UiLanguage::Russian => 4,
        }]
    }

    // Aliases are language-independent; acknowledgements follow the UI language.
    fn words(self) -> (&'static [&'static str], [&'static str; 5]) {
        match self {
            Self::HideSubtitles => (
                &[
                    "hide subtitles",
                    "hide subtitle",
                    "close subtitles",
                    "close subtitle",
                    "turn off subtitles",
                    "turn off subtitle",
                    "关闭字幕",
                    "隐藏字幕",
                    "字幕を消して",
                    "字幕を非表示",
                    "자막 숨겨 줘",
                    "자막 숨겨줘",
                    "자막 꺼 줘",
                    "자막 꺼줘",
                    "скрой субтитры",
                    "выключи субтитры",
                ],
                [
                    "Subtitles hidden.",
                    "好，字幕已收起。",
                    "字幕を隠したよ。",
                    "자막을 숨겼어.",
                    "Субтитры скрыты.",
                ],
            ),
            Self::ShowSubtitles => (
                &[
                    "show subtitles",
                    "show subtitle",
                    "open subtitles",
                    "open subtitle",
                    "turn on subtitles",
                    "turn on subtitle",
                    "打开字幕",
                    "显示字幕",
                    "字幕を表示して",
                    "字幕を表示",
                    "자막 보여 줘",
                    "자막 보여줘",
                    "자막 켜 줘",
                    "자막 켜줘",
                    "покажи субтитры",
                    "включи субтитры",
                ],
                [
                    "Subtitles are back.",
                    "好，字幕已打开。",
                    "字幕を表示したよ。",
                    "자막을 켰어.",
                    "Субтитры включены.",
                ],
            ),
            Self::ComeHere => (
                &[
                    "come here",
                    "come over here",
                    "过来一下",
                    "到我这里来",
                    "过来这里",
                    "こっちに来て",
                    "こっちにおいで",
                    "이리 와",
                    "이리 와 줘",
                    "подойди сюда",
                    "иди сюда",
                ],
                ["Coming!", "来啦！", "今行くね！", "지금 갈게!", "Уже иду!"],
            ),
            Self::ReturnHome => (
                &[
                    "go back to your spot",
                    "return to your spot",
                    "回到原位",
                    "回原位",
                    "元の位置に戻って",
                    "원래 자리로 돌아가",
                    "вернись на место",
                ],
                [
                    "Back to my spot.",
                    "好，我回原位啦。",
                    "元の場所に戻るね。",
                    "원래 자리로 돌아갈게.",
                    "Возвращаюсь на место.",
                ],
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_accept_all_supported_languages_and_asr_formatting() {
        for command in [
            Command::HideSubtitles,
            Command::ShowSubtitles,
            Command::ComeHere,
            Command::ReturnHome,
        ] {
            for alias in command.words().0 {
                assert_eq!(Command::parse(alias), Some(command));
                assert_eq!(
                    Command::parse(&format!("  {}！ \n", alias.to_uppercase())),
                    Some(command)
                );
            }
            for language in UiLanguage::ALL {
                assert!(!command.reply(language).is_empty());
            }
        }
        assert_eq!(Command::parse("Come\t  here..."), Some(Command::ComeHere));
    }

    #[test]
    fn quoted_negated_and_embedded_phrases_are_not_commands() {
        for text in [
            "",
            "...",
            "hello",
            "你好",
            "谢谢",
            "don't hide subtitles",
            "Can you explain how to hide subtitles?",
            "He said come here.",
            "hide, subtitles",
            "\"hide subtitles\"",
            "“关闭字幕”",
            "不要关闭字幕",
            "关闭字幕以后怎么打开",
            "过来一下，我有话说",
            "Come here; show subtitles",
        ] {
            assert_eq!(Command::parse(text), None, "{text:?}");
        }
    }
}
