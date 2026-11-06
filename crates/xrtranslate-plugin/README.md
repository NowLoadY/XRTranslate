# Native plugin SDK

A plugin is a directory under the application's `plugins/` folder:

```text
XRTranslate/
├── plugins/
│   └── example/
│       ├── plugin.json
│       ├── xrt-example          # xrt-example.exe on Windows
│       └── LICENSE
└── …
```

Copy the directory in to install; move or delete it to uninstall. The running
host notices changes within about a second and updates processes, UI and CLI.
There is no package database, import format, dependency resolver or separate
protocol-version negotiation. Disable a plugin before replacing its executable
on Windows. Settings has a **Plugin folder** button and enable switches.

## Manifest

```json
{
  "id": "example",
  "version": "0.1.0",
  "min_host_version": "0.4.0",
  "title": { "en": "Example", "zh": "示例" },
  "executable": "xrt-example",
  "page": true,
  "subscriptions": ["translation.committed"],
  "tools": []
}
```

Only `min_host_version` determines host-version compatibility. New optional
fields may be added without a second compatibility scheme. Require a newer host
only when using a capability introduced there. Build one directory per target
OS/architecture. Optional `requires` lists capability names; missing cooperation
is reported locally, without installing dependencies or involving other plugins.

## Implementation

Implement `Plugin` and call `serve`; see [Lyrics](../../plugin-src/lyrics/src/main.rs)
and [Big Walk](../../plugin-src/bigwalk/src/main.rs). The base MIT SDK does not link the
host UI toolkit or private implementation. Other languages can use the same
newline-delimited JSON over persistent stdin/stdout pipes. No Python is required.

- `initialize`: receives UI language, host executable, private data directory,
  selected model directory, runtimes, configuration and available capabilities.
- `call` / `reply`: domain operations and results, in either direction.
- `event`: subscribed events and UI changes.
- `tick`: returns a view when it changes. Blocking work belongs on a worker.

`Context::client()` supplies a cloneable asynchronous client for UI and workers.
Poll requests on the event thread; `wait()` is for workers only. Dropping an
unfinished request cancels it. Worker requests and replies progress independently
of the UI event loop. An implementation using `Context::call` receives
results in `reply` and handles cancellation in `cancel`.

Messages are ordinary JSON objects with a `type` field:

```json
{"type":"call","id":1,"name":"example.status","arguments":{}}
{"type":"reply","id":1,"result":{"Ok":{"ready":true}}}
{"type":"reply","id":2,"result":{"Err":"Model is not selected"}}
{"type":"event","name":"ui","value":{"id":"enabled","value":true}}
{"type":"cancel","id":1}
{"type":"view","widgets":[{"type":"button","id":"run","label":{"en":"Run"}}]}
```

Call IDs are local to each sender. Stdout carries only this protocol; stderr
feeds the application log. Pipes and queues are bounded; no subprocess is spawned
per operation. Disabling/removing a plugin terminates its process and cancels its
host requests. SDK plugins also exit when the host closes stdin.

Views use existing controls with stable IDs. Their ordinary UI handlers receive
`ui` events; UI Director automatically makes them discoverable and operable from
the Agent/CLI. Do not implement a second set of UI commands. `Host` contributions
reuse `audio.capture` and `text.compose`; direct text arrives as a `ui` event with
ID `text.compose`, while translated input uses `translation.committed`. Generic
3D scenes contain points/lines and reuse the host camera controls.

Tools have names under the plugin ID, descriptions and JSON Schema `inputSchema`.
The catalogue is queried on demand. Plugins can call existing Director operations
or `inference.request` for configured instruction-capable inference. The host
resolves asynchronous Director receipts and forwards the eventual result; callers
do not run a second polling loop. The shared
`translation` module defines completed results, provenance and text formatting.
Named events connect cooperating plugins; consumers own those contracts.
The host supplies `text.action` for core text/language controls, `system.metrics`
for cached presentation data and `udp.configure`/`udp.send` for an exclusive
datagram destination per plugin. Configure with a numeric `address` and optional
`close_packets`; send with the returned `id` and `packets`. Transferring a destination
revokes old sends; removal sends its final cleanup packets. The core does not
interpret packet contents.
A `config` event replaces the plugin's saved configuration.
`state` publishes a transient snapshot to subscribers of `plugin.state`; stopping
the plugin clears it. `message` sends plain speech to the companion's inbox, and
`notice` reports an error with optional diagnostic details.

## Existing egui pages

The optional `egui` feature provides `surface::UiPlugin` and `surface::serve`.
The plugin renders its existing page with the same components and handlers. The
host forwards input and paints the resulting meshes and texture updates. The
same frame includes the UI Director registry, so CLI operations invoke those
handlers and remain visible on screen. No Rust ABI, pointers or GPU devices cross
the process boundary. `icon` and `navigation_order` control page navigation.

Only one frame is in flight per visible page; queued input is retained. Hidden
pages receive `ui.visibility=false` and keep their background work. Native GPU
resources use named paint callbacks or host-owned textures, not copied video
frames. Plugin code must use the shared renderers for these resources.

XRTranslate's [client core](../xrtranslate-client-core/README.md) contains its
existing UI components, language and session types, playback and file helpers.
The Media, Meeting and Vocabulary plugins use it without depending on the main
application crate. This optional UI integration has its own dependency licenses;
the base protocol remains usable without it.

Models are optional manifest entries with ID, label, directory, pinned revision,
runtime and files (path, HTTPS URL, bytes). The host adds Welcome-page cards and
uses the existing downloader, mirrors, cancellation and runtime provisioning.
Model weights remain separate. The host does not know individual model types.
Data lives in `<runtime>/plugins/data/<id>` and survives uninstall;
separately managed models also remain. Only install trusted native executables.
The process inherits the host working directory. Use the supplied data directory
for records and resolve bundled assets relative to the plugin executable.

## Build helper

Manual copying is sufficient. This optional native helper copies only the
manifest, executable, LICENSE, NOTICE, `licenses/` and `assets/`, then publishes
the complete directory. It adjusts the executable filename for the platform.

```sh
cargo build --release -p xrt-lyrics -p xrtranslate-plugin --features xrtranslate-plugin/host
target/release/xrt-plugin stage plugin-src/lyrics target/release/xrt-lyrics plugins/lyrics
```

The destination must not exist. `plugins.list` reports the scanned directory and
plugin status; `plugins.enable` toggles an installed plugin. There is no CLI import
or uninstall protocol: filesystem presence is the installation state.
