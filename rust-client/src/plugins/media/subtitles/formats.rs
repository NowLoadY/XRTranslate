//! Subtitle text and cue identity survive format conversion.
use super::{SubtitleCue, SubtitleMetadata, SubtitleTimeline, normalize_cue_text};
use serde::{Deserialize, Serialize};
use std::fmt::Write;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct CueOrigin {
    label: String,
    settings: String,
}

impl CueOrigin {
    pub(super) fn for_split(&self, id: &str) -> Self {
        Self {
            label: if self.label.is_empty() {
                String::new()
            } else {
                id.into()
            },
            settings: self.settings.clone(),
        }
    }
}

impl SubtitleTimeline {
    pub(crate) fn parse(content: &str) -> Result<Self, String> {
        let content = content
            .trim_start_matches('\u{feff}')
            .replace("\r\n", "\n")
            .replace('\r', "\n");
        let mut timeline = Self::new();
        let lines: Vec<_> = content.lines().collect();
        for lines in lines
            .split(|line| line.trim().is_empty())
            .filter(|lines| !lines.is_empty())
        {
            if matches!(
                lines.first().and_then(|s| s.split_whitespace().next()),
                Some("WEBVTT" | "NOTE" | "STYLE" | "REGION")
            ) {
                timeline.vtt_prelude.push(lines.join("\n"));
                continue;
            }
            let time_index = lines
                .iter()
                .take(2)
                .position(|line| line.contains("-->"))
                .ok_or_else(|| format!("Invalid subtitle cue: {}", timeline.count() + 1))?;
            let (start, rest) = lines[time_index].split_once("-->").unwrap();
            let rest = rest.trim();
            let (end, settings) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            let (start_ms, end_ms) = (parse_time(start.trim())?, parse_time(end)?);
            if end_ms < start_ms {
                return Err("Subtitle end precedes its start".into());
            }
            let text = normalize_cue_text(&lines[time_index + 1..].join("\n"));
            if text.trim().is_empty() {
                return Err("Subtitle cue contains no text".into());
            }
            let id = format!("srt_{}", timeline.count());
            timeline.origins.insert(
                id.clone(),
                CueOrigin {
                    label: if time_index == 1 {
                        lines[0].trim().into()
                    } else {
                        String::new()
                    },
                    settings: settings.trim().into(),
                },
            );
            timeline
                .metadata
                .insert(id.clone(), SubtitleMetadata::authored());
            timeline.cues.push(SubtitleCue {
                id,
                start_ms,
                end_ms,
                speaker_name: None,
                original_text: text,
                translated_text: None,
                source_language: String::new(),
                target_language: String::new(),
            });
        }
        if timeline.count() == 0 {
            return Err("No subtitle cues found".into());
        }
        timeline.cues.sort_by_key(|cue| cue.start_ms);
        timeline.revision = 1;
        Ok(timeline)
    }

    pub fn export_srt(&self) -> String {
        self.export_timed(false, false)
    }
    pub fn export_vtt(&self) -> String {
        self.export_timed(true, false)
    }

    #[cfg(all(feature = "mpv", any(windows, target_os = "linux")))]
    pub(crate) fn export_playback_srt(&self) -> String {
        self.export_timed(false, true)
    }

    fn export_timed(&self, vtt: bool, playback: bool) -> String {
        let mut output = if vtt {
            if self.vtt_prelude.is_empty() {
                "WEBVTT\n\n".into()
            } else {
                format!("{}\n\n", self.vtt_prelude.join("\n\n"))
            }
        } else {
            String::new()
        };
        let mut exported = 0;
        for (index, cue) in self.cues.iter().enumerate() {
            // Keep the export boundary safe for legacy or externally supplied
            // cues too: a blank line ends a cue in SRT/VTT.
            let text = if playback {
                normalize_cue_text(&cue.playback_text())
            } else {
                cue.export_text()
            };
            if text.is_empty() {
                continue;
            }
            exported += 1;
            let origin = self.origins.get(&cue.id);
            let label = origin.map(|o| o.label.as_str()).filter(|s| !s.is_empty());
            if vtt {
                if let Some(label) = label {
                    let _ = writeln!(output, "{label}");
                }
            } else {
                // Imported labels can be out of order or have gaps after edits.
                let _ = writeln!(output, "{exported}");
            }
            let end = self.effective_end_at(index);
            let settings = if vtt {
                origin.map(|o| o.settings.as_str()).unwrap_or("")
            } else {
                ""
            };
            let _ = writeln!(
                output,
                "{} --> {}{}{}",
                format_time(cue.start_ms, vtt),
                format_time(end, vtt),
                if settings.is_empty() { "" } else { " " },
                settings
            );
            let _ = writeln!(output, "{text}");
            output.push('\n');
        }
        output
    }
}

fn parse_time(text: &str) -> Result<i64, String> {
    let invalid = || format!("Invalid subtitle timestamp: {text}");
    let text = text.replace(',', ".");
    let (clock, fraction) = text.split_once('.').ok_or_else(invalid)?;
    if fraction.len() != 3 || !fraction.bytes().all(|c| c.is_ascii_digit()) {
        return Err(invalid());
    }
    let parts = clock
        .split(':')
        .map(str::parse::<i64>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| invalid())?;
    let (hours, minutes, seconds) = match parts.as_slice() {
        [m, s] => (0, *m, *s),
        [h, m, s] => (*h, *m, *s),
        _ => return Err(invalid()),
    };
    if !(0..=9999).contains(&hours) || !(0..60).contains(&minutes) || !(0..60).contains(&seconds) {
        return Err(invalid());
    }
    Ok(((hours * 60 + minutes) * 60 + seconds) * 1000
        + fraction.parse::<i64>().map_err(|_| invalid())?)
}

fn format_time(ms: i64, vtt: bool) -> String {
    let ms = ms.max(0);
    format!(
        "{:02}:{:02}:{:02}{}{:03}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        if vtt { '.' } else { ',' },
        ms % 1000
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paragraph_timeline() -> SubtitleTimeline {
        let mut timeline = SubtitleTimeline::parse(
            "1\n00:01:07,616 --> 00:01:11,552\nHello.\n\n2\n00:01:11,552 --> 00:01:15,264\nNext.\n",
        )
        .unwrap();
        timeline.cues[0].original_text = "Hello.\r\n \t\r\nWelcome.\rAgain.".into();
        timeline.cues[0].translated_text = Some("你好。\n\n欢迎。\r\r再来。".into());
        timeline.cues[0].source_language = "en".into();
        timeline.cues[0].target_language = "zh".into();
        timeline
    }

    #[test]
    fn generated_paragraphs_do_not_split_srt_or_vtt_cues() {
        let timeline = paragraph_timeline();
        for output in [timeline.export_srt(), timeline.export_vtt()] {
            let parsed = SubtitleTimeline::parse(&output).expect("export must be valid subtitles");
            assert_eq!(parsed.count(), 2);
            assert_eq!(parsed.cues[0].start_ms, 67_616);
            assert_eq!(parsed.cues[0].end_ms, 71_552);
            assert_eq!(
                parsed.cues[0].original_text,
                "Hello.\nWelcome.\nAgain.\n你好。\n欢迎。\n再来。"
            );
            assert_eq!(parsed.cues[1].original_text, "Next.");
            assert!(!output.contains('\r'));
        }
        // Export normalization must not change the editable transcript.
        assert!(timeline.cues[0].original_text.contains("\r\n \t\r\n"));
    }

    #[test]
    fn srt_renumbers_imported_labels_and_skips_empty_cues() {
        let mut timeline = SubtitleTimeline::parse(
            "9\n00:00:03,000 --> 00:00:04,000\nLast.\n\n4\n00:00:01,000 --> 00:00:02,000\nFirst.\n\n5\n00:00:02,000 --> 00:00:03,000\nEmpty.\n",
        )
        .unwrap();
        timeline.cues[0].translated_text = Some("\n First.\r\n".into());
        timeline.cues[1].original_text = "\r\n \t\n".into();
        timeline.cues[2].original_text.clear();
        timeline.cues[2].translated_text = Some("最后。".into());
        assert_eq!(
            timeline.export_srt(),
            "1\n00:00:01,000 --> 00:00:02,000\nFirst.\n\n2\n00:00:03,000 --> 00:00:04,000\n最后。\n\n"
        );
        assert!(timeline.export_vtt().contains("\n\n9\n00:00:03.000"));
    }

    #[cfg(all(feature = "mpv", any(windows, target_os = "linux")))]
    #[test]
    fn video_export_keeps_multiline_cues_together() {
        let output = paragraph_timeline().export_playback_srt();
        let parsed = SubtitleTimeline::parse(&output).unwrap();
        assert_eq!(parsed.count(), 2);
        assert_eq!(
            parsed.cues[0].original_text,
            "Hello.\nWelcome.\nAgain.\n你好。\n欢迎。\n再来"
        );
    }
}
