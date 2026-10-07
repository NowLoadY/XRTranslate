//! Authored subtitles keep their text, cue identity and exact timestamps.
use super::{SubtitleCue, SubtitleMetadata, SubtitleTimeline};
use serde::{Deserialize, Serialize};
use std::fmt::Write;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct CueOrigin {
    label: String,
    settings: String,
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
            let text = lines[time_index + 1..].join("\n");
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
        self.export_timed(false)
    }
    pub fn export_vtt(&self) -> String {
        self.export_timed(true)
    }

    fn export_timed(&self, vtt: bool) -> String {
        let mut output = if vtt {
            if self.vtt_prelude.is_empty() {
                "WEBVTT\n\n".into()
            } else {
                format!("{}\n\n", self.vtt_prelude.join("\n\n"))
            }
        } else {
            String::new()
        };
        for (index, cue) in self.cues.iter().enumerate() {
            let origin = self.origins.get(&cue.id);
            let label = origin.map(|o| o.label.as_str()).filter(|s| !s.is_empty());
            if vtt {
                if let Some(label) = label {
                    let _ = writeln!(output, "{label}");
                }
            } else {
                let _ = writeln!(
                    output,
                    "{}",
                    label
                        .filter(|s| s.parse::<u64>().is_ok())
                        .map(str::to_owned)
                        .unwrap_or_else(|| (index + 1).to_string())
                );
            }
            let end = if origin.is_some() {
                cue.end_ms
            } else if self.metadata_for(&cue.id).timing
                == xrtranslate_protocol::SegmentTiming::Unknown
            {
                cue.end_ms.max(cue.start_ms + 2000)
            } else if cue.end_ms <= cue.start_ms {
                cue.start_ms + 3000
            } else {
                cue.end_ms
            };
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
            let _ = writeln!(output, "{}", cue.original_text.trim());
            if let Some(translated) = cue
                .translated_text
                .as_deref()
                .map(str::trim)
                .filter(|text| !text.is_empty() && *text != cue.original_text.trim())
            {
                let _ = writeln!(output, "{translated}");
            }
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
    #[test]
    fn authored_subtitles_preserve_multiline_text_identity_and_short_timing() {
        for (input, vtt) in [
            (
                "\u{feff}7\r\n00:00:01,015 --> 00:00:01,100\r\nFirst line\r\nSecond line\r\n",
                false,
            ),
            (
                "WEBVTT\n\ncue-a\n00:01.015 --> 00:01.100 align:start\nFirst line\nSecond line\n",
                true,
            ),
        ] {
            let mut timeline = SubtitleTimeline::parse(input).unwrap();
            assert_eq!(timeline.cues()[0].original_text, "First line\nSecond line");
            assert!(timeline.cues()[0].translated_text.is_none());
            timeline.set_translation("srt_0", "译文".into());
            let saved = serde_json::to_string(&timeline).unwrap();
            let restored: SubtitleTimeline = serde_json::from_str(&saved).unwrap();
            let output = if vtt {
                restored.export_vtt()
            } else {
                restored.export_srt()
            };
            assert!(output.contains(if vtt {
                "cue-a\n00:00:01.015 --> 00:00:01.100 align:start"
            } else {
                "7\n00:00:01,015 --> 00:00:01,100"
            }));
            assert!(output.contains("First line\nSecond line\n译文"));
        }
        assert!(SubtitleTimeline::parse("1\n00:00:70,000 --> 00:00:75,000\nBad time").is_err());
        assert!(SubtitleTimeline::parse("not subtitles").is_err());
    }
}
