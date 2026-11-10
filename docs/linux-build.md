# Linux build and use

The Linux x86_64 client uses the same models, inference pipeline, and backend
as Windows. Windowing, audio devices, fonts, and native runtime libraries are
selected at the host boundary.

## Prerequisites

Install Rust 1.95 or newer. On Debian/Ubuntu, install the native build and
desktop dependencies through the system package manager:

```sh
./scripts/install-linux-deps.sh
git submodule update --init XR-Corpus
```

The installer checks the native-architecture packages, runs `sudo apt-get update`
and `sudo apt-get install` for missing dependencies, and verifies their
`pkg-config` modules. Enter your sudo password in the terminal when requested.
When already running as root, it calls APT directly. The run and release scripts
use this same check automatically, so the separate install step is optional.
Once dependencies are installed, later launches do not need sudo or an APT update.
Only package installation is elevated; build and run the application as your
normal user. This follows Ubuntu's [APT guidance for scripts](https://documentation.ubuntu.com/project/contributors/bug-fix/install-built-packages/).

The package list in `scripts/linux-build-env.sh` includes the X11/Wayland, ALSA,
PipeWire/SPA, libclang, WebKitGTK 4.1, GTK 3 and D-Bus development packages, plus
Vulkan/EGL and font runtimes. MPV builds additionally install `libmpv-dev`. APT manages
the system files and their dependencies; the scripts do not extract `.deb`
packages into the project or create substitute linker libraries. Other
distributions need the equivalent development packages installed through their
package manager.

The build scripts compile the current working tree and never switch source
revisions. They warn if XR-Corpus differs from the revision recorded by the main
repository, while allowing local development to build as-is. A normal `git pull`
can leave the submodule at an older, incompatible revision. When updating a
checkout without local submodule work, run `git submodule update --init XR-Corpus`
after pulling. Preserve any local submodule work before deliberately switching
revisions. For a fresh clone, use `git clone --recurse-submodules`.

The required speech libraries are included in `third_party/sherpa-onnx`.
Clearing the build cache does not require downloading them again.
Set `SHERPA_ONNX_LIB_DIR` or `SHERPA_ONNX_ARCHIVE_DIR` to supply matching local
libraries or an official archive instead.

## Run from source

```sh
./scripts/run-linux.sh
```

The script builds the backend with `managed-ort` and the XR-Corpus server,
then builds and runs the client from the source tree. Repeated launches reuse
Cargo's build output and the same `runtime/` data without creating a release
directory. Set `WINIT_UNIX_BACKEND=x11` or `wayland` if the window backend
needs to be selected explicitly.

The run script also builds and updates the public plugins in `plugin-src/` before
launching, using the current source SDK and shared build cache. Plugin data is
preserved; separately installed private and third-party plugins are untouched.
The [plugin build guide](../plugin-src/README.md) covers independent builds.

GTCRN speech denoising weights and their license are embedded in the client.
Model discovery prepares them offline in the configured model directory, just
as release packages include them. Enabling denoising never requires a model
download; vocal separation remains an optional download.

The Linux scripts include MPV playback by default. Media videos play inside the
app, with the same subtitles and controls as Windows. X11 and Wayland are
supported; hardware decoding depends on the installed graphics driver.
The main window uses Vulkan. Video rendering runs on its own OpenGL thread on
the same GPU and driver, identified by their standard UUIDs. Exported memory and
GPU semaphores hand each completed frame to Vulkan; one GPU copy keeps the UI
texture independent of video rendering, with no CPU pixel readback. This requires
`VK_KHR_external_memory_fd`, `VK_KHR_external_semaphore_fd`,
`GL_EXT_memory_object_fd` and `GL_EXT_semaphore_fd`. Missing video interop support
does not change the UI renderer.

Both `scripts/run-linux.sh` and the packaged launcher unset
`__EGL_VENDOR_LIBRARY_DIRS` and `__EGL_VENDOR_LIBRARY_FILENAMES` before starting
the client, using [GLVND's default system driver discovery](https://github.com/NVIDIA/libglvnd/blob/master/src/EGL/icd_enumeration.md).
This prevents shell environments from hiding installed GPU drivers without
selecting a vendor or hardcoding driver paths. The build environment is unchanged;
the floating overlay still sets its own software-rendering environment after launch.

Set `XRTRANSLATE_FEATURES=` to build without MPV when only audio is needed.
Source builds use the system development package `libmpv-dev`, which supplies
the linker files along with the MPV runtime.

## Build a local release

Release builds also require Python 3 and the GitHub CLI (`gh`). Downloaded
resources are cached and reused on subsequent builds.

```sh
./scripts/build-linux-release.sh
./dist/linux-x86_64/xrtranslate
```

The output directory must not already exist. Set `XRTRANSLATE_RELEASE_DIR` to
another path when keeping an earlier package and its runtime data.

The release script builds and includes the current public Linux plugins
automatically. Locally installed and private plugins are not included.

The release script prepares the three basic ONNX models and CPU ONNX core
through the shared verified resource installer. It packages those fixed
resources, the application binaries, and the default XR Corpus seed. Locally
downloaded models, managed GPU runtimes, settings, and editable databases are
excluded. Additional models, including optional OCR, and compatible runtimes
are downloaded from the welcome flow according to the selected configuration.
Install the Vulkan loader (`libvulkan1` on Debian/Ubuntu), the GPU's Vulkan/EGL
drivers, and the system MPV runtime (`libmpv2`) on machines using the release
package. Mesa drivers use `mesa-vulkan-drivers` and `libegl-mesa0`; proprietary
drivers are supplied by the GPU vendor's system packages.
Account login also uses the system `libwebkit2gtk-4.1-0`
package (its dependencies include GTK 3). The separate login helper is included
in every desktop release, so no Python or additional login application is needed.
The packaged launcher shows installation instructions only
when a required shared library is missing; otherwise it opens the application
directly. The application does not require a Python interpreter,
virtual environment, or PyTorch.

The release includes a default XR Corpus seed at `corpora/default.sqlite`.
On first launch, the editable terminology database is created at
`runtime/xr-corpus.sqlite`; application updates preserve that runtime file.

Managed GGUF ASR and translation models require a compatible NVIDIA or AMD GPU. The
required VRAM varies by model card; compact choices can be offered on smaller
GPUs. AMD models use the managed Vulkan runtime; local TTS requires NVIDIA CUDA; online TTS does not.
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

## Publish a Linux release with GitHub Actions

Create the version tag and its GitHub Release, then run the **Linux release**
workflow manually from the Actions page or with GitHub CLI:

```sh
gh workflow run linux-release.yml -f tag=v0.3.3
```

The workflow builds the tag by default. To rebuild an existing release from a
specific revision, also pass `-f source_commit="$(git rev-parse HEAD)"`; the full
commit SHA is required and its Cargo version must match the release tag.
Submodules use the revisions recorded by that source commit.

The workflow uses GitHub's `ubuntu-26.04` runner and the standard release
script. It checks packaged resources, executable versions, corpus health and
the ZIP before uploading `XRTranslate-<tag>-linux-x64.zip` to the existing
release. An existing Linux ZIP with that name is replaced; release notes,
tags and other assets are preserved. Binaries link against Ubuntu 26.04 system
libraries; compatibility with older distributions is not guaranteed.

Separate caches retain Cargo downloads, compatible release build outputs and
the fixed models/ONNX archive. Build caches account for the runner image, Rust
version, lockfiles, build declarations and source revision. Resource caches
change when their declarations or preparation script change. Every run still
executes the locked Cargo builds, creates a fresh package and runs validation.
Caches include reusable model/runtime download archives and exclude release
packages and generated configuration.

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
playback device is selected when no specific device is chosen. The media
playback bar supports seeking, and clicking a subtitle plays from its timestamp.
Unavailable playback does not block task details or subtitle export.
Builds without MPV retain local audio playback through Symphonia and CPAL.

## Virtual microphone output

XRTranslate automatically prepares a virtual microphone at startup and when
refreshing audio devices. No terminal commands or separate cable installation
are needed on PulseAudio or PipeWire with `pipewire-pulse`.

Audio Studio uses the same app microphone output node and connections as Windows:

- **XRTranslate Output** is the default playback endpoint for that node.
- Select **XRTranslate Microphone** as the microphone in VRChat or the receiving app.

TTS, original voice, mixing, effects, and the Follow TTS switch use the existing
Audio Studio routes. Saved selections of other virtual cables remain unchanged.

Linux discovers software playback endpoints through PulseAudio, including
PipeWire's `pipewire-pulse`, using device metadata rather than Windows product
names. The existing ALSA microphone and speaker selections remain available.
The pair uses the sound server's built-in
[null sink](https://docs.pipewire.org/page_pulse_module_null_sink.html) and
[remapped source](https://docs.pipewire.org/page_pulse_module_remap_source.html),
with stable names `xrtranslate_output` and `xrtranslate_microphone`. It is reused
across application exits and recreated after audio-server restarts. XRTranslate
does not write system audio configuration or change default input/output devices.

If automatic creation fails, check that the PulseAudio-compatible server is
running and permits these modules, then re-enter Audio Studio to retry discovery.
The application log records the server error. Existing external virtual cables
remain usable even if creation of the built-in pair is unavailable.
