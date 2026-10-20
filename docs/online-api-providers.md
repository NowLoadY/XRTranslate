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

## Free-tier translation presets

The following API options were checked against official documentation on
2026-10-11. Each requires the user's own API key, entered on Welcome. Free-tier
eligibility and quotas belong to the provider account; paid Google/Groq accounts
can bill the same model. One parallel slot keeps these presets conservative
without adding a separate provider scheduler.

| Provider ID | Listed model | Free API offering |
| --- | --- | --- |
| `siliconflow` | `tencent/Hunyuan-MT-7B` | Input and output are [priced at zero](https://siliconflow.cn/pricing). [Identity verification is required](https://docs.siliconflow.cn/docs/userguide/faqs/rate-limit-and-upgradation); model rate limits are fixed. |
| `google` | `gemini-3.5-flash-lite` | [Free-tier input and output](https://ai.google.dev/gemini-api/docs/pricing), subject to account/region eligibility and quotas. Google uses free-tier content to improve its products. |
| `groq` | `qwen/qwen3.8-27b` | [Free plan](https://console.groq.com/docs/rate-limits): 30 requests/minute, 1,000/day, 8,000 tokens/minute and 200,000/day. |
| `openrouter` | `openrouter/free` | [Zero-price free-model router](https://openrouter.ai/openrouter/free); a [free account](https://openrouter.ai/pricing) allows 50 requests/day. The actual model can change between requests. |

All four use `openai-chat`, including its existing SSE handling and bearer-key
authentication. They add no provider-specific execution branches. The
[Gemini compatibility endpoint](https://ai.google.dev/gemini-api/docs/openai)
uses `reasoning_effort: minimal`; the
[Groq Qwen model](https://console.groq.com/docs/model/qwen/qwen3.8-27b)
uses `reasoning_effort: none` for direct responses. These catalogue options apply
to translation and, for models declaring `supports_chat`, context-free `ask`.
OpenRouter keeps the serving model's
default and reserves a larger output budget for models that include reasoning.

Hunyuan-MT uses the existing single-user-message encoding. It is a
[specialized translation model](https://huggingface.co/tencent/Hunyuan-MT-7B),
so it declares `supports_chat: false`: general voice editing and vocabulary sentence
generation are unavailable. Contextual translation models can still use Ask for
bounded subtitle editing. This preset does not declare reference-context support.

## Free-tier ASR presets

Checked against official API and pricing pages on 2026-10-11. These presets
also require the user's own key on Welcome and use one parallel slot. Free
quotas are account limits, not a promise of unlimited continuous recognition.

| Provider ID | Listed models | Free API offering |
| --- | --- | --- |
| `groq` | `whisper-large-v3-turbo` (default), `whisper-large-v3` | The [free plan](https://console.groq.com/docs/rate-limits) lists 20 requests/minute, 2,000/day, 7,200 audio seconds/hour and 28,800/day for each model. |
| `siliconflow` | `FunAudioLLM/SenseVoiceSmall` | The [current price page](https://www.siliconflow.cn/pricing) lists it at zero; the [transcription API](https://docs.siliconflow.cn/docs/api/audio-transcriptions-post) lists this exact model. Identity verification and account rate limits apply. |
| `google` | `gemini-3.5-flash-lite` | [Audio input and text output have a free tier](https://ai.google.dev/gemini-api/docs/pricing), sharing this model's quota with translation. Free-tier content is used to improve Google's products. |

Groq uses the existing multipart connector and requests `verbose_json` to retain
detected language metadata. Its [Whisper prompt](https://console.groq.com/docs/speech-to-text)
provides lexical context rather than instructions; the preset keeps the shared
context budget at 200 characters. It does not claim structured hotword support.

SenseVoiceSmall uses the same connector with just model and audio fields, as
documented by SiliconFlow. It declares [Chinese, Cantonese, English, Japanese
and Korean](https://huggingface.co/FunAudioLLM/SenseVoiceSmall) and no language
parameter, prompt or weighted-hotword support. SiliconFlow's older retirement
announcement conflicts with its current model/price listing; the preset follows
the current listing, without an authenticated availability probe. Other ASR
names appearing only on the price page are not added without a matching API
contract.

Gemini uses the existing [audio chat-completions format](https://ai.google.dev/gemini-api/docs/openai)
and the shared ASR instruction graph. Its model card selects minimal reasoning,
the same way as text chat. There are no provider-name dispatch branches.

## TTS

TTS uses the same catalogue, Welcome configuration and runtime credentials.
Model cards declare language coverage, delivery, preset voices and cloning
support. Connectors normalize output to PCM16; one bounded worker owns playback
ordering and cancellation. Online synthesis does not load a local GPU runtime.
Preset selection does not create a clone or upload a local reference recording.

The built-in options are [Groq Orpheus](https://console.groq.com/docs/rate-limits)
(English and Arabic) and [Gemini TTS](https://ai.google.dev/gemini-api/docs/pricing)
(multilingual Flash-Lite and Flash). Both offer account-dependent free tiers,
checked on 2026-10-12. The OpenAI speech connector returns complete WAV audio;
the Gemini speech connector streams PCM. Their current connectors expose
preset voices; local models retain reference-based cloning. Provider errors
use the existing error notice without automatic service fallback.

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
| `openai-speech` | TTS | Speech JSON request; WAV audio |
| `gemini-speech` | TTS | Interactions speech request; SSE PCM audio |
| `openai-transcription` | ASR | WAV multipart upload; JSON transcript |
| `openai-audio-chat` | ASR | Base64 WAV in chat messages; final text |
| `dashscope-asr` | ASR | Native multimodal audio HTTP; final transcript |
| `dashscope-asr-websocket` | ASR | Live PCM input, cumulative previews and final transcript; completed windows use the same transport |

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
| `asr_language_hint` | Optional override for sending the multipart `language` field; absent preserves the connector default |
| `asr_response_format` | Optional multipart response format: `json` or `verbose_json`; absent uses the server default |
| `context_max_chars` | Optional ASR lexical-context limit |
| `vocabulary_bias` | Structured weighted hotword support |
| `single_user_message` | Encode rendered translation messages as one user message |
| `reasoning_effort` | Optional chat-completions reasoning level, applied to translation, `ask` and audio chat; absent means use the server default |

ASR models declaring `streaming: cumulative` on a live-audio connector send
frames during speech and emit partial snapshots. The normal VAD path selects
live input; the continuous-recognition option retains its overlapping window
path. Both share final result processing. HTTP-only ASR keeps `streaming: none`.
The shared live-ASR callback always carries a complete preview snapshot, never
a text delta. Empty snapshots retract the preview. Connectors own provider
sentence IDs, duplicate events and finalization; the pipeline commits only the
successfully completed transcript. Audio EOF drains queued frames and awaits
provider completion. Disconnects and cancellation must not promote a draft to
a final result. Adding a delta-based connector requires normalizing its deltas
to this snapshot contract inside the connector.
Unsupported streaming, instruction or weighted-vocabulary declarations fail
configuration validation instead of silently disappearing. See
[live ASR](providers/qwen-audio-streaming-asr.md) for audio ownership, bounded
queues, connection reuse and cancellation.
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

Multipart transcription preserves returned language metadata and normalizes it
through the shared language catalogue. In bidirectional mode, a transcript
without language metadata is accepted directly when existing script evidence
unambiguously selects one language of the pair. Conflicting or ambiguous output
still uses the shared recovery policy. Online HTTP connections remain pooled
for 90 seconds across speech pauses; local runtime pooling is unchanged.

Reversing a text conversation's language pair reuses its session and recent
bilingual history. Each request keeps its own source and target languages;
`ask` remains context-free. API failures carry the returned diagnostic into
the shared bottom notification pill (including SSE error payloads), with longer
text available on hover and in the log. A shared presenter adds localized hints
for common HTTP statuses and explicit diagnostics such as exhausted quota,
invalid keys, context limits, memory exhaustion and timeouts. Unknown errors
show the original diagnostic alone. These hints do not drop context, retry
requests or reopen Welcome; quick translation uses the same presenter.

To add a service using an existing format, add its provider/model records to
the catalogue and, if it should be a shipped preset, an empty provider entry in
`config.json`. Review configuration expansion; compile affected consumers when
the connector contract changes.
Do not add hash identities, local test servers, or provider-by-provider test
loops. See [Provider integration](providers/README.md) for ownership boundaries.
