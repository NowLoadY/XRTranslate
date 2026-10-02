# Qwen Cloud ASR (`qwen`, `qwen-intl`)

The default cloud Qwen recognizer is `qwen-audio-3.0-asr-flash`. The regional
presets use DashScope's native HTTP API (`transport: dashscope`), with one
recognition result per submitted audio window.

This fits XRTranslate's shared recognition flow: audio segmentation and
overlap stay in the pipeline, recognition text enters the shared sentence
stability gate, translation can update incrementally, and TTS receives only
validated final sentences. The provider does not decide when a sentence is
ready for translation or playback.

Audio windows and sentences have independent boundaries. A maximum-duration
audio cut does not finish a sentence: its source-text tail carries into
recognition of the next window. Eligible complete sentences pass the 300 ms
stability gate. A natural pause, speaker
change, or input end can release the unfinished final sentence. This lets
continuous speech cross audio-window limits without translating arbitrary
word-count fragments. The pending tail is bounded to 4,096 characters, with
an explicit diagnostic for pathological uninterrupted, unpunctuated input;
that resource limit is not a sentence-completion rule.

At a `MaxActiveFrames` cut, the final sentence touching the audio window's
right edge is held for next-window lookahead even if it has punctuation;
earlier complete sentences can proceed. This policy does not establish
agreement between independent ASR hypotheses. Merged or split text receives
timestamps estimated from text partitions. Voice cloning receives each
original audio window and its original transcript once through a separate
internal reference event; reconstructed captions are never paired with
misaligned or repeated audio-window PCM.

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
4. Enter your Model Studio API key in the welcome flow or **Settings -> Service Providers**.
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

Remote model metadata lives in `crates/xrtranslate-assets/src/remote.rs`.
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

The separate [WebSocket adapter](qwen-audio-streaming-asr.md) currently replays
completed audio at real-time pace and consumes final results. It therefore
does not provide a lower-latency live ASR path in the current pipeline.
Asynchronous `filetrans` models are intended for recording transcription and
are not suitable defaults for live game subtitles.

## Replacing the old preset

The old `qwen3-asr-flash` cloud preset used Chat Completions. The new model uses
a different request and response contract; changing only the model string is
insufficient. Known old preset settings are migrated to the native endpoint
and new default while retaining credentials. There is no retained legacy
Qwen cloud Chat Completions adapter. Local Qwen3 recognition remains separate.

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
