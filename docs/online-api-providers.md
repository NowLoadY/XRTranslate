# Online API providers

For the repository-wide provider boundary and the implementation checklist,
see [Provider integration](providers/README.md).

The repository `config.json` is the default document. Every platform stores
model selection, providers, and runtime parameters in
`<project>/runtime/user-config.json`. The updater preserves this runtime
directory. Older packaged builds stored the override in the platform user
configuration directory; it is merged into the runtime file on first launch
and the old copy is removed after a successful migration.

The versioned downloadable model cards live together in
`crates/xrtranslate-assets/model_catalog.json`. They declare package IDs,
supported language codes, hardware and memory estimates, benchmarks, and
pinned download files. Runtime settings reference these stable IDs; downloaded
model weights remain under the managed models directory. Known remote model
languages and capabilities are recorded separately in
`crates/xrtranslate-assets/src/remote.rs`; they do not create installable assets.

The runtime recursively merges this override over the defaults. Saving a
setting therefore never edits the tracked default file, and newly shipped
defaults remain available for fields the user has not customized.

The native route exposes ASR and translation provider settings through this
effective configuration.
Each selected provider object supports the following common fields:

- `transport`: `local` for managed llama.cpp, `onnx-cpu` for a native CPU
  recognizer, `openai` for a remote OpenAI-compatible HTTP endpoint,
  `dashscope` for native Qwen Audio HTTP recognition, or `websocket` for the
  registered Qwen Audio WebSocket adapter.
- `url`: the complete capability endpoint: `/v1/chat/completions` for
  translation and audio-chat services, `/v1/audio/transcriptions` for OpenAI ASR.
- `model`: the remote model identifier. It is required for remote transports.
- `api_key`: optional Bearer credential. The desktop settings editor masks this
  value while editing.
- `context_window_tokens`, `max_tokens`, and `parallel_slots`: request and
  scheduler limits shared by local and remote routes.

OpenAI ASR uploads a WAV file to `/audio/transcriptions` as multipart form data,
with the selected model, optional language and lexical context (`prompt`), and
JSON output. The regional Qwen ASR presets default to
`qwen-audio-3.0-asr-flash`, sending a WAV Data URL in native DashScope
`input.messages` and reading `output.text` from the
`/api/v1/services/aigc/multimodal-generation/generation` endpoint. Generic
audio-chat services use raw base64 WAV with `format: "wav"`.
These provider contracts share the same recognition and context pipeline.
Existing OpenAI transcription configurations that used the chat endpoint are
migrated automatically; custom model selections are preserved.
Known old Qwen preset settings are also migrated to the native HTTP
preset. See [Qwen ASR](providers/qwen-asr.md) for model selection, context and
weighted vocabulary capabilities, and the distinction from streaming ASR.

Translation message content is rendered by the active Prompt Studio graph in
`xrtranslate-prompt`. An online provider profile selects the
`openai_compatible` Request messages and adds transport credentials, model and
sampling fields; it must not prepend, append, or rewrite prompt text. The
built-in graph produces the original system/user message pair exactly.

ASR and translation are independent capabilities. It is valid to select a
remote ASR provider while keeping Hy-MT2 local, or the reverse. When no
selected capability uses `local`, the desktop client does not require or launch
the llama.cpp executable.

The Settings page includes OpenAI and regional Qwen presets and an **Add online
API** action. The action creates a normal provider object in the override
document, so custom OpenAI-compatible services use the existing save,
validation, and reload flow rather than a second settings store.
