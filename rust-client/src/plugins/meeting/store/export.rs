use std::collections::{HashMap, HashSet};

use super::model::{MeetingBundle, Segment, SegmentMarker};

/// Deterministically exports stored meeting facts as Markdown. It never
/// summarizes or invents content; timestamp links point back to segment anchors.
pub fn render_markdown(bundle: &MeetingBundle) -> String {
    let mut output = format!(
        "# {}\n\n- Status: `{}`\n- Source: `{}`\n- Languages: `{}` → `{}`\n",
        bundle.meeting.name.trim(),
        bundle.meeting.status,
        bundle.meeting.source_kind,
        bundle.meeting.source_language,
        bundle.meeting.target_language
    );
    if let Some(path) = bundle.meeting.audio_source_path.as_deref() {
        let display_name = display_name(path);
        output.push_str(&format!(
            "- Imported audio: `{}` (external reference)\n",
            escape_inline_code(display_name)
        ));
    }
    if bundle.meeting.recording_path.is_some() {
        output.push_str("- Recording: retained in XRTranslate local storage\n");
    }
    if let Some(minutes) = &bundle.minutes {
        output.push_str("\n## Meeting notes\n\n");
        output.push_str(minutes.markdown.trim());
        output.push('\n');
    }

    let speakers: HashMap<&str, &str> = bundle
        .speakers
        .iter()
        .map(|speaker| (speaker.id.as_str(), speaker.name.as_str()))
        .collect();
    let mut markers: HashMap<&str, Vec<&SegmentMarker>> = HashMap::new();
    for marker in &bundle.markers {
        markers
            .entry(marker.segment_id.as_str())
            .or_default()
            .push(marker);
    }
    if !bundle.markers.is_empty() {
        output.push_str("\n## Highlights\n");
        for segment in &bundle.segments {
            if let Some(segment_markers) = markers.get(segment.id.as_str()) {
                let text = segment
                    .translated_text
                    .as_deref()
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .unwrap_or(&segment.original_text);
                output.push_str(&format!(
                    "\n### [{}](#segment-{})\n\n{}\n",
                    format_timestamp(segment.start_ms),
                    segment.id,
                    markdown_quote(text)
                ));
                for marker in segment_markers {
                    let note = marker.text.trim();
                    if !note.is_empty() {
                        output.push('\n');
                        output.push_str(note);
                        output.push('\n');
                    }
                }
            }
        }
        output.push('\n');
    }
    let mut emitted_segment_ids = HashSet::new();
    output.push_str("\n## Transcript\n");
    for (topic_index, topic) in bundle.topics.iter().enumerate() {
        let title = topic
            .title
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| format!("Topic {}", topic_index + 1));
        output.push_str(&format!("\n### {title}\n"));
        for segment in bundle
            .segments
            .iter()
            .filter(|segment| segment.topic_id == topic.id)
        {
            emitted_segment_ids.insert(segment.id.as_str());
            render_segment_markdown(&mut output, segment, &speakers);
        }
    }
    // Preserve evidence even if a forward-compatible partial bundle omitted its topic.
    for segment in &bundle.segments {
        if !emitted_segment_ids.contains(segment.id.as_str()) {
            render_segment_markdown(&mut output, segment, &speakers);
        }
    }
    output
}

fn display_name(path: &str) -> &str {
    path.rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("external audio")
}

fn render_segment_markdown(output: &mut String, segment: &Segment, speakers: &HashMap<&str, &str>) {
    let timestamp = format_timestamp(segment.start_ms);
    let speaker = segment
        .canonical_speaker_id
        .as_deref()
        .and_then(|id| speakers.get(id).copied())
        .or(segment.speaker_token.as_deref())
        .unwrap_or("Unknown speaker");
    output.push_str(&format!(
        "\n<a id=\"segment-{}\"></a>\n#### [{}](#segment-{}) · {}\n\n{}",
        segment.id,
        timestamp,
        segment.id,
        speaker,
        markdown_quote(&segment.original_text)
    ));
    if let Some(translation) = segment
        .translated_text
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        output.push_str("\n\nTranslation:\n\n");
        output.push_str(&markdown_quote(translation));
    }
    output.push('\n');
}

fn format_timestamp(milliseconds: i64) -> String {
    let total_seconds = milliseconds.max(0) / 1_000;
    let hours = total_seconds / 3_600;
    let minutes = (total_seconds % 3_600) / 60;
    let seconds = total_seconds % 60;
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

fn markdown_quote(value: &str) -> String {
    value
        .lines()
        .map(|line| format!("> {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn escape_inline_code(value: &str) -> String {
    value.replace('`', "'")
}
