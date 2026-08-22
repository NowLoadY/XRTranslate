# Online API providers

For the repository-wide provider boundary and the implementation checklist,
see [Provider integration](providers/README.md).

The repository `config.json` is the immutable default document. User and
development changes are stored in a separate `user-config.json` override:

- debug/development builds: `<project>/runtime/user-config.json`;
- packaged builds: the platform user configuration directory under
  `XRTranslate/user-config.json`.

The runtime recursively merges this override over the defaults. Saving a
setting therefore never edits the tracked default file, and newly shipped
defaults remain available for fields the user has not customized.

The native route exposes ASR and translation provider settings through this
effective configuration.
Each selected provider object supports the following common fields:

- `transport`: `local` for managed llama.cpp or `openai` for an OpenAI
  Chat Completions-compatible HTTP endpoint.
- `url`: the complete `/v1/chat/completions` endpoint.
- `model`: the remote model identifier. It is required for `openai`.
- `api_key`: optional Bearer credential. The desktop settings editor masks this
  value while editing.
- `context_window_tokens`, `max_tokens`, and `parallel_slots`: request and
  scheduler limits shared by local and remote routes.

The ASR request uses an OpenAI-compatible multimodal chat message containing a
base64 WAV `input_audio` part. This is deliberately separate from the
multipart `/audio/transcriptions` API: an adapter for that contract can be
added later without changing provider selection, prompt composition, or the
session pipeline.

Translation message content is rendered by the active Prompt Studio graph in
`xrtranslate-prompt`. An online provider profile selects the
`openai_compatible` Request messages and adds transport credentials, model and
sampling fields; it must not prepend, append, or rewrite prompt text. The
built-in graph produces the original system/user message pair exactly.

ASR and translation are independent capabilities. It is valid to select a
remote ASR provider while keeping Hy-MT2 local, or the reverse. When no
selected capability uses `local`, the desktop client does not require or launch
the llama.cpp executable.

The Settings page includes two built-in OpenAI examples and an **Add online
API** action. The action creates a normal provider object in the override
document, so custom OpenAI-compatible services use the existing save,
validation, and reload flow rather than a second settings store.
