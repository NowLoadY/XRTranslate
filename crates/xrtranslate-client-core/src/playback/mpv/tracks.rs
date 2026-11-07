use super::super::{AudioMixSettings, AudioTrack, PlaybackAudio};
use libmpv_sys::{
    mpv_format_MPV_FORMAT_FLAG as FLAG, mpv_format_MPV_FORMAT_INT64 as INT64,
    mpv_format_MPV_FORMAT_NODE_ARRAY as ARRAY, mpv_format_MPV_FORMAT_NODE_MAP as MAP,
    mpv_format_MPV_FORMAT_STRING as STRING, mpv_node,
};
use serde_json::Value;
use std::ffi::CStr;

pub(super) const LIMITER: &str = "alimiter=limit=0.95:level=0:latency=1";

pub(super) fn apply(
    mpv: &super::MpvHandle,
    tracks: &[(i64, AudioMixSettings)],
) -> Result<(), String> {
    if tracks.len() > 1
        || tracks
            .first()
            .is_some_and(|(_, settings)| *settings != Default::default())
    {
        mpv.set_property_string("aid", "no")?;
        mpv.set_property_string("lavfi-complex", &mix_graph(tracks))
    } else {
        mpv.set_property_string("lavfi-complex", "")?;
        mpv.set_property_string(
            "aid",
            &tracks.first().map_or("no".into(), |(id, _)| id.to_string()),
        )
    }
}

/// Use the same levels and peak protection for preview and export.
fn mix_graph(tracks: &[(i64, AudioMixSettings)]) -> String {
    let inputs = tracks
        .iter()
        .enumerate()
        .map(|(i, (id, settings))| {
            format!(
                "[aid{id}]aformat=channel_layouts=stereo,volume='{}':eval=frame[a{i}];",
                volume_expression(settings)
            )
        })
        .collect::<String>();
    let labels = (0..tracks.len())
        .map(|i| format!("[a{i}]"))
        .collect::<String>();
    format!(
        "{inputs}{labels}amix=inputs={}:duration=longest:normalize=0,{LIMITER}[ao]",
        tracks.len()
    )
}

pub(super) fn resolve<'a>(
    source: &PlaybackAudio,
    tracks: &'a [AudioTrack],
) -> Option<&'a AudioTrack> {
    tracks.iter().find(|track| match source {
        PlaybackAudio::Original(id) => {
            track.external.is_none() && id.is_none_or(|id| track.id == id)
        }
        PlaybackAudio::External(path) => track.external.as_ref().is_some_and(|file| {
            file == path
                || file
                    .canonicalize()
                    .ok()
                    .zip(path.canonicalize().ok())
                    .is_some_and(|(a, b)| a == b)
        }),
        _ => false,
    })
}

fn volume_expression(settings: &AudioMixSettings) -> String {
    let volume = settings.volume.min(200) as f64 / 100.0;
    let mut intervals: Vec<_> = settings
        .duck_intervals
        .iter()
        .copied()
        .filter(|[start, end]| *end > (*start).max(0))
        .map(|[start, end]| [start.max(0), end])
        .collect();
    intervals.sort_unstable();
    let mut merged: Vec<[i64; 2]> = Vec::new();
    for [start, end] in intervals {
        if let Some(last) = merged.last_mut().filter(|last| last[1] >= start) {
            last[1] = last[1].max(end);
        } else {
            merged.push([start, end]);
        }
    }
    // A short cosine ramp avoids clicks without exposing compressor controls.
    let ramps: Vec<_> = merged
        .iter()
        .map(|[start, end]| {
            let start = *start as f64 / 1000.0;
            let end = *end as f64 / 1000.0;
            format!(
                "(0.5-0.5*cos(PI*clip(min((t-{:.3})/0.15,({:.3}-t)/0.25),0,1)))",
                start - 0.15,
                end + 0.25
            )
        })
        .collect();
    if ramps.is_empty() || volume == 0.0 {
        format!("{volume:.4}")
    } else {
        // Balanced expressions keep long timelines below the parser's nesting limit.
        fn maximum(ramps: &[String]) -> String {
            if ramps.len() == 1 {
                return ramps[0].clone();
            }
            let (left, right) = ramps.split_at(ramps.len() / 2);
            format!("max({},{})", maximum(left), maximum(right))
        }
        format!(
            "if(isnan(t),{volume:.4},{volume:.4}*(1-{}))",
            maximum(&ramps)
        )
    }
}

/// Copy property-event data while libmpv owns its lifetime.
pub unsafe fn read(node: &mpv_node) -> Vec<AudioTrack> {
    let value = unsafe { value(node) };
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|track| {
            if track["type"].as_str()? != "audio" {
                return None;
            }
            let title = [track["title"].as_str(), track["lang"].as_str()]
                .into_iter()
                .flatten()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" · ");
            Some(AudioTrack {
                id: track["id"].as_i64()?,
                title,
                selected: track["selected"].as_bool().unwrap_or(false),
                external: track["external"].as_bool().unwrap_or(false).then(|| {
                    track["external-filename"]
                        .as_str()
                        .unwrap_or_default()
                        .into()
                }),
            })
        })
        .collect()
}

unsafe fn value(node: &mpv_node) -> Value {
    unsafe {
        match node.format {
            STRING => Value::String(CStr::from_ptr(node.u.string).to_string_lossy().into_owned()),
            FLAG => Value::Bool(node.u.flag != 0),
            INT64 => Value::from(node.u.int64),
            ARRAY | MAP => {
                let list = &*node.u.list;
                let mut array = Vec::new();
                let mut map = serde_json::Map::new();
                for i in 0..list.num as usize {
                    let v = value(&*list.values.add(i));
                    if node.format == ARRAY {
                        array.push(v);
                    } else {
                        map.insert(
                            CStr::from_ptr(*list.keys.add(i))
                                .to_string_lossy()
                                .into_owned(),
                            v,
                        );
                    }
                }
                if node.format == ARRAY {
                    Value::Array(array)
                } else {
                    Value::Object(map)
                }
            }
            _ => Value::Null,
        }
    }
}
