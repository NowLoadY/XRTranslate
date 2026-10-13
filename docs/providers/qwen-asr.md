# Qwen Cloud ASR (`qwen`, `qwen-intl`)

The default cloud Qwen recognizer is `qwen-audio-3.0-asr-flash`. The regional
presets use DashScope's native HTTP API (`transport: dashscope`), with one
recognition result per submitted audio window.

Final ASR results and revisable pseudo-streaming windows share sentence parsing
but have different readiness policies. In ordinary endpointed mode (including
non-streaming HTTP ASR), reliable sentence-ending punctuation in the final
recognition result is sufficient to start translation. This includes adaptive
pauses and maximum-duration cuts. No additional silence, timer or next ASR
window is required. The assembler preserves turn identity and context, and
retains only the incomplete suffix across audio chunks.

Pseudo-streaming windows can still revise their right edge. A terminal period,
even alongside a short VAD pause, does not complete the latest sentence. It
waits for subsequent text or a real input end; an interior sentence already has
lookahead. In both modes, commas, colons and audio-window limits alone do not
complete unfinished text. Two seconds of continuous silence, a speaker boundary,
or explicit input end release the final tail, with or without punctuation.
These sentence-readiness rules apply to every ASR connector.

Pseudo-streaming first reports after about two seconds of audio, then advances
by about two seconds using three-second overlapping windows (four seconds for
persistently sparse system audio). Recognition and sentence translation run
independently. An unchanged sentence shares its in-flight translation and cached
result across recognition revisions; only a source correction cancels that
sentence. No fixed 300 ms timer guesses sentence completion.

On models declaring ASR text bias or weighted vocabulary, pseudo-streaming may
supply the last recognized phrase to the next ASR request. It refreshes at a
pause, comma or sentence-ending boundary, not on every window. The hint is at
most 160 characters, bounded again by the model's text-context limit, and remains
separate from persistent Corpus terms. Vocabulary-only models receive a short,
lightly weighted phrase. Sentence submission, input end, route or speaker change
clears the hint; language recovery and quality retries omit it. It never enters
the translation prompt as recognition bias.

An unfinished non-revisable tail carries across windows, with a 4,096-character
resource limit and an explicit diagnostic rather than arbitrary translation
cuts. Merged text receives timestamps estimated from text partitions. Voice
cloning receives original audio windows with their original transcripts;
reconstructed captions are not paired with repeated or misaligned PCM.
Queue latency records actual waits for the inference worker; it excludes the
time retained text spends awaiting sentence completion. Total latency still
includes that time. Backend assembly diagnostics report ready sentences and
retained character counts without logging the transcript.

The HTTP adapter still returns one completed recognition result per audio
window. Sentence accumulation and incremental translation do not turn it into
continuous incremental ASR.

## Configure

1. Obtain a Model Studio API key from the [Alibaba Cloud Model Studio Console](https://modelstudio.console.alibabacloud.com/).
2. In the welcome flow, select the online regional Qwen service for Speech Recognition (ASR).
3. Set the endpoint URL:
   - `qwen` (China/Beijing): `https://dashscope.aliyuncs.com/api/v1/services/aigc/multimodal-generation/generation`
   - `qwen-intl` (Singapore): `https://dashscope-intl.aliyuncs.com/api/v1/services/aigc/multimodal-generation/generation`
   - An appropriate workspace-scoped native endpoint may also be used, such as
     `https://{WorkspaceId}.cn-beijing.maas.aliyuncs.com/api/v1/services/aigc/multimodal-generation/generation`.
4. Enter your Model Studio API key on the **Welcome** page.
5. Keep the default model `qwen-audio-3.0-asr-flash`. The API key, endpoint,
   and model must belong to the same deployment region.

## Model capabilities and request semantics

- **Lexical context:** `asr_prompt_mode: context_bias`, with a 400-character
  limit. The rendered `ASR CONTEXT` contains likely spoken words and phrases;
  it is sent in `input.messages` as a user `input_text` message. It is not a
  semantic instruction prompt.
- **Weighted vocabulary:** declared independently from lexical context. XR
  Corpus terms become `parameters.vocabulary` entries, with weights 1 through
  5 or 50. The provider accepts at most 2,000 entries, including at most 50
  super-hot words (weight 50). Terms remain structured data and do not pass
  through the text prompt graph.
- **Audio:** PCM16 is encoded as 16 kHz mono WAV, then sent once as
  `data:audio/wav;base64,...` in a user `input_audio` message. The native
  request declares the audio format and sample rate. There is no paced audio
  replay and no file-upload/polling task.
- **Language:** automatic recognition or one selected language hint. Model
  management records the 30 supported languages; Filipino uses `fil` on the
  wire, with `tl` accepted as an application alias. Local Qwen3 model language
  coverage is a separate contract.
- **Output:** the adapter reads native `output.text` as the final result for
  the submitted window. Provider partial results and timestamps are not
  published by this adapter. Sentence completion remains pipeline policy.

Remote model metadata lives in `crates/xrtranslate-assets/remote_catalog.json`.
It describes supported languages, recognition capabilities, and native text
polishing without creating downloadable model assets. Polishing is recorded
as absent for HTTP 3.0 and present for HTTP 3.1; the separate WebSocket models
remain unspecified where the documentation does not establish this behavior.
This is informational metadata, not a request toggle. Configuration and the runtime consume
those declarations through their shared capability contracts.

## Why this default

For game dialogue, the default prioritizes short audio requests, game-name
and proper-name bias, and predictable billing. The Beijing price checked on
2026-10-02 was CNY 0.00022 per submitted audio second, or CNY 0.792 per hour of
submitted audio. Overlapping recognition windows and retries also submit
audio, so this is not a guarantee of the cost per hour of gameplay.

`qwen-audio-3.1-asr-flash` is a candidate for comparison, with token-based
billing and native text polishing. Its treatment of repetitions and informal
game speech needs evaluation with representative audio before changing the
default. No measured latency or recognition-quality comparison has been made
for these two models in XRTranslate.

The separate [WebSocket adapter](qwen-audio-streaming-asr.md) sends live PCM as
it arrives and publishes cumulative recognition previews. Completed-window
replay remains available for pseudo-streaming and recovery.
Asynchronous `filetrans` models are intended for recording transcription and
are not suitable defaults for live game subtitles.

## Replacing the old preset

The old `qwen3-asr-flash` cloud preset used Chat Completions. The new model uses
a different request and response contract; changing only the model string is
insufficient. The built-in catalogue supplies the native endpoint and model.
There is no old online-preset migration or retained Qwen cloud Chat Completions
adapter. Local Qwen3 recognition remains separate.

Alibaba's retirement notice lists `qwen3-asr-flash-2025-09-08` and
`qwen3-asr-flash-2026-02-10` for retirement on 2026-10-10. The notice does not
explicitly list the unsuffixed alias, so it should not be interpreted as a
separate guaranteed retirement date for that alias.

## Official References

- [Native HTTP recognition API](https://docs.bailian.console.aliyun.com/zh/model-studio/fun-asr-flash-recorded-speech-recognition-http-api)
- [ASR model overview](https://docs.bailian.console.aliyun.com/zh/model-studio/asr-model)
- [Qwen Audio 3.1 Flash model capabilities](https://help.aliyun.com/zh/model-studio/qwen-audio-3-1-asr-flash)
- [Model pricing](https://docs.bailian.console.aliyun.com/zh/model-studio/model-pricing)
- [Old-model retirement notice](https://www.aliyun.com/notice/118434)
