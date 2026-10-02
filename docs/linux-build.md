# Linux build and use

The Linux x86_64 client uses the same models, inference pipeline, and backend
as Windows. Windowing, audio devices, fonts, and native runtime libraries are
selected at the host boundary.

## Prerequisites

Install Rust 1.95 or newer. On Debian/Ubuntu, install the native build and
desktop dependencies:

```sh
sudo apt install build-essential pkg-config libx11-dev libxcursor-dev \
  libxrandr-dev libxi-dev libwayland-dev libxkbcommon-dev libasound2-dev \
  libpipewire-0.3-dev libclang-dev \
  libegl-mesa0 libglx-mesa0 libgl1-mesa-dri \
  fontconfig fonts-noto-core fonts-noto-cjk fonts-dejavu-core
git submodule update --init XR-Corpus
```

On Debian/Ubuntu with PipeWire and libclang already installed, both Linux
scripts can download missing PipeWire/SPA development files and libclang C
headers into `target/linux-build-deps/`, without sudo. This build cache is
reused on later runs and excluded from release packages. Other distributions
need the equivalent development packages installed through their package manager.

## Run from source

```sh
./scripts/run-linux.sh
```

The script builds the backend with `managed-ort` and the XR-Corpus server,
then builds and runs the client from the source tree. Repeated launches reuse
Cargo's build output and the same `runtime/` data without creating a release
directory. Set `WINIT_UNIX_BACKEND=x11` or `wayland` if the window backend
needs to be selected explicitly.

For optional MPV support, install `libmpv-dev` and run with
`XRTRANSLATE_FEATURES=mpv ./scripts/run-linux.sh` while using the source tree.

## Build a local release

```sh
./scripts/build-linux-release.sh
./dist/linux-x86_64/xrtranslate
```

The output directory must not already exist. Set `XRTRANSLATE_RELEASE_DIR` to
another path when keeping an earlier package and its runtime data.

The release script prepares the three basic ONNX models and CPU ONNX core
through the shared verified resource installer. It packages those fixed
resources, the application binaries, and the default XR Corpus seed. Locally
downloaded models, managed GPU runtimes, settings, and editable databases are
excluded. Additional models, including optional OCR, and compatible runtimes
are downloaded from the welcome flow according to the selected configuration.
The application does not require a Python interpreter,
virtual environment, or PyTorch.

The release includes a default XR Corpus seed at `corpora/default.sqlite`.
On first launch, the editable terminology database is created at
`runtime/xr-corpus.sqlite`; application updates preserve that runtime file.

Managed GGUF ASR and translation models require a compatible NVIDIA or AMD GPU. The
required VRAM varies by model card; compact choices can be offered on smaller
GPUs. AMD models use the managed Vulkan runtime; TTS still requires NVIDIA CUDA.
SenseVoiceSmall recognition runs on the CPU without CUDA. The Linux runtime
catalogue includes llama.cpp CUDA 12.8, llama.cpp Vulkan, and ONNX Runtime 1.28 CUDA 12 with
matching CUDA and cuDNN libraries. The bundled CPU ONNX core runs the basic
VAD, denoise, speaker, SenseVoice, and lightweight OCR models; GPU models do not
fall back to CPU.

Screen sharing uses the desktop portal and the system PipeWire service. The
build dependency `libpipewire-0.3-dev` also installs the SPA headers; libclang
is needed to generate Rust bindings. The Linux package requires the system
`libpipewire-0.3.so.0` library and an `xdg-desktop-portal` backend appropriate for
the desktop. These system libraries are not copied into the release or added
to Windows and Android builds.

## Audio capture

Linux microphone capture and TTS playback use CPAL. System audio capture uses
the monitor source of the selected playback device, and application audio
capture follows that application's playback streams. Both work with PulseAudio
and PipeWire's `pipewire-pulse` service. The client loads `libpulse.so.0` at run
time, so the packaged program can start even when that library or a compatible
sound server is absent; system and application capture become available once
both are present. For Debian/Ubuntu, the runtime package is `libpulse0`.

Applications using the PulseAudio protocol must have an active playback stream
to appear in the picker. Their streams are discovered again while capture is
running, so a stream that is recreated can resume capture. PipeWire-native
applications that bypass `pipewire-pulse` are not listed as individual capture
targets; their output is still included in system audio capture. The default
playback device is selected when no specific device is chosen. MPV's embedded
child window is Windows-only;
Linux MPV support can still handle non-embedded playback and audio extraction.
