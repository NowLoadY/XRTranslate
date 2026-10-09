# Platform Architecture

Platform support is a host concern. Domain crates and plugins consume neutral
contracts and must not branch on operating-system names to download models,
start inference, or choose storage locations.

## Boundaries

- `xrtranslate-engine::language` owns task language selection and adaptive routing.
  Fixed direction, automatic detection into one fixed target, and bidirectional
  automatic switching are distinct intents. Existing `source_lang`/`target_lang`
  fields remain the storage and wire format; `LanguageSelection` parses them at
  boundaries. `LanguageCapabilities` intersects recognition and translation
  support for audio, and uses only translation support for text. The same policy
  validates UI choices, task admission, and dynamic switching; empty intersections
  are errors and never manufacture a Chinese/English fallback.
- Model cards and remote provider metadata declare capability subsets;
  `NativeModelRouteConfig::language_capabilities` resolves them for both client
  and backend. The client caches the result until configuration is saved or
  reloaded. Unspecified remote support remains explicitly unknown and is subject
  to the provider's final validation.
- `session_coordinator::TranslationTask` captures the selected languages, input,
  and plugin binding before backend startup. Files share one import/session path;
  deferred starts and audio reconfiguration use the captured task languages.
  Continuing or reprocessing a meeting reads that meeting's stored selection.
  Plugin UI rendering never rewrites saved language intent to fit a model.
- `xrtranslate-engine::language` owns the shared language catalogue: canonical
  codes, English display names, aliases and available script evidence. UI options,
  backend routes and inference adapters use this catalogue. The ASR pipeline
  passes canonical codes (or no language for automatic detection), including on
  retries. `xrtranslate-inference/src/asr/providers/` owns each provider's wire
  differences: Qwen3 name prefills, Qwen streaming hints and SenseVoice base codes.
  Model manifests/API capability lists remain separate from language identity;
  appearing in the catalogue does not imply support by every model. The optional
  inference `sensevoice` feature owns sherpa-onnx; the backend only constructs
  and dispatches the adapter. Unknown language codes never default to English.
- `xrtranslate-assets` owns model manifests, immutable downloads, staging,
  integrity verification, and atomic activation. Model packages are identical
  across operating systems. Provider configuration selects packages through
  `model_asset`; host/onboarding UI enumerates those manifests and must not
  branch on concrete provider or model names. `model_asset` remains the
  compatibility key for singular selections; `model_assets` is the ordered,
  provider-scoped form for capabilities such as TTS whose language packs can
  be activated together. TTS packages from the same provider that claim the
  same language are replacement model variants, not a composable set.
  `voice_presets` declares stable user-facing speaker/accent choices contained
  by one package; choosing a preset never creates another download.
- TTS Center shares Prompt Studio's `ui/components/selection_card` presentation.
  Its immutable reference recordings, matching transcripts and attribution live
  in `xrtranslate-assets/resources/voices`, explicitly embedded by `voices/mod.rs`.
  `tts_session::voices::VoiceLibrary` applies a reference through the current
  `NativeTtsAdapter`, including every active language pack. `GET/POST /tts/voice`
  uses DTOs from `xrtranslate-protocol::tts`; the UI never handles provider-specific
  clone formats. Selection is committed only after registration succeeds and
  is restored with the active provider after restart. Personal recordings and
  `selection.json` stay in `runtime/voice_clones`; releases include only curated
  audio and its attribution in `resources/voices/<id>/reference.wav`,
  `reference.txt`, `SOURCE.md` and `LICENSE`, never that writable directory.
  Source WAV files are tracked in Git alongside their transcripts and attribution.
  User cards use `voices::VoiceCatalog`, shared by the desktop and backend.
  Each reference has a stable UUID, stored under
  `runtime/voice_clones/cards/<first-two-id-characters>/<id>/` as `card.json`,
  `reference.wav` and `reference.txt`. Display names never become paths. Imports
  commit by renaming a complete staging directory; a repeated name creates a new
  identity rather than overwriting another card. The catalog loads only metadata
  in a background worker; audio is read on demand. The shared card grid renders
  visible rows only, and search results are recalculated only when content or the
  query changes. Import uses the existing media decoder/resampler and the shared
  engine PCM/WAV encoder. The default transcript comes from the configured ASR
  adapter via `POST /tts/reference/transcribe`, scheduled as offline work without
  changing the live translation session. The source-language picker is shared
  with Translation and constrained by ASR support; manual text remains optional
  and allows imports without starting model services. Clone data
  remains provider-owned and is only created for selected cards. `voice_id` also
  reads the older `builtin_id` selection field for compatibility.
  Preview playback uses a separate local output stream and cannot enter the
  live audio graph.
- `xrtranslate-download` owns download-source routing as well as transfer
  mechanics. Feature installers pass a neutral `DownloadSource`; the shared
  router maps supported official GitHub and Hugging Face URLs to the selected
  mirror. Model/runtime modules must not hard-code mirror hosts or duplicate
  URL rewriting, transfer, resume, proxy, retry, or verification behavior.
  Source changes use its cooperative cancellation contract: the feature worker
  releases the open `.part` file, the owning model/runtime installer removes
  only that resource's staging, and the manager restarts through the new source.
  Partial files from official and mirror channels are never mixed.
  Official and mirror routes carry the same immutable artifact contract, so a
  verified installed resource is reused regardless of the currently selected
  route and is replaced only after explicit resource deletion.
- The desktop model task manager is the host-level serial scheduler above the
  single-package asset transaction. It de-duplicates rapid requests, preserves
  queue order, and exposes active package/file, per-package progress, aggregate
  batch progress, completion, and failure state. The download page displays
  both transferred and installed bytes from manifests. Model and runtime
  installers do not run concurrently; they continue to reuse the same neutral
  transfer implementation without moving archive extraction into it.
- `xrtranslate-supervisor` owns the neutral `LlamaServerSpec` and process
  lifecycle. It receives an executable path and never selects a platform or
  model asset.
- `xrtranslate-config` describes runtime archives declaratively. Each archive
  declares `target`, `archive_format`, required files, size, and
  (when relevant) `cuda_version`; executable archives additionally declare
  `kind` and `executable`.
  Adding Linux assets is a configuration/catalogue change, not a second
  downloader or inference pipeline.
- CUDA runtime selection is shared by llama.cpp and in-process ONNX providers.
  The installer downloads one matching CUDA redistributable under
  `runtime/cuda/<version>`. ONNX GPU plans additionally install the declared
  cuDNN closure under `runtime/cudnn/<cuda-major>`; CUDA 12 and CUDA 13 cuDNN
  files must never share a directory. The installer atomically publishes
  `runtime/native-runtime.json` only for a complete compatible closure. The
  marker contains resolved provider, CUDA, and cuDNN directories plus an exact
  dependency preload order; backend processes consume that contract without
  modifying the system `PATH` or guessing DLL names.
- Runtime readiness requires both the complete immutable file closure and an
  exact marker matching the selected CUDA/ONNX/cuDNN plan. If verified files
  remain but the marker is absent or stale, the runtime planner automatically
  performs a zero-download validation and atomically reconstructs the marker.
  Backend startup remains blocked until that internal repair completes.
- The native-runtime marker records `llama_cpp_backend` and `onnx_backend`
  independently. Its project-relative `onnx_core_library`, `provider_dir`,
  `cuda_bin_dir`, `cudnn_bin_dir`, and `preload_libraries` are resolved through
  `RuntimeLayout`.
  Packaged backends dynamically load the selected core before any ONNX API.
  CUDA and cuDNN dependencies are preloaded in the marker's declared order;
  the core then loads its colocated `onnxruntime_providers_shared` and
  `onnxruntime_providers_cuda`. Provider DLLs must never be preloaded directly
  or combined with a core from another archive.
- Managed llama.cpp model cards declare `LlamaGpu`: NVIDIA CUDA or AMD Vulkan.
  `runtime_install/hardware.rs` owns GPU probing and the shared model eligibility
  check; the UI and runtime planner consume the same result. AMD probing queries
  Vulkan 1.2, 16-bit storage, compute queues and device-local memory directly,
  without an SDK or downloaded helper. Memory thresholds remain per model card.
  CUDA is preferred when eligible; otherwise the best eligible AMD adapter is
  selected. Vulkan server archives are declared only in `config.json` and use
  the same download/mirror/hash/extraction/repair/removal lifecycle and
  `runtime/llama.cpp` directory as CUDA servers. No CUDA/cuDNN archives are
  selected for Vulkan-only inference.
- The native marker records the Vulkan physical device index. The backend
  supplies it only to the llama child through `GGML_VK_VISIBLE_DEVICES` and
  requires `--device Vulkan0`, preventing silent CPU fallback. Host preflight
  revalidates the selection against the current driver enumeration. ONNX marker
  updates preserve the independent llama backend and device selection.
- Current managed ONNX TTS cards still declare `NvidiaCuda`; AMD llama support
  does not imply ONNX CUDA compatibility. Unsupported model choices are disabled
  before installation, and managed GPU models never silently fall back to CPU.
- Small ONNX components shipped as application resources (currently VAD,
  denoise and speaker helpers) are a separate execution class. They may use the
  compact packaged CPU ONNX core and do not cause CUDA, cuDNN, or model-package
  downloads. A package is never exempt merely because its files use ONNX.
- An eligible NVIDIA host selects the newest declared CUDA package supported by
  its driver; CUDA 12 and CUDA 13 providers remain separate immutable assets.
  If no complete compatible GPU bundle exists, planning fails with an
  actionable reason instead of mixing runtime files or silently using CPU.
- Blackwell / RTX 50-series selection keeps CUDA 12.8 as the minimum toolkit
  capability. The declared llama.cpp catalogue provides CUDA 13.1 for drivers
  reporting CUDA 13.1/13.2 and prefers CUDA 13.3 when the driver supports it.
  CUDA 12.4 is never selected for Blackwell. When 13.1 is selected because the
  driver cannot load 13.3, the UI retains an actionable NVIDIA App upgrade
  notice while using the compatible GPU runtime.
- `rust-client/src/runtime_install.rs` performs one generic workflow: select
  assets for the current target, download with `xrtranslate-download`, verify,
  extract, and persist the resulting executable path. It must not inspect
  vendor filenames such as `*-win-*`. A small, separately named legacy
  migration path may interpret old configuration entries once; normal runtime
  selection must consume normalized metadata only.
- Startup onboarding checks persisted `first_run` first, then probes configured
  resources directly from disk. The initial `Idle` state of background model
  discovery and runtime planning is UI state, not evidence that resources are
  absent; live manager state is used only after those tasks have started.
- Resource deletion follows the same ownership boundary as installation.
  `xrtranslate-assets` removes one manifest package file-by-file (preserving
  unrelated files in custom directories); the runtime installer removes only
  catalogue-managed llama.cpp, CUDA, cuDNN, and ONNX CUDA directories. External
  custom runtimes and the packaged CPU ONNX core are never deleted automatically.
- `rust-client/src/audio.rs` and the player window host expose capability
  methods. The audio host owns device enumeration and independent real-time
  route lifecycles; core studios and plugins supply neutral route configurations
  and bounded PCM inputs without leaking feature-specific node types into the host. Unsupported host
  capabilities return typed/actionable errors; they are not represented by
  fake devices or duplicated UI pipelines. A game-microphone route may target
  an already installed virtual-cable render endpoint, but user-mode routing
  must not claim to create a system capture endpoint. A bundled replacement
  would be a separately installed and signed driver capability.
- Windows system-audio capture is a typed choice between endpoint loopback and
  application process-tree loopback. Endpoint loopback captures every stream
  rendered to that endpoint; application loopback captures the selected process
  and its child processes through the Windows process-loopback API. Persisted
  graphs identify an application by normalized executable identity and display
  name. The host resolves that identity to a current process ID when compiling
  a route; process IDs are runtime state and must never be persisted as graph
  identity. Applications appear only when the Windows audio-session inventory
  can associate a render session with their process.
- Audio discovery is host-owned, automatic, and demand-driven. Entering
  Translation or Audio Studio refreshes the source snapshot once; opening the
  application selector refreshes its Windows audio sessions again. Discovery
  must not poll continuously in the background. Each category permits only one
  in-flight scan and retains its last successful snapshot; a refresh must not
  temporarily invalidate graphs, clear unrelated errors, reset level meters, or
  require a page-specific refresh button.
- Audio Studio persists one complete default graph and user-created graphs.
  Only the selected graph supplies the host audio topology; inactive graphs do
  not execute. Switching graphs preserves their edits and reconciles the host
  route. The selected graph's ASR branch is synchronized before the core
  Translation workflow starts; starting live routing controls only monitor and
  application-microphone outputs.
- Mixer connections use stable, dynamically allocated input ports. Each input
  connection owns its persisted enabled state; a disabled connection remains
  visible and editable but is excluded from execution, risk analysis, and ASR
  source derivation. Microphone, system-audio, and combined recognition modes
  are derived from these visible connection states rather than stored as a
  second hidden ASR setting.
- Optional virtual-mixer control remains a host adapter, not an audio-engine
  dependency. On Windows, the VoiceMeeter adapter is constructed only when the
  official uninstall registration and matching installed Remote DLL exist. It
  loads that DLL in place, logs in for the desktop lifetime, and exposes a
  focused status/launch/shutdown/strip-to-bus capability to Audio Studio.
  VoiceMeeter is launched automatically only while an enabled route needs one
  of its endpoints, and is shut down only if XRTranslate launched it. Route leases
  restore the previous B1/B2/B3 button value on replacement, failure, stop, or
  shutdown; they never reset the mixer or bundle a vendor DLL.

## Adding a target

1. Add runtime archive metadata to `config.json` (or the release manifest) with
   the target identifier `<os>-<arch>` and the executable path inside the
   archive.
2. Keep the backend provider plan unchanged; add target-specific runtime
   archives only. Model manifests remain platform-neutral.
3. Add host integration only where the capability genuinely differs, behind the
   existing host module boundary.
4. Add selection and lifecycle tests using declared metadata, never filename
   parsing. The generic model downloader and inference adapters must remain
   untouched.

This preserves the dependency direction in `docs/refactoring-contract.md`:
platform code composes shared capabilities, while shared capabilities remain
independent of concrete plugins and operating systems.

## Android packages and application updates

Build a signed ARM64 APK with `./scripts/build-android-release.sh`.
The script uses the current signing environment or loads
`~/.config/xrtranslate/signing/android-release.env` (under `XDG_CONFIG_HOME` when
set). Set `XRT_ANDROID_SIGNING_ENV` to use another environment file. Missing
signing values stop the build before compilation. The output goes to `dist/`.
Use `--abis x86_64` for x64, or `--abis arm64-v8a,x86_64` for a universal APK.
The shared cross-platform entry remains `python3 scripts/android.py build --profile release`.
Prerequisites and SDK setup are available through `python3 scripts/android.py --help`
and `python3 scripts/android.py setup`. Python is build tooling only.

The build reads the workspace version from `Cargo.toml`; Android never maintains
an independent version. Releases use `major.minor.patch` or
`major.minor.patch-beta.N`. The Android version code is
`((major * 100 + minor) * 100 + patch) * 1000 + rank`, with beta ranks 0–998 and
stable rank 999. Minor/patch components are 0–99 and major is 0–209; codes must
be positive. This keeps beta → stable and subsequent releases upgradeable.

Configure signing using environment variables before the release build:

```sh
export XRT_ANDROID_KEYSTORE=/absolute/path/to/release.jks
export XRT_ANDROID_KEY_ALIAS=xrtranslate
# Supply XRT_ANDROID_STORE_PASSWORD and XRT_ANDROID_KEY_PASSWORD securely.
python3 scripts/android.py build --profile release
```

Reuse the same release key for subsequent versions and keep it outside the
repository. Without signing configuration, the build explicitly emits an
`-unsigned.apk`; debug packages carry `-debug.apk`. Neither is an update asset.
The distributable output is `dist/XRTranslate-v<version>-android-arm64.apk`,
`-android-x64.apk`, or `-android-universal.apk`. Publish signed packages as assets
of the corresponding `v<version>` GitHub release; beta releases also use the
GitHub prerelease flag. The normal Gradle output remains available under
`android/app/build/outputs/apk/`.

Android shares `app_update` discovery, stable/beta selection, progress, proxy,
download integrity checks and GitHub fallback with desktop. It accepts only
release APKs for its architecture (or universal), while desktop selects its own
ZIP packages. `updates/UpdateInstaller` handles only package/version/signer
validation, unknown-app-source permission and the Android system installer.
The FileProvider exposes only the update download directory. Cancelling system
installation leaves the downloaded update available to retry. Installing an
update always requires system confirmation; a differently signed development
installation cannot be overwritten by a release package.
