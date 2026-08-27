# Qwen Cloud ASR (`qwen`)

XRTranslate supports Alibaba Cloud Model Studio's `qwen3-asr-flash` as the default cloud `qwen` ASR provider.

The integration uses the OpenAI-compatible HTTP Chat Completions endpoint with multimodal audio inputs (`input_audio`), transcribing completed VAD-delimited speech utterances.

## Configure

1. Obtain a Model Studio API key from the [Alibaba Cloud Model Studio Console](https://modelstudio.console.alibabacloud.com/).
2. In **Settings -> Service Providers** (or Onboarding Step 2), select `qwen` for Speech Recognition (ASR).
3. Set the endpoint URL:
   - Default (China/Beijing): `https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions`
   - International (Singapore): `https://dashscope-intl.aliyuncs.com/compatible-mode/v1/chat/completions`
4. Enter your Model Studio API Key.
5. Default model: `qwen3-asr-flash`.

## Prompt and Recognition Semantics

- **Prompt mode**: `context_bias`
- Context terms and vocabulary hints are passed as recognition context to bias transcriptions toward proper nouns, domain terminology, and game names.
- The raw PCM audio chunk is automatically encoded as a standard 16 kHz mono WAV and delivered via RFC 2397 Data URI (`data:audio/wav;base64,...`).

## Official References

- [Non-real-time speech recognition - Alibaba Cloud Model Studio](https://www.alibabacloud.com/help/en/model-studio/non-realtime-speech-recognition-user-guide)
- [Qwen3-ASR Model Overview](https://www.alibabacloud.com/help/en/model-studio/asr-model/)
