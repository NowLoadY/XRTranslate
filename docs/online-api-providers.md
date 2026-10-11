# Online API providers

The online catalogue is
[`crates/xrtranslate-assets/remote_catalog.json`](../crates/xrtranslate-assets/remote_catalog.json).
It owns service identities, regional endpoints, default request limits, model
lists, and exact model capabilities. Downloadable weights stay in the separate
`model_catalog.json`; online models have no installation or hardware records.

`config.json` selects capabilities and enables preset provider entries. An empty
preset object inherits its catalogue defaults. Welcome is the only service
configuration UI: users select a built-in provider and one of its listed models,
then enter the API key. There is no custom-provider action or endpoint editor.
The settings page does not contain a second service editor.

## Runtime credentials and releases

Development and installed builds use the same `runtime/user-config.json`.
Welcome saves model/provider selections and keys there, including partially
completed online setup. Writes replace the file atomically; a failure leaves
the previous file intact. Runtime is ignored by Git. Treat this JSON as private
local data; it stores keys as text, not in an encrypted credential vault.

The catalogue and source defaults contain no credentials. Compilation rejects
non-empty `api_key` fields in either embedded default document. Do not put
development keys in `config.json`, the catalogue, or source code.

`xrtranslate-installer export-defaults --output <path>` expands built-in service
options and clears API keys without reading runtime. Linux and Android release
scripts use this export; the desktop packager applies the same sanitization to
its config input. The binary also embeds the online catalogue, so releases
always carry the built-in model choices. Packaging never copies user runtime.
No Git-content hash comparison or API test request is involved.

The desktop updater preserves runtime user data while updating bundled runtime libraries. Android refreshes
its shipped `config.json` while leaving application-private runtime files
untouched. The next start merges runtime preferences over the new defaults,
retaining entered keys and selections while receiving new provider metadata.
Obsolete online-model migrations have been removed; no existing online users
need them. The independent runtime-location migration for older local-model
installations remains.

## Connector contract

A service name is an identity, not an execution branch. Configuration resolves
it to one of the implemented wire formats:

| Connector | Capability | Request and result |
| --- | --- | --- |
| `openai-chat` | Translation | Chat-completions messages; JSON or SSE text |
| `openai-transcription` | ASR | WAV multipart upload; JSON transcript |
| `openai-audio-chat` | ASR | Base64 WAV in chat messages; final text |
| `dashscope-asr` | ASR | Native multimodal audio HTTP; final transcript |
| `dashscope-asr-websocket` | ASR | Native task events and PCM frames; final transcript |

Any host implementing a format can reuse its connector. Only a new wire
format requires adapter code. `url` is the complete endpoint; it is not
restricted to an official vendor host. `model` is the service's exact model ID.
Credentials remain provider settings and never enter the catalogue.

New catalogue integrations may declare `connector` explicitly. Legacy `openai` transport
uses the capability and endpoint path to distinguish chat from multipart ASR;
`dashscope` and `websocket` resolve to the existing native audio connectors.
This compatibility resolution happens once in configuration. Adapters do not
guess a protocol from provider or model names.

## Model capabilities

The catalogue's `models` records use `connector` plus exact `model` identity.
The same model contract applies through regional endpoints or custom gateways.
Provider `services[].models` lists the model choices shown on Welcome. Unknown
revisions do not inherit capabilities through name-prefix matching.

The shared `capabilities` object has these fields:

| Field | Meaning |
| --- | --- |
| `streaming` | `none`, `delta`, or `cumulative` output |
| `supports_context` | Translation reference context |
| `asr_prompt_mode` | `none`, semantic `instruction`, or lexical `context_bias` |
| `context_max_chars` | Optional ASR lexical-context limit |
| `vocabulary_bias` | Structured weighted hotword support |
| `single_user_message` | Encode rendered translation messages as one user message |

All current ASR connectors consume completed audio windows and return a final
transcript. The WebSocket transport does not imply live audio ingestion or
interim results. Unsupported ASR streaming, instruction, or weighted-vocabulary
claims fail configuration validation instead of silently disappearing.
Translation with `streaming: none` uses a normal request even when the pipeline
asks for updates; final-result handling remains shared.

Known models supply their defaults. An explicit provider `capabilities` object
overrides only its supplied fields. For unknown models, legacy
`supports_prompt_context`, `asr_prompt_mode`, `asr_context_max_chars`, and
`supports_vocabulary_bias` remain accepted. Unknown chat translation models
retain the existing delta-stream default; other capabilities default to off.
Set `streaming: none` for services without SSE support.

ASR connectors share one `AsrOptions` input and one `AsrTranscript` output.
Instruction, lexical context, and weighted vocabulary stay separate. The
shared pipeline owns context composition, scheduling, cancellation, retries,
and result delivery. Translation uses the same Prompt Studio graph regardless
of host; connector options control only wire encoding and streaming assembly.

To add a service using an existing format, add its provider/model records to
the catalogue and, if it should be a shipped preset, an empty provider entry in
`config.json`. Review configuration expansion and compile affected consumers.
Do not add hash identities, local test servers, or provider-by-provider test
loops. See [Provider integration](providers/README.md) for ownership boundaries.
