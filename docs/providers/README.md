# Provider integration

This is the entry point for adding or reviewing a model provider. The runtime
is deliberately split into shared orchestration and capability-specific
provider code so a new provider has one predictable composition path.

## Dependency path

```text
remote_catalog.json + config.json + xrtranslate-config
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

## Shared ASR audio admission

`backend/pipeline/audio.rs` owns aligned raw/enhanced buffering and bounded VAD
evidence for every connector. Empty enhancer output means pending input; an
enhancer failure switches all unprocessed audio to aligned raw bypass. Draining
preserves the complete raw tail and excludes synthetic padding from ASR and its
timestamps. Enhancement feeds VAD and speaker analysis; ASR still receives raw PCM.

Complete turns and overlapping windows require sufficient new speech duration
and density. Clear short words and continuations of accepted windows are retained;
pre-roll and old overlap cannot qualify a fresh utterance. Live connectors use the
same admission policy before sending their retained onset, and a rejected final
turn completes without committing recognition or translation. These decisions
must not branch on model names or live inside a provider adapter.

## ASR text capabilities

ASR instruction text, lexical context, and weighted vocabulary are independent
capabilities. A provider profile must describe them independently instead of
using a generic `supports_prompt` assumption.

| Capability | Configuration | Prompt Studio path | Adapter input | Meaning |
|---|---|---|---|---|
| Semantic instruction | `capabilities.asr_prompt_mode: instruction` | `ASR PROMPT` | `instruction_prompt` | Natural-language directions to the recognizer |
| Lexical context bias | `capabilities.asr_prompt_mode: context_bias` | `ASR CONTEXT` | `context_bias` | Likely words or phrases; not an instruction |
| No text input | `capabilities.asr_prompt_mode: none` | none | neither text field | Provider receives no composed ASR text |
| Weighted vocabulary | `capabilities.vocabulary_bias: true` plus provider weight settings | bypasses the text graph | structured `AsrVocabularyBias` entries | Provider-native term-to-weight bias |

`capabilities.context_max_chars` applies only to lexical context. Legacy flat
configuration fields remain accepted.

The cloud `qwen` and `qwen-intl` providers default to
`qwen-audio-3.0-asr-flash` over DashScope's native HTTP API. They declare lexical
context bias and structured weighted vocabulary independently; see
[Qwen ASR](qwen-asr.md). Remote model languages and capabilities belong in
`xrtranslate-assets/remote_catalog.json`, separately from downloadable model assets.

## Adding an online service

1. Add the service defaults, available model IDs, and model capabilities to
   `crates/xrtranslate-assets/remote_catalog.json`. Enable a shipped preset with
   an empty provider object in `config.json`. No provider-name registration is
   needed for a service using an existing connector.
2. Select an implemented `connector` by wire format. Only a new protocol needs
   an adapter under `xrtranslate-inference/src/asr/providers/` and a connector
   dispatch entry in `backend/model_runtime/asr.rs`. Every ASR adapter consumes
   the shared `AsrOptions` and returns `AsrTranscript`.
3. Declare streaming, context, and vocabulary features through the shared
   capability schema. Keep semantic instructions, lexical context, and
   structured vocabulary separate; the pipeline routes by these capabilities.
4. Keep service-specific encoding inside the connector. UI and shared pipelines
   must not match service/model names. Welcome is the only service setup UI; it exposes built-in providers and models.
5. Validate configuration expansion and compile the affected consumers. Reuse
   existing evidence; do not add hash identities, test servers, or redundant
   regression/roundtrip suites for catalogue additions.

See [Online API providers](../online-api-providers.md) for the schema, runtime credential storage, release packaging, and connector formats.

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
| desktop onboarding | generic provider and manifest presentation | provider-specific install branches |

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
   source revision, byte size, archive mapping, required license,
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
   mirror, resume, proxy, retry, byte-count validation, or staging logic to the provider or
   UI.
4. Implement the model under
   `crates/xrtranslate-inference/src/tts/providers/<provider>/`. Reuse
   `tts/audio.rs` and `tts/onnx_runtime.rs`; keep tokenizer, phoneme, tensor,
   speaker-embedding, and graph-order rules inside the provider directory.
5. Register the provider and its default asset only in
   `apps/xrtranslate-backend/src/model_runtime/tts/`. The TTS session worker,
   `main.rs`, and pipeline continue to consume the neutral adapter. The shared
   TTS adapter group routes by the languages of active `model_assets`.
6. Let onboarding enumerate config and manifests. Add a generic
   capability to a shared schema only when multiple providers need the same
   semantics; do not add a provider-name branch for a label or download button.
7. Reuse existing real-model verification to check output sample rate and
   voice-cloning quality when model files and hardware are available. Do not
   add redundant tests for implementation details.
8. Update the runtime resource matrix, TTS runtime design, and a provider note
   documenting sources, licenses, limits, and verification evidence. Run the
   formatting and compile checks appropriate to the change.

Downloadable model packages are managed-GPU resources: NVIDIA CUDA and the
manifest's VRAM threshold are mandatory (3 GiB for small ASR/translation;
7 GiB for normal ASR/translation and current TTS packages). A small ONNX model is CPU-exempt only when it
is bundled and explicitly owned as an application component, not because a
provider happens to use ONNX files.

## Other capabilities

Translation follows the same composition boundary in
`model_runtime/translation.rs` and the existing
`xrtranslate-inference/src/translation/` domain. For the shared local TTS
runtime and resource lifecycle, consult
[local ONNX TTS runtime design](../tts-cuda-runtime-design.md).

## Provider-specific notes

- [Qwen ASR](qwen-asr.md)
- [Qwen Audio WebSocket ASR](qwen-audio-streaming-asr.md)
- [Qwen Translation](qwen-translation.md)
- [OpenVoice TTS](openvoice-tts.md)
- [OpenVoice language-pack recipe](openvoice-language-packs.md)
