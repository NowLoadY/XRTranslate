# Qwen Cloud Machine Translation (`qwen`)

XRTranslate supports Alibaba Cloud Model Studio's `qwen-mt-flash` (and other Qwen-MT models like `qwen-mt-plus` and `qwen-mt-lite`) as the cloud `qwen` translation provider.

The integration uses the OpenAI-compatible HTTP Chat Completions endpoint.
Prompt Studio renders the translation text, and the Qwen provider profile
packs that text into the model's single-user-message request format.

## Configure

1. Obtain a Model Studio API key from the [Alibaba Cloud Model Studio Console](https://modelstudio.console.alibabacloud.com/).
2. In the welcome flow, select the online `qwen` service for Machine Translation.
3. Set the endpoint URL:
   - Default (China/Beijing): `https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions`
   - International (Singapore): `https://dashscope-intl.aliyuncs.com/compatible-mode/v1/chat/completions`
4. Enter your Model Studio API key in the welcome flow or **Settings -> Service Providers**.
5. Default model: `qwen-mt-flash`, retained for game dialogue as the balance
   between latency, language coverage, and translation quality. `qwen-mt-lite`
   is an option for simpler exchanges when speed matters most and its language
   coverage is sufficient; `qwen-mt-plus` is an option for more demanding
   translation. These choices have not been benchmarked against game audio in
   XRTranslate. ASR and translation model selections remain independent.

## Prompt Studio Integration

- Qwen-MT enforces a strict single-turn message structure (`role: user`).
- XRTranslate's `QwenRemote` translation profile selects Prompt Studio's
  `OPENAI` translation graph.
- The current provider adapter collects non-empty string message content,
  places system content first, then joins the remaining content with blank
  lines into one `role: user` message. Context, glossaries, and history are
  included when rendered by the graph; their original message roles are not
  retained in the provider request.
- This is request-format adaptation. It does not establish identical prompt
  semantics across providers or guarantee that every custom graph is accepted
  by Qwen-MT.

## Official References

- [Qwen-MT Translation Model - Alibaba Cloud Model Studio](https://www.alibabacloud.com/help/en/model-studio/machine-translation)
