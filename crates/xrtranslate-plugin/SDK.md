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
  window. On Windows, the plugin renders with its own GPU renderer and the host
  composites its shared surface. UI geometry and texture pixels stay in the plugin.
  Older hosts and other platforms retain the compatible drawing adapter.
- For a game or another intensive interactive canvas, own a window and render
  directly there. The SDK still provides controls and access to host services;
  the host does not receive or render that window's frames.

The `examples/plugin-window` directory contains a complete GPU canvas example
with shared controls. Build it with the same command as any other plugin.

Embedded pages can use ordinary egui WGPU callbacks, including their own shaders.
Keep callback resources in the plugin renderer's `CallbackResources` and create
them in `prepare`. Playback continues to use the shared playback service, so a
page does not need its own video decoder. UI controls, input and automation use
the same API with either presentation path.
Custom GPU callbacks require the shared GPU path; provide a drawing fallback or
an independent window when supporting older hosts or other platforms.

Only task requests and state need to cross the host connection for an independent
window. Recognition and translation still run through host services; each plugin
keeps its own domain state and context policy.
