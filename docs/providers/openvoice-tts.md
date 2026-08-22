# OpenVoice TTS provider

XRTranslate's `openvoice` provider is a pure-Rust, in-process ONNX TTS and
voice-cloning path. It does not start Python, an external model server, or a
provider-specific downloader. The current package is the NVIDIA NVIGI
OpenVoice v3 bundle, which combines an English MeloTTS v3 base voice with the
OpenVoice V2 tone-color converter.

This note documents the implementation that is actually registered today. It
must not be read as a claim that every language supported by upstream
OpenVoice V2 is available through this particular package and frontend.

## Current capability contract

| Capability | Current contract |
| --- | --- |
| Provider ID | `openvoice` |
| Transport | in-process `onnx` |
| Model asset | `openvoice-v3-onnx-fp16` |
| Synthesis language | English only: `en`, `en-US`, `en-GB`, or `English` |
| Output | mono PCM16 at 22,050 Hz |
| Voice reference | mono PCM16 WAV; decoded and resampled to 22,050 Hz |
| Reference transcript | accepted by the shared clone API but not consumed by OpenVoice |
| User synthesis option | `speed`, validated in `0.5..=2.0` |
| Execution device | `Auto`, `CUDA`, or `CPU`; no Vulkan or DirectML provider |

The English-only restriction belongs to this model package and frontend, not
to shared TTS infrastructure. Upstream OpenVoice V2 describes broader
multilingual support, while the selected NVIGI package supplies the English
MeloTTS frontend and base speaker used here. A future multilingual package
must declare and implement its own language capability instead of weakening
the current adapter's guard.

The 22,050 Hz value is part of the raw-binary TTS wire contract. The backend
sends PCM bytes without an inline header, so the desktop derives the source
rate from the selected model asset's immutable `audio_output` capability. The
editable legacy `sample_rate` setting is not trusted for native model packages;
this prevents a configuration-only change from altering playback speed or
pitch.

## Model and signal path

```text
text
  -> English normalization + CMU pronunciation lookup
  -> BERT WordPiece tokens and contextual embeddings
  -> MeloTTS English v3 base synthesis at 44,100 Hz
  -> resample to 22,050 Hz
  -> OpenVoice V2 tone-color converter
       source embedding: packaged EN-Newest base speaker
       target embedding: registered reference voice
  -> mono PCM16 at 22,050 Hz

reference PCM16 WAV
  -> decode + resample to 22,050 Hz
  -> 1024-point Hann STFT, hop 256, reflect padding 384
  -> FP32 reference encoder
  -> 256-value target speaker embedding
```

The provider owns the English frontend, phoneme/tone/language tensor layout,
model input and output names, reference spectrogram contract, and conversion
logic. Shared `tts/audio.rs` owns reusable PCM conversion and resampling;
shared `tts/onnx_runtime.rs` owns ONNX Runtime initialization and atomic
CUDA-to-CPU session construction.

The frontend deterministically expands English numbers, ordinals, currency,
and common symbols, then uses the packaged CMU dictionary. The NGC archive also
contains a G2P graph, but does not publish the grapheme-index mapping required
to call it. That graph is deliberately not driven with guessed indices; an OOV
word falls back to spelling until a versioned token contract can be verified.

The BERT, MeloTTS, converter, and reference-encoder sessions are constructed
as one execution-provider group. If any CUDA session cannot be constructed,
the CUDA attempt is dropped and the complete group is rebuilt on CPU. The
backend reports the device that actually prepared, not the requested setting.

## Asset provenance

The asset manifest is the only installation contract. It fixes every source
by size and SHA-256 and activates the package only after all required files
pass preflight.

- The primary archive is NVIDIA's
  [NVIGI OpenVoice v3 model package](https://catalog.ngc.nvidia.com/orgs/nvidia/nvigisdk/models/openvoice).
  The NGC package contains the model configuration, QINT8 BERT graph, FP16
  MeloTTS and converter graphs, CMU dictionary, BERT vocabulary, base-speaker
  embedding, and their license notices.
- The FP32 reference encoder is the immutable
  [`tone_ref_encoder.onnx` export at revision `34d010c`](https://huggingface.co/TigreGotico/voiceclonnx-openvoice-v2/commit/34d010c192c97f763207f488f6057fd07fee42ad).
  Its model card records the upstream OpenVoice V2 provenance, strict state
  loading, spectrogram contract, and ONNX parity measurements.
- The upstream project is
  [MyShell OpenVoice](https://github.com/myshell-ai/OpenVoice). Its current
  repository states that OpenVoice V1 and V2 are MIT licensed. The extracted
  package notices remain authoritative for the exact redistributed files.

The network transfer is 207,772,473 bytes: the 204,513,198-byte NGC archive
plus the independently pinned 3,259,275-byte reference encoder. The required
installed files total 255,830,497 bytes after archive extraction. Neither the
model package nor Python is bundled into an XRTranslate release.

## Integration boundaries

Concrete OpenVoice knowledge is intentionally limited to these owners:

- `crates/xrtranslate-assets/src/catalog/tts/openvoice.rs`: immutable files,
  archive mapping, hashes, licenses, provider identity, and model label;
- `crates/xrtranslate-inference/src/tts/providers/openvoice/`: frontend, tensor
  contract, reference encoding, synthesis, and provider validation;
- `apps/xrtranslate-backend/src/model_runtime/tts/`: provider registry,
  provider-erased adapter, configuration parsing, asset ownership validation,
  and per-provider construction modules;
- `config.json`: user-selectable provider settings and declarative runtime
  archives.

`main.rs`, the inference pipeline, TTS session worker, onboarding, and settings
UI consume neutral adapters, asset manifests, or capabilities. They must not
branch on `openvoice`, an NGC revision, a model filename, or a provider-specific
tensor name.

OpenVoice does not consume ASR instructions, lexical context, or weighted
vocabulary. Prompt Studio and ASR bias semantics must not be routed into this
TTS adapter merely because both features accept text elsewhere in the system.

## Verification

Fast tests protect English language gating, WordPiece-to-phone alignment,
finite spectrogram dimensions, provider/asset ownership, archive integrity
metadata, and shared CUDA fallback policy. Runtime validation rejects silent or
non-finite model output.

The ignored end-to-end test accepts these environment variables:

- `XRTRANSLATE_OPENVOICE_MODEL_DIR`;
- `XRTRANSLATE_OPENVOICE_DEVICE`;
- `XRTRANSLATE_OPENVOICE_CUDA_PRELOAD`;
- `XRTRANSLATE_ORT_DYLIB_PATH`;
- `XRTRANSLATE_OPENVOICE_REQUIRE_CUDA` when CUDA is mandatory for the run.

The forced-CUDA smoke run completed on an RTX 5070 Ti and produced 56,064
samples (2.543 seconds) without CPU fallback. Its synthetic reference proves
only that the complete graph path executes and returns non-silent PCM.
Release-quality acceptance still requires a clean real speaker WAV,
intelligible English text, ASR read-back, an audible comparison with the
reference speaker, and CPU/CUDA quality parity. A non-silent waveform alone is
not evidence of intelligibility or successful voice cloning.
