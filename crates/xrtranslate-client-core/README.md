# Client core

Shared building blocks used by the application and its native plugins. This crate
does not depend on `rust-client` or any plugin implementation.

- `ui` and `i18n`: the existing controls, theme, layout, animation and translations.
  UI Director registration remains part of the controls, so a page has one set of
  handlers for both mouse input and CLI operations.
- `session_coordinator`, `recognition` and `media_import`: typed tasks, ownership,
  language selection, import options and normalized results. The host owns device
  capture, model allocation and task scheduling.
- `services`: the current host capability snapshot and asynchronous inference.
- `companion` and `voice_interaction`: shared presentation commands, surface
  status and voice-review state. Output devices keep their own rendering and input.
- `playback` and `audio_playback`: the existing players. `playback::remote` lets a
  plugin control a host-owned video surface without transferring video pixels.
- `bookmarks`, `recording`, `export` and `file_dialog`: shared record contracts and
  file helpers. Domain records and editing policies remain in their plugins.

Use `xrtranslate_plugin::surface::UiPlugin` for an existing egui page. Configure
the page with `ui::configure_surface`, render ordinary controls, and forward
notifications with `ui::forward_notices`. Examples of complete plugins are in
`plugin-src/media`, `plugin-src/meeting` and `plugin-src/vocabulary`.

Host services use the SDK's ordinary call/reply messages: `translation.start`,
`translation.stop`, `translation.pause`, `translation.resume`,
`translation.finish`, `inference.request`, `backend.prepare`,
`models.preprocessing` and `bookmarks.query`. Tasks carry their owning plugin;
the host rejects operations on another owner's tasks and cancels owned work when
the plugin is removed. Shared model configuration, downloads and inference remain
in the host. Plugins do not create another model library or backend pool.

Output plugins can subscribe to `translation.output` for the same normalized
preview, caption and lifecycle events used by the host. Presentation plugins
subscribe to `companion.presentation`, `companion.command` and `voice.state`;
their ordinary `state` event reports `companion::SurfaceStatus` in `companion`
and `companion::VoiceMode` in `voice`. Input gestures publish `voice.action`.
The SteamVR plugin demonstrates these contracts without host-side SteamVR logic.
