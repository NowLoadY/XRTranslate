# Qwen Audio streaming ASR

XRTranslate includes an optional adapter for Alibaba Model Studio's
`qwen-audio-3.0-asr-flash-streaming` as the `qwen-audio-streaming` ASR
provider. The integration uses DashScope's native duplex WebSocket protocol,
not an OpenAI-compatible HTTP emulation.

The default cloud Qwen preset uses the [native HTTP adapter](qwen-asr.md).
The WebSocket adapter described here consumes completed audio windows; it
does not provide continuous live-audio recognition in the current pipeline.

## Optional manual configuration

This provider is registered in the runtime but is not a shipped welcome-flow
preset. To evaluate it, add an `asr.providers.qwen-audio-streaming` object to
`runtime/user-config.json` and select it through `asr.provider`. Merge these
fields into the existing override document:

```json
{
  "asr": {
    "provider": "qwen-audio-streaming",
    "providers": {
      "qwen-audio-streaming": {
        "transport": "websocket",
        "url": "wss://dashscope.aliyuncs.com/api-ws/v1/inference",
        "model": "qwen-audio-3.0-asr-flash-streaming",
        "api_key": "YOUR_BEIJING_API_KEY",
        "asr_prompt_mode": "context_bias",
        "asr_context_max_chars": 400,
        "supports_vocabulary_bias": true,
        "vocabulary_weight": 4,
        "parallel_slots": 2
      }
    }
  }
}
```

The API key must belong to the endpoint's region. A workspace endpoint can
also be used:
`wss://{WorkspaceId}.cn-beijing.maas.aliyuncs.com/api-ws/v1/inference`.
Remote endpoints must use `wss://`. Vocabulary weights may be 1 through 5 or
50; weight 50 is the provider's super-hot-word setting.

The adapter waits for `task-started`, sends mono PCM16 at 16 kHz in 3,200-byte
frames paced at 100 ms, sends `finish-task`, then aggregates final sentences
until `task-finished`. Connection/start and complete-task deadlines bound a
stalled request, and a pipeline generation change cancels the in-flight socket.

## Prompt and vocabulary semantics

The provider declares `asr_prompt_mode: context_bias`; it does not declare
semantic instruction-prompt support.

- The `ASR CONTEXT` Prompt Studio page renders unweighted lexical recognition
  context into `payload.input.context`. This text is a list of likely spoken
  terms, not an instruction to the model. It is bounded to 400 Unicode
  characters before Prompt Studio execution, so the displayed Request trace is
  the exact provider payload. A custom static composition above the limit is
  rejected rather than silently rewritten.
- XR Corpus vocabulary is independently converted to
  `payload.parameters.vocabulary` as structured `term -> weight` entries. It
  bypasses the text graph. The adapter validates the configured weight and
  filters optional Corpus terms to the provider's term-length, 2,000-entry,
  and super-hot-word limits; an unsuitable hint cannot fail the utterance.
- The `ASR PROMPT` page is used only by providers whose profile declares
  `asr_prompt_mode: instruction`. Its rendered text is delivered verbatim as a
  semantic instruction. It is never silently converted into Qwen recognition
  context or weighted vocabulary.

This distinction is part of the provider capability contract. Adding another
ASR provider must select `none`, `instruction`, or `context_bias` explicitly,
and must declare structured vocabulary support separately.

## Service limits and current behavior

The Beijing price checked on 2026-10-02 was CNY 0.00033 per submitted audio
second, or CNY 1.188 per hour of submitted audio. Overlap and retries can
increase submitted duration. Region-specific pricing and free quotas must be
checked against the current pricing page.

XRTranslate currently sends each completed audio window through the
streaming protocol at real-time pace. It consumes final sentences only; Qwen's
intermediate partial results are not yet published through the session
protocol. Therefore this provider is usable without changing the existing
utterance pipeline, but its request latency includes paced audio replay. A
five-second audio window takes approximately another five seconds to send
after recording, before considering connection and server processing time.
It should not be selected merely because its model name contains `streaming`.
Sentence stability, incremental translation, and final TTS delivery remain
the responsibility of the shared pipeline. An audio duration limit does not
finish an incomplete sentence; source text carries across windows until the
shared sentence gate can release it. See [Qwen ASR](qwen-asr.md) for the common
sentence and resource-boundary behavior.

Official references:

- <https://docs.bailian.console.aliyun.com/zh/model-studio/qwen-audio-asr-streaming-client-events>
- <https://help.aliyun.com/zh/model-studio/improve-asr-accuracy>
- <https://docs.bailian.console.aliyun.com/zh/model-studio/model-pricing>
