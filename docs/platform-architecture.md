# Platform Architecture

Platform support is a host concern. Domain crates and plugins consume neutral
contracts and must not branch on operating-system names to download models,
start inference, or choose storage locations.

## Boundaries

- `xrtranslate-assets` owns model manifests, immutable downloads, staging,
  integrity verification, and atomic activation. Model packages are identical
  across operating systems. Provider configuration selects packages through
  `model_asset`; host/onboarding UI enumerates those manifests and must not
  branch on concrete provider or model names.
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
- `xrtranslate-supervisor` owns the neutral `LlamaServerSpec` and process
  lifecycle. It receives an executable path and never selects a platform or
  model asset.
- `xrtranslate-config` describes runtime archives declaratively. Each archive
  declares `target`, `archive_format`, required files, size, checksum, and
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
- The native-runtime marker records `llama_cpp_backend` and `onnx_backend`
  independently. Its project-relative `onnx_core_library`, `provider_dir`,
  `cuda_bin_dir`, `cudnn_bin_dir`, and `preload_libraries` are resolved through
  `RuntimeLayout`.
  Packaged backends dynamically load the selected core before any ONNX API.
  CUDA and cuDNN dependencies are preloaded in the marker's declared order;
  the core then loads its colocated `onnxruntime_providers_shared` and
  `onnxruntime_providers_cuda`. Provider DLLs must never be preloaded directly
  or combined with a core from another archive.
- CPU-only hosts use the compact ONNX core included in the native application
  package and download no ONNX execution-provider, CUDA, or cuDNN runtime. A
  compatible NVIDIA host selects the newest declared CUDA package supported by its driver;
  CUDA 12 and CUDA 13 providers remain separate immutable assets. If no complete
  compatible GPU bundle exists, planning succeeds with the CPU backend and an
  actionable fallback reason instead of mixing incompatible runtime files.
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
  methods. Unsupported host capabilities return typed/actionable errors; they
  are not represented by fake devices or duplicated UI pipelines.

## Adding a target

1. Add runtime archive metadata to `config.json` (or the release manifest) with
   the target identifier `<os>-<arch>` and the executable path inside the
   archive.
2. Keep the model manifests and backend provider plan unchanged.
3. Add host integration only where the capability genuinely differs, behind the
   existing host module boundary.
4. Add selection and lifecycle tests using declared metadata, never filename
   parsing. The generic model downloader and inference adapters must remain
   untouched.

This preserves the dependency direction in `docs/refactoring-contract.md`:
platform code composes shared capabilities, while shared capabilities remain
independent of concrete plugins and operating systems.
