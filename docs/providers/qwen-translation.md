# Qwen Cloud Machine Translation (`qwen`)

XRTranslate supports Alibaba Cloud Model Studio's `qwen-mt-flash` (and other Qwen-MT models like `qwen-mt-plus` and `qwen-mt-lite`) as the cloud `qwen` translation provider.

The `dashscope-translation` connector uses the OpenAI-compatible HTTP endpoint
with Qwen-MT's structured `translation_options`. The user message contains only
the source text. Source and target language names are supplied separately;
Prompt Studio's rendered instructions, style and reference context are sent as
the `domains` hint. The shared translation pipeline still owns context,
streaming previews, validation and recovery.

## Configure

1. Obtain a Model Studio API key from the [Alibaba Cloud Model Studio Console](https://modelstudio.console.alibabacloud.com/).
2. In the welcome flow, select Online API and the regional Qwen translation service.
3. Enter the API key. The preset already supplies its endpoint:
   - Default (China/Beijing): `https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions`
   - International (Singapore): `https://dashscope-intl.aliyuncs.com/compatible-mode/v1/chat/completions`
4. Beijing workspace keys also work with the default Beijing endpoint. A custom
   deployment can override `translation.providers.<provider>.url` in user
   configuration; the welcome flow does not require a Base URL field.
5. Default model: `qwen-mt-flash`, retained for game dialogue as the balance
   between latency, language coverage, and translation quality. `qwen-mt-lite`
   is an option for simpler exchanges when speed matters most and its language
   coverage is sufficient; `qwen-mt-plus` is an option for more demanding
   translation. ASR and translation model selections remain independent.

## Prompt Studio Integration

- Catalogue metadata selects the shared chat-messages prompt profile and the
  structured translation connector independently of the service host.
- The connector preserves rendered content and message order in the hint.
  It does not author instructions or maintain another
  history. Context and terminology are included only when the graph renders them.
- The official domain hint expects English instructions. As with any specialized
  translation model, arbitrary custom instructions are not equivalent to general
  chat support. Qwen-MT does not advertise the separate Ask/chat capability.

## Official References

- [Qwen-MT Translation Model - Alibaba Cloud Model Studio](https://www.alibabacloud.com/help/en/model-studio/machine-translation)
- [Qwen-MT API reference](https://help.aliyun.com/zh/model-studio/qwen-mt-api)
