# XRTranslate plugins

This is an independent Cargo workspace. It can be kept in its own repository or
copied outside the XRTranslate checkout without changing its source paths. The
host's workspace and build scripts do not compile these packages.

Download an XRTranslate SDK bundle, or export one from a host source checkout:

```powershell
cargo build --release --locked -p xrtranslate-plugin --features host
./target/release/xrt-plugin.exe sdk . C:/SDK/XRTranslate
```

Build the plugin packages from this directory:

```powershell
./build.ps1 -Sdk C:/SDK/XRTranslate -Output C:/Packages/XRTranslate-plugins
```

Use `-Plugin media,meeting` to build selected plugins. `-TargetDirectory` selects
an optional build cache. On Linux, use `bash build.sh /opt/xrtranslate-sdk
~/packages/xrtranslate-plugins` instead. The build tool in the SDK also supports
building a single plugin directory directly.

The SDK path may also be a host source checkout with its `xrt-plugin` tool already
built. Linux `scripts/run-linux.sh` uses this to update public plugins automatically
before launching, without exporting a separate SDK copy.

Copy completed plugin directories into the application's `plugins/` folder.
For a distributable application package, pass their parent directory to
`build_release.ps1 -PluginsDirectory`, or set `XRTRANSLATE_PLUGINS_DIR` when
running `scripts/build-linux-release.sh`. These inputs are ready-to-use plugin
packages, not plugin source directories. The Linux release script builds the
public plugins automatically when no package directory is supplied.

Independent host builds do not rebuild plugins. Rebuild a plugin when changing that
plugin, or when deliberately adopting an incompatible SDK interface. New optional
host features do not require existing plugins to adopt them.

## Included plugins

| Source | Platforms | Responsibility |
|---|---|---|
| `agent` | Linux, Windows | Ongoing conversation, tool execution, context and progress |
| `media` | Linux, Windows | Media projects, subtitle editing/optimization, dubbing and export |
| `meeting` | Linux, Windows | Meeting records, recording navigation and transcript processing |
| `vocabulary` | Linux, Windows | Wordbook pages and example work using the shared bookmark store |
| `vr-overlay` | Windows (page also builds on Linux) | SteamVR subtitles, companion rendering and spatial voice controls |
| `avatar` | Linux, Windows | Game controls, connections and caption displays using shared OSC and live-task services |
| `ocr` | Linux, Windows | Screen translation and status using host region capture and OCR |
| `osc` / `chilloutvr-osc` | Linux, Windows | Independent game outputs using one shared chatbox implementation |
| `lyrics` | Linux, Windows | Music observation, lyric cache, title and synchronized text |
| `bigwalk` | Linux, Windows | Translation/direct text delivery through the game bridge |
| `auto-input` | Windows | User-enabled insertion into an already focused text field |


Translation, capture, models and downloads remain host services. Each plugin owns
its own records and context policy. Private plugins are not included in public
packages. A plugin can use a host panel or its own window, depending on its needs.
