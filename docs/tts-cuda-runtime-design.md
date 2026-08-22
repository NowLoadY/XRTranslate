# Local ONNX TTS runtime design

## Goal

Local ONNX TTS providers use CUDA whenever a complete compatible NVIDIA
runtime is available and otherwise use CPU. Vulkan and DirectML are not
selectable execution providers. The application supplies a matched local ONNX
Runtime, CUDA, and cuDNN closure, so users do not install a CUDA Toolkit or
modify the machine-wide `PATH`.

Audio8 and OpenVoice share this host/runtime infrastructure while keeping
their model files, frontends, tensor contracts, voice-registration semantics,
and synthesis rules inside their provider modules. Model packages live under
`models/`, are downloaded separately, and are never bundled into the default
application release.

## Ownership

- `xrtranslate-assets` declares TTS model packages and owns model download,
  archive extraction, staging, integrity verification, deletion, and atomic
  activation.
- `xrtranslate-config` declares immutable runtime archives and their target,
  CUDA ABI, extraction metadata, required files, size, and checksum. It does
  not probe the host or perform downloads.
- The desktop host probes NVIDIA capabilities and converts configured ASR,
  translation, and TTS providers into neutral `RuntimeRequirements`.
- `runtime_install` selects declared llama.cpp, ONNX Runtime, CUDA, and cuDNN
  assets; downloads them through `xrtranslate-download`; verifies them; and
  publishes an app-local runtime marker.
- The backend consumes the resolved marker before the first ONNX API call. It
  does not download files, inspect the operating system, or guess library
  names.
- `xrtranslate-inference::tts::onnx_runtime` owns process-wide ORT bootstrap
  and the shared atomic CUDA-to-CPU session-group policy.
- Concrete provider modules own model sessions and tensor behavior. The
  provider registry in `backend/model_runtime/tts/` is the only backend
  factory that names them.
- UI code consumes declarative provider/manifests and runtime diagnostics and
  emits typed install, repair, delete, or retry actions. It does not own host
  probing or installation.

This dependency direction follows `platform-architecture.md` and
`refactoring-contract.md`.

## Neutral runtime plan

The host contract describes capabilities instead of provider or archive
names:

```text
RuntimeRequirements
├─ llama_cpp: bool
├─ onnx_tts: bool
└─ onnx_cuda: bool

DetectedAccelerator
└─ Nvidia { compute_capability, driver_cuda }

ResolvedRuntimePlan
├─ llama backend: Cuda { abi } | Cpu
├─ ONNX backend: Cuda { major } | Cpu
├─ declared assets: ONNX core/provider + CUDA + cuDNN
└─ diagnostic: Ready | DownloadRequired | RepairRequired | Unsupported
```

`onnx_tts` means that a configured local TTS provider uses the ONNX transport;
it does not mean Audio8 or OpenVoice specifically. `onnx_cuda` is false for an
explicit CPU selection. Provider or model changes use last-write-wins
planning: an already-started immutable download may finish, after which the
host recomputes the plan and reuses every matching verified asset.

Selection uses the highest declared CUDA ABI supported by the driver and GPU
compute capability for which ONNX Runtime, a shared CUDA redistributable, and
cuDNN form a complete plan. When llama.cpp is also required, its server bundle
uses the compatible shared CUDA choice. CUDA 13 is selected only when the
whole closure is present; otherwise a compatible CUDA 12 plan or CPU is
selected. Blackwell/RTX 50-series hardware never selects the declared CUDA
12.4 llama.cpp package. Drivers reporting CUDA 13.1/13.2 select the CUDA 13.1
llama.cpp bundle; CUDA 13.3-capable drivers prefer 13.3. The UI retains the
NVIDIA App upgrade notice when it uses 13.1 for a driver that cannot load 13.3.

A host without an NVIDIA GPU selects CPU without downloading GPU assets. A
detected device without a complete compatible closure also produces a valid
CPU plan with an actionable fallback reason. Requested `Auto`/`CUDA` and the
backend that actually prepared are different facts and are reported separately.

## Runtime package layout

Runtime resources are split by ownership and ABI:

```text
runtime/
├─ llama.cpp/                  # selected server and backend libraries
├─ onnxruntime/
│  ├─ cpu/                     # compact release-packaged core
│  ├─ cuda-12/                 # official ORT core + shared/CUDA providers
│  └─ cuda-13/
├─ cuda/
│  ├─ 12.4/                    # cudart + cublas closure
│  ├─ 13.1/
│  └─ 13.3/
└─ cudnn/
   ├─ 12/                      # declared cuDNN 9 CUDA-major closure
   └─ 13/
```

The CPU ONNX core is included with the native application. A CPU-only plan
downloads no ONNX provider, CUDA, or cuDNN archive. CUDA plans install the
official ONNX core and its `onnxruntime_providers_shared` and
`onnxruntime_providers_cuda` libraries as one indivisible archive. The core
must not be mixed with provider libraries from another archive.

CUDA redistributables are shared with llama.cpp when the selected ABI and
declared artifact identity match. cuDNN is a separate ONNX GPU dependency and
is stored by CUDA major. Files from CUDA 12 and CUDA 13 are never placed in one
directory. Every archive is selected from configuration, downloaded through
the shared downloader, verified by SHA-256, and extracted to staging before
activation.

The desktop publishes `<runtime_root>/native-runtime.json`. It records the
llama.cpp and ONNX backends independently, exact ONNX core/provider directory,
CUDA and cuDNN directories, CUDA ABI, ordered preload library paths, and
fallback reason. Paths are stored relative to the movable runtime root. The
backend resolves those paths, preloads the declared CUDA/cuDNN dependencies,
and dynamically initializes the selected ONNX core without changing global
`PATH`.

## Shared inference policy

`Auto` is the production default:

1. consume and validate the managed runtime marker;
2. preload its exact dependency list before any ORT session is opened;
3. initialize the selected ORT core once for the process;
4. ask the provider to construct its atomic model group on CUDA;
5. if any session in that group cannot be constructed, drop the CUDA attempt
   and reconstruct the same group on CPU;
6. warm and reuse the resulting provider runtime and report the active device.

The shared session builder owns graph optimization, thread policy, CUDA
provider registration, and atomic fallback. It does not know model paths,
tensor names, frontend behavior, or provider IDs. A provider supplies its
ordered model paths and owns all graph-specific validation.

Raw TTS binary frames remain mono PCM16 without a custom header. The desktop
opens playback using the selected provider's declared source rate. The rate
must match `SynthesizedPcm.sample_rate`, or shared code must resample before
the bytes cross the wire. A configuration-only rate change is not a valid
sample-rate conversion.

## Provider-specific execution

### Audio8

Audio8 is the autoregressive multilingual option. Slow/Fast generation
sessions use the shared atomic execution-provider group. Registration and
codec placement retain Audio8's validated model-specific policy, including
the registration transcript and sampling controls. Its fixed PCM contract is
44,100 Hz. Details of its historical model/export correction remain in
`fix_bug/tts-audio8-distortion.md`.

### OpenVoice

OpenVoice uses four grouped sessions: QINT8 BERT, FP16 MeloTTS English v3,
FP16 OpenVoice V2 converter, and the FP32 reference encoder. Its frontend and
current package synthesize English only. Reference registration derives a
speaker embedding from audio and does not use the transcript. MeloTTS base
audio is converted to 22,050 Hz before tone-color conversion; the final PCM
contract is 22,050 Hz. See [OpenVoice TTS](providers/openvoice-tts.md) for the
model path, provenance, and test boundary.

These differences must remain inside provider code or explicit typed
capabilities. The TTS worker must not branch on `audio8` or `openvoice`.

## User experience and onboarding

The application uses a four-step onboarding flow:

- **Step 1: Welcome**: core feature introduction;
- **Step 2: Install models**: ASR and translation provider/model selection;
- **Step 3: Optional TTS**: choose any configured TTS provider or Skip;
- **Step 4: Inference Runtime**: install or select the shared llama.cpp/ONNX
  runtime plan.

Onboarding and settings enumerate provider configuration and asset manifests.
They do not name a model archive, construct a path, or own deletion. Provider
labels may communicate model capabilities such as OpenVoice's English scope,
but custom provider control flow is not required for ordinary model download.

The Settings TTS runtime card exposes only `Auto`, `CUDA`, and `CPU` and shows
planned versus active facts:

- `Planned · CUDA 13/12` or `Planned · CPU` before a backend session;
- `Active · CUDA 13/12` or `Active · CPU` after provider preparation;
- download/repair actions for an incomplete managed closure;
- an actionable incompatibility or driver-upgrade reason when relevant.

Disconnecting clears live diagnostics so the UI never presents an old active
backend as the current one.

## Verification

Required automated coverage includes:

- no NVIDIA device and explicit CPU select CPU with no GPU downloads;
- CUDA 12/13 and Blackwell selection require a complete ORT, CUDA, and cuDNN
  closure and never mix majors;
- missing cuDNN, CUDA, or provider files produce a CPU fallback or repair
  diagnostic rather than a partially active GPU plan;
- llama.cpp and ONNX reuse the same declared CUDA archive when compatible;
- marker paths remain project-relative and movable;
- legacy runtime migration does not delete the packaged CPU core or external
  custom runtimes;
- every TTS catalogue provider has a backend runtime profile;
- a provider's complete session group uses one active execution device;
- provider UI never offers Vulkan or DirectML;
- backend diagnostics report the active backend and ABI after fallback;
- raw PCM rate and provider language limits are honored end to end.

The Audio8 Slow/Fast FP16 group has been exercised with the managed CUDA 13
closure on an RTX 5070 Ti with driver CUDA 13.3. On the same GPU, OpenVoice's
forced-CUDA smoke test registered a synthetic reference and generated 56,064
samples (2.543 seconds) without CPU fallback. Real-reference intelligibility,
voice similarity, CPU/CUDA quality parity, cold start, warm first audio, and
steady-state real-time factor remain required release evidence rather than
assumptions.
