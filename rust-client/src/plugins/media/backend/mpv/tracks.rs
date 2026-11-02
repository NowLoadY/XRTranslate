use super::super::AudioTrack;
use libmpv_sys::{
    mpv_format_MPV_FORMAT_FLAG as FLAG, mpv_format_MPV_FORMAT_INT64 as INT64,
    mpv_format_MPV_FORMAT_NODE_ARRAY as ARRAY, mpv_format_MPV_FORMAT_NODE_MAP as MAP,
    mpv_format_MPV_FORMAT_STRING as STRING, mpv_node,
};
use serde_json::Value;
use std::ffi::CStr;

/// Copy property-event data while libmpv owns its lifetime.
pub(super) unsafe fn read(node: &mpv_node) -> Vec<AudioTrack> {
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
