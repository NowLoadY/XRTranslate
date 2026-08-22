# Qwen Audio streaming ASR

XRTranslate exposes Alibaba Model Studio's
`qwen-audio-3.0-asr-flash-streaming` as the `qwen-audio-streaming` ASR
provider. The integration uses DashScope's native duplex WebSocket protocol,
not an OpenAI-compatible HTTP emulation.

## Configure

1. Create an Alibaba Model Studio API key in the region that owns the model.
2. In Service Configuration, select `qwen-audio-streaming` for ASR and enter
   the API key.
3. Keep the endpoint on `wss://`. The shipped Beijing-compatible endpoint is
   `wss://dashscope.aliyuncs.com/api-ws/v1/inference`. A workspace endpoint is
   preferred when available:
   `wss://{WorkspaceId}.cn-beijing.maas.aliyuncs.com/api-ws/v1/inference`.
4. Optionally choose a vocabulary weight from 1 through 5, or 50. Weight 50 is
   the provider's super-hot-word setting and is intentionally not the default.

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

As documented on 2026-08-22, the Beijing deployment grants new users 36,000
seconds (10 hours) of free usage for 90 days; subsequent Beijing usage is
billed at CNY 0.00033 per second. Singapore is CNY 0.00066 per second and has
no corresponding free quota. Immediate and precompiled hot-word features do
not add a separate charge. Verify the current price and quota before relying on
them.

XRTranslate currently sends each completed VAD-delimited utterance through the
streaming protocol at real-time pace. It consumes final sentences only; Qwen's
intermediate partial results are not yet published through the session
protocol. Therefore this provider is usable without changing the existing
utterance pipeline, but its request latency includes paced audio replay.

Official references:

- <https://help.aliyun.com/zh/model-studio/fun-asr-realtime-websocket-api>
- <https://help.aliyun.com/zh/model-studio/fun-asr-client-events>
- <https://help.aliyun.com/zh/model-studio/fun-asr-server-events>
- <https://help.aliyun.com/zh/model-studio/improve-asr-accuracy>
- <https://help.aliyun.com/zh/model-studio/model-pricing>
