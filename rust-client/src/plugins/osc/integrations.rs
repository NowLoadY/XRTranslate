//! Optional cooperation belongs to the consumer, not to the plugin loader.
use super::OscPlugin;
use crate::{
    plugin_host::Host,
    plugins::{PluginId, PluginPreferences, text_output::LyricsText},
};
use serde_json::json;

pub(crate) fn connect(host: &mut Host, vrchat: &OscPlugin, chilloutvr: &OscPlugin) {
    for (id, plugin) in [
        (PluginId::OSC, vrchat),
        (PluginId::CHILLOUTVR_OSC, chilloutvr),
    ] {
        let output = plugin.publisher();
        host.subscribe("lyrics.text", move |value| {
            if value["output"] == id.as_str()
                && let Ok(text) = serde_json::from_value(value["text"].clone())
            {
                output.set_lyrics(text);
            }
        });
    }
}
pub(crate) fn sync(
    host: &mut Host,
    preferences: &PluginPreferences,
    vrchat: &OscPlugin,
    chilloutvr: &OscPlugin,
) {
    let available = host.available("lyrics");
    let mut requests = Vec::new();
    for (id, plugin) in [
        (PluginId::OSC, vrchat),
        (PluginId::CHILLOUTVR_OSC, chilloutvr),
    ] {
        if available
            && preferences.is_enabled(id)
            && let Some(budget) = plugin.applied().lyric_budget()
        {
            requests.push(
                json!({"output":id,"budget":budget,"offset_ms":plugin.applied().lyric_offset_ms()}),
            );
        }
    }
    if host.set_context("outputs.lyrics", json!(requests)) {
        for (id, plugin) in [
            (PluginId::OSC, vrchat),
            (PluginId::CHILLOUTVR_OSC, chilloutvr),
        ] {
            if !requests.iter().any(|value| value["output"] == id.as_str()) {
                plugin.publisher().set_lyrics(LyricsText::default());
            }
        }
    }
}
