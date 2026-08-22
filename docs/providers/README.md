# Provider integration

This is the entry point for adding or reviewing a model provider. The runtime
is deliberately split into shared orchestration and capability-specific
provider code so a new provider has one predictable composition path.

## Dependency path

```text
config.json + xrtranslate-config
        │ normalized capabilities and provider settings
        ▼
backend/model_runtime/{asr.rs,translation.rs,tts/}
        │ selects a profile and constructs a provider-erased adapter
        ▼
xrtranslate-inference/<capability>/providers/
        │ owns authentication, transport, wire format, and provider limits
        ▼
backend pipeline/session
        │ shared VAD, routing, retry, scheduling, traces, and protocol events
        ▼
desktop UI and plugins consume neutral config/protocol contracts
```

The dependency direction is downward through neutral contracts. The session
pipeline and UI must not branch on provider or model names.

## Shared infrastructure versus provider code

Shared infrastructure owns:

- configuration merging, validation, capability declarations, and secret
  persistence;
- model/runtime asset resolution and managed-process lifecycle;
- provider-neutral TTS clone capture, bounded synthesis work, PCM playback
  delivery, and shared ONNX execution-provider policy;
- VAD, language routing and recovery retries, scheduling, cancellation, and
  protocol events;
- Prompt Studio graph storage, validation, rendering, and execution traces;
- XR Corpus context and structured vocabulary facts.

A provider implementation owns:

- authentication and endpoint validation;
- HTTP/WebSocket request and response wire formats;
- conversion from neutral options into provider fields;
- provider-specific limits, error mapping, and response normalization;
- provider-specific text or audio preprocessing, model tensor contracts,
  synthesis options, output sample rate, and language support.

Do not move retry behavior into a prompt template. A retry may render the same
semantic graph for a different language or with optional context removed, but
the decision to retry belongs to the route/pipeline policy.

## ASR text capabilities

ASR instruction text, lexical context, and weighted vocabulary are independent
capabilities. A provider profile must describe them independently instead of
using a generic `supports_prompt` assumption.

| Capability | Configuration | Prompt Studio path | Adapter input | Meaning |
|---|---|---|---|---|
| Semantic instruction | `asr_prompt_mode: instruction` | `ASR PROMPT` | `instruction_prompt` | Natural-language directions to the recognizer |
| Lexical context bias | `asr_prompt_mode: context_bias` | `ASR CONTEXT` | `context_bias` | Likely words or phrases; not an instruction |
| No text input | `asr_prompt_mode: none` | none | neither text field | Provider receives no composed ASR text |
| Weighted vocabulary | `supports_vocabulary_bias: true` plus provider weight settings | bypasses the text graph | structured `AsrVocabularyBias` entries | Provider-native term-to-weight bias |

`asr_context_max_chars` applies only to lexical context. Weighted vocabulary
must remain structured data, including for local models added in the future;
never serialize weights into an instruction string merely to reuse Prompt
Studio.

Current local Qwen3-ASR and OpenAI-compatible audio-chat profiles use semantic
instruction mode. Their request format is unchanged by this organization.
Qwen Audio streaming uses lexical context plus a separate structured weighted
vocabulary payload; see [Qwen Audio streaming ASR](qwen-audio-streaming-asr.md).

## Adding an ASR provider

1. Add declarative defaults and capabilities under `asr.providers` in
   `config.json`. Extend `xrtranslate-config` only for a genuinely shared,
   typed capability; keep vendor-only fields in the provider object.
2. Add the transport adapter under
   `crates/xrtranslate-inference/src/asr/providers/` and re-export it through
   that directory's `mod.rs`. Keep neutral results and vocabulary types in
   `asr/types.rs`.
3. Register the provider profile and construct its provider-erased adapter in
   `apps/xrtranslate-backend/src/model_runtime/asr.rs`. The root runtime plan
   continues to expose only neutral capabilities to the pipeline.
4. If the provider uses existing instruction, context-bias, or vocabulary
   semantics, select the existing capability. Change `xrtranslate-prompt` only
   when introducing a new semantic prompt target shared by more than one
   adapter.
5. Let `pipeline/asr_prompt.rs` perform capability-based delivery. Provider
   names and wire fields do not belong there.
6. Use the declarative service-config schema for common settings. Add custom UI
   control flow only when the setting cannot be represented by the shared
   descriptors.
7. Add adapter wire-contract tests, runtime-profile registration tests,
   capability/delivery tests, configuration validation tests, and run the full
   workspace suite required by the refactoring contract.

## TTS shared and provider boundaries

Local TTS has two independent resource domains:

1. `xrtranslate-assets` owns immutable model files, archive extraction,
   staging, verification, and atomic activation.
2. `xrtranslate-config` plus the desktop runtime installer own the reusable
   ONNX Runtime, CUDA, and cuDNN closure selected for the host.

The inference crate then separates reusable mechanics from model semantics:

| Owner | Shared responsibility | Must not contain |
| --- | --- | --- |
| `tts/audio.rs` | PCM conversion and resampling | provider IDs or tensor names |
| `tts/onnx_runtime.rs` | process-wide ORT bootstrap and atomic CUDA session grouping | model filenames or text frontend rules |
| `tts/providers/<provider>/` | frontend, tensor layout, model stages, provider limits, fixed output contract | downloads, host probing, UI state |
| `backend/model_runtime/tts/` | registered profile, erased adapter, asset ownership, config parsing, adapter construction | session queue/order policy |
| `backend/tts_session.rs` | clone capture and bounded synthesis worker | provider names or model paths |
| desktop onboarding/settings | generic provider and manifest presentation | provider-specific install branches |

Reference transcripts are also a provider capability, not a universal voice
clone requirement. Audio8 consumes a transcript during registration;
OpenVoice accepts the same neutral registration request but derives its target
embedding from audio and ignores the transcript. Likewise, supported output
languages and fixed sample rates remain distinct provider contracts. Shared
code may route on a typed capability, but must not infer behavior from a
provider name.

TTS text is synthesis input, not an ASR prompt. Prompt Studio instructions,
lexical context, and weighted vocabulary never flow into a TTS provider unless
a future, separately specified TTS semantic capability requires them.

## Adding a local ONNX TTS provider

1. Add provider defaults under `tts.providers` in `config.json`. Use shared
   field names only for shared meanings. Do not advertise a language, sample
   rate, reference transcript, or cloning behavior that the adapter does not
   implement.
2. Add one immutable `ModelAssetManifest` under
   `crates/xrtranslate-assets/src/catalog/tts/`. Declare every installed file,
   source revision, byte size, SHA-256, archive mapping, required license,
   synthesis language tags, and hardware requirement. Add one manifest per
   independently installable language pack; do not put an unverified language
   in editable provider configuration. Mutually exclusive quality variants
   may claim the same language; selectable accents or speakers contained in
   one package belong in `voice_presets` and must not be represented as
   duplicate model downloads. Assign semantic file roles to tokenizer,
   pronunciation lexicon, phoneme map, graph, embedding, and license inputs;
   do not hide distinct frontend resources behind one duplicated generic role.
   Re-export it through the TTS catalogue and aggregate registry.
3. Reuse `xrtranslate-download` through the assets installer. Do not add HTTP,
   mirror, resume, proxy, retry, checksum, or staging logic to the provider or
   UI.
4. Implement the model under
   `crates/xrtranslate-inference/src/tts/providers/<provider>/`. Reuse
   `tts/audio.rs` and `tts/onnx_runtime.rs`; keep tokenizer, phoneme, tensor,
   speaker-embedding, and graph-order rules inside the provider directory.
5. Register the provider and its default asset only in
   `apps/xrtranslate-backend/src/model_runtime/tts/`. The TTS session worker,
   `main.rs`, and pipeline continue to consume the neutral adapter. The shared
   TTS adapter group routes by the languages of active `model_assets`.
6. Let onboarding and settings enumerate config and manifests. Add a generic
   capability to a shared schema only when multiple providers need the same
   semantics; do not add a provider-name branch for a label or download button.
7. Add catalogue integrity tests, provider/profile coverage tests, config asset
   ownership tests, frontend/tensor tests, forced-CUDA coverage, and an
   ignored real-model smoke test. Validate actual output sample rate and real
   voice-cloning quality, not only non-empty bytes.
8. Update the runtime resource matrix, TTS runtime design, and a provider note
   documenting sources, licenses, limits, and verification evidence. Run the
   complete formatting, compile, and test gates in the refactoring contract.

Downloadable model packages are managed-GPU resources: NVIDIA CUDA and at
least 8 GiB VRAM are mandatory. A small ONNX model is CPU-exempt only when it
is bundled and explicitly owned as an application component, not because a
provider happens to use ONNX files.

## Other capabilities

Translation follows the same composition boundary in
`model_runtime/translation.rs` and the existing
`xrtranslate-inference/src/translation/` domain. For the shared local TTS
runtime and resource lifecycle, consult
[local ONNX TTS runtime design](../tts-cuda-runtime-design.md).

## Provider-specific notes

- [Qwen Audio streaming ASR](qwen-audio-streaming-asr.md)
- [OpenVoice TTS](openvoice-tts.md)
- [OpenVoice language-pack recipe](openvoice-language-packs.md)
