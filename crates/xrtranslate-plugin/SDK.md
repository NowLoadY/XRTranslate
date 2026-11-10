# XRTranslate plugin SDK

This SDK contains the shared controls, services, source dependencies and build
tool. Building a plugin does not require the XRTranslate repository or a host
build. Keep the SDK in its own directory; plugin source can live anywhere.

On Windows:

```powershell
& C:/SDK/XRTranslate/xrt-plugin.exe build C:/Projects/my-plugin --sdk C:/SDK/XRTranslate --output C:/Packages/my-plugin
```

On Linux:

```sh
/opt/xrtranslate-sdk/xrt-plugin build ~/projects/my-plugin --sdk /opt/xrtranslate-sdk --output ~/packages/my-plugin
```

The source directory needs `Cargo.toml`, `plugin.json` and a Rust executable.
Use version dependencies, for example `xrtranslate-plugin = "0.1.0"`.
The build tool resolves those dependencies from this SDK. It uses the
project lockfile and seeds a new one with the SDK dependency versions.
Commit the resulting lockfile with your plugin. `--target-dir` selects a shared
build cache, `--debug` selects a development build, and `--offline` uses only
dependencies already cached by Cargo. Use `--locked` to reject lockfile changes
in reproducible builds. Install Rust and the platform's usual
native build prerequisites before building.

Relative output and cache paths are resolved from the directory where you invoke
the build tool.

For host development, `--sdk` also accepts the host source checkout directly.
Use its built `target/release/xrt-plugin` tool; no SDK export is needed.

To build several plugins in one Cargo workspace, pass the workspace directory
and repeat `--plugin <child-directory>`. They compile together and are packaged
separately beneath `--output`. The repository's `plugin-src/build.ps1` and
`build.sh` use this mode for the public plugins.

Copy the completed package into the application's `plugins` directory to install
it. The application does not compile plugins during startup.

## Compatibility

The SDK version and application version are separate. A routine application
update does not require rebuilding installed plugins. Existing plugins continue
using their supported capabilities; new optional capabilities do not invalidate
them. Raise `min_host_version` only when a plugin needs a newer application feature.

Plugins use the published message interface rather than sharing Rust objects
with the host. They may use a different Rust compiler or UI toolkit. Dependency
versions in this SDK help reproduce builds; they are not installation requirements.

## Choosing a UI

- Use the host's standard controls for small settings panels.
- Use `xrtranslate_client_core::ui::surface::serve` for a page inside the main
  window. The plugin owns layout and interaction; the host draws lossless binary
  meshes, texture deltas and named shader callbacks using its shared GPU renderer
  on every platform. Unchanged drawings are retained.
- For a game or another intensive interactive canvas, own a window and render
  directly there. The SDK still provides controls and access to host services;
  the host does not receive or render that window's frames.

The `examples/plugin-window` directory contains a complete GPU canvas example
with shared controls. Build it with the same command as any other plugin.

Embedded pages use named callbacks supplied by the shared UI components; callback
parameters cross the pipe while pipelines, 3D geometry and video stay in the host.
For custom shaders, use an independent window with its own WGPU callbacks and
resources. Playback uses the shared playback service, so an embedded page does
not need its own decoder. UI controls, input and automation use the same handlers.
Implement `UiPlugin::back` to handle system Back inside an embedded page. Return
`true` when handled (including blocked navigation while saving); return `false`
to let the host leave the page. The surface closes open popup menus first.

Only task requests and state need to cross the host connection for an independent
window. Recognition and translation still run through host services; each plugin
keeps its own domain state and context policy.

## Android packaging

Android plugins use the same process messages, host services and rendered UI.
The SDK builder accepts `--target aarch64-linux-android --no-default-features`
with the NDK compiler configured by the Android build entry.

Android does not execute code from writable application storage. The APK bundles
plugin executables as `lib<executable>.so` in `jniLibs`, and plugin manifests and
resources separately in assets. The host resolves executables within Android's
installed native library directory; plugin state remains in `runtime/plugins`.
The file picker and exports are handled by the host Activity through the shared
file-dialog adapter. Plugins never need their own Activity.

For the repository's APK build, opt in with
`[package.metadata.xrtranslate] android = true` in the plugin's Cargo manifest.
Use the shared Android-compatible UI dependencies and install the file-dialog
client during plugin initialization if the page imports or exports files.
Updating packaged code requires installing a new APK; copying executables into
writable storage is not an Android installation mechanism. Plugins can still be
enabled or disabled independently.
