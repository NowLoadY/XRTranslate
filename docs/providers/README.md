# Provider integration

This is the entry point for adding or reviewing a model provider. The runtime
is deliberately split into shared orchestration and capability-specific
provider code so a new provider has one predictable composition path.

## Dependency path

```text
config.json + xrtranslate-config
        │ normalized capabilities and provider settings
        ▼
backend/model_runtime/{asr,translation}.rs
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
- VAD, language routing and recovery retries, scheduling, cancellation, and
  protocol events;
- Prompt Studio graph storage, validation, rendering, and execution traces;
- XR Corpus context and structured vocabulary facts.

A provider implementation owns:

- authentication and endpoint validation;
- HTTP/WebSocket request and response wire formats;
- conversion from neutral options into provider fields;
- provider-specific limits, error mapping, and response normalization.

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

## Other capabilities

Translation follows the same composition boundary in
`model_runtime/translation.rs` and the existing
`xrtranslate-inference/src/translation/` domain. TTS still has its own native
runtime and resource lifecycle; consult
[TTS CUDA runtime design](../tts-cuda-runtime-design.md) before extending it.

## Provider-specific notes

- [Qwen Audio streaming ASR](qwen-audio-streaming-asr.md)
