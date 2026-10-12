# desktop-lyric reference source

Source: https://github.com/tuberry/desktop-lyric
Revision: 15c02930f11c78dc9b83f7566d2c93062eb2f62b
Downloaded: 2026-10-12

This is an unmodified source snapshot (apart from this note), retained as the
reference for XRTranslate’s lyrics plugin. Original copyright notices and
GPL-3.0-or-later license are preserved in the source and LICENSE.md.

XRTranslate adapts NetEase matching from src/lyric.js, playback synchronization
from src/mpris.js and src/extension.js, and timed lyric scrolling from
src/paper.js. The Rust adaptation lives in rust-client/src/plugins/lyrics.
It uses one provider and a bounded 50 MB LRC file cache with automatic eviction;
it does not run the GNOME extension. Cached lyrics can be cleared from the
reset icon beside the plugin's enablement switch.
