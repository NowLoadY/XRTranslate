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
  fontconfig fonts-noto-core fonts-noto-cjk fonts-dejavu-core
git submodule update --init XR-Corpus
```

## Run from source

```sh
./scripts/run-linux.sh
```

The script builds the backend with `managed-ort` and the XR-Corpus server,
then runs the client. When `dist/linux-x86_64/xrtranslate` exists, it runs that
package instead. Set `WINIT_UNIX_BACKEND=x11` or `wayland` if the window
backend needs to be selected explicitly.

For optional MPV support, install `libmpv-dev` and run with
`XRTRANSLATE_FEATURES=mpv ./scripts/run-linux.sh` while using the source tree.

## Build a local release

```sh
./scripts/build-linux-release.sh
./dist/linux-x86_64/xrtranslate
```

The release script requires the three small ONNX models at their configured
paths under `models/` and the CPU core at
`runtime/onnxruntime/cpu/libonnxruntime.so.1.28.0`. It packages the locally
installed `models/` and managed runtime directories, so a prepared machine
can reuse its model files without downloading them again. Model and runtime
archives installed later are verified by the shared installer. The application
does not require a Python interpreter, virtual environment, or PyTorch.

The release includes a default XR Corpus seed at `corpora/default.sqlite`.
On first launch, the editable terminology database is created at
`runtime/xr-corpus.sqlite`; application updates preserve that runtime file.

Local ASR, translation, and TTS models require a compatible NVIDIA GPU with at
least 7 GiB of reported VRAM. The Linux managed runtime catalogue includes
llama.cpp CUDA 12.8 and ONNX Runtime 1.28 CUDA 12 with matching CUDA and cuDNN
libraries. The compact CPU ONNX core runs the bundled VAD, denoise, and speaker
models; large managed models do not fall back to CPU.

## Audio limits

Linux microphone capture and TTS playback use CPAL. System and application
audio loopback routes currently use Windows APIs and are unavailable on Linux.
MPV's embedded child window is also Windows-only; Linux MPV support can still
handle non-embedded playback and audio extraction.
