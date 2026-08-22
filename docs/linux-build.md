# Linux Build

The desktop client is built with `eframe`/`egui`. `eframe` selects the native
`winit` backend for Linux (X11 or Wayland), so Linux does not need a second
window framework or a platform-specific application entry point.

## Prerequisites

Install Rust 1.95 or newer and the native libraries used by the selected
backend. `eframe`/`egui` 0.36 require Rust 1.95; older toolchains fail before
platform compilation begins.
On Debian/Ubuntu this is typically:

```sh
sudo apt install build-essential pkg-config libx11-dev libxcursor-dev \
  libxrandr-dev libxi-dev libwayland-dev libxkbcommon-dev libasound2-dev \
  libssl-dev
```

The `xr-corpus-core` path dependency must be initialized at `XR-Corpus/`.
The core Linux client does not link to MPV. This keeps model download,
inference, session, and non-video plugins independent from native player
libraries. Build the optional MPV capability only when video playback or MPV
audio extraction is needed:

```sh
cargo build -p rust-client --features mpv
```

That build requires a system `libmpv` development package, for example
`libmpv-dev` on Debian-based distributions. Without `--features mpv`, the
player reports a clear capability error and Symphonia-only audio imports still
work; unsupported formats do not silently fall through to a missing native
library.

## Build and run

```sh
git submodule update --init XR-Corpus
cargo build -p rust-client --release
cargo run -p rust-client --release
```

Use `WINIT_UNIX_BACKEND=x11` or `WINIT_UNIX_BACKEND=wayland` to select a window
backend when both are available. Linux builds keep the same host/plugin
composition and shared runtime contracts as Windows builds.

The archive schema already supports Linux `zip` and `tar-gz` targets; the
extractor validates paths, rejects links/special entries, and restores
executable permissions on Unix. Managed local models are currently disabled on
Linux because no verified Linux NVIDIA detection/runtime closure is published.
The historical CPU archive remains readable for migration but is never selected
for a managed model. Small bundled ONNX components remain CPU-capable. Linux
GPU archives and detection can be added later without changing model download
or inference code.

## Platform limits

Microphone capture and TTS use CPAL on Linux. Windows WASAPI loopback is not
silently emulated: system-audio capture reports an actionable unavailable
error until a PipeWire/PulseAudio implementation is added. The Windows-only
embedded mpv child window is similarly isolated behind the player window host;
Linux still builds and can use mpv for non-embedded operations.
