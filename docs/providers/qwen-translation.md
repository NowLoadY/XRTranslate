# Qwen Cloud Machine Translation (`qwen`)

XRTranslate supports Alibaba Cloud Model Studio's `qwen-mt-flash` (and other Qwen-MT models like `qwen-mt-plus` and `qwen-mt-lite`) as the cloud `qwen` translation provider.

The integration uses the OpenAI-compatible HTTP Chat Completions endpoint, conforming to Alibaba Cloud Model Studio's machine translation specification while connecting directly with XRTranslate's Prompt Studio.

## Configure

1. Obtain a Model Studio API key from the [Alibaba Cloud Model Studio Console](https://modelstudio.console.alibabacloud.com/).
2. In **Settings -> Service Providers** (or Onboarding Step 2), select `qwen` for Machine Translation.
3. Set the endpoint URL:
   - Default (China/Beijing): `https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions`
   - International (Singapore): `https://dashscope-intl.aliyuncs.com/compatible-mode/v1/chat/completions`
4. Enter your Model Studio API Key.
5. Default model: `qwen-mt-flash` (recommended for low latency and high quality). Alternatively, use `qwen-mt-plus` for formal/domain precision or `qwen-mt-lite` for live subtitle scenarios.

## Prompt Studio Integration

- Qwen-MT enforces a strict single-turn message structure (`role: user`).
- XRTranslate's `QwenRemote` translation profile maps directly to Prompt Studio's `OPENAI` translation graph.
- The runtime automatically merges system instructions, reference context, bilingual terminology glossaries, and historical turns into the single-turn prompt payload.
- As a result, custom Prompt Studio DAG flows, glossaries, and tone guidelines seamlessly guide Qwen-MT without triggering format rejection errors.

## Official References

- [Qwen-MT Translation Model - Alibaba Cloud Model Studio](https://www.alibabacloud.com/help/en/model-studio/machine-translation)
