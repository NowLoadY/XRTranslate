# Qwen Audio streaming ASR

XRTranslate ships `qwen-audio-streaming` on Welcome with
`qwen-audio-3.0-asr-flash-streaming` and `qwen-audio-3.1-asr-flash-streaming`.
Both declare `streaming: cumulative` on the `dashscope-asr-websocket` connector.
Keys are entered on Welcome and remain in runtime. The endpoint and key must
belong to the same region; remote connections use `wss://`.

## Live audio and completed windows

The normal VAD mode opens a live turn as soon as speech is confirmed. Its
bounded pre-roll goes first, then newly captured PCM16 mono audio at 16 kHz is
sent while the person speaks. The connector waits for `task-started` and packs
incoming audio into 100 ms binary messages without replay pacing. Sending and
receiving run concurrently. Local silence, speaker changes, duration limits,
and explicit pause/finish controls close the input and send `finish-task`.

Intermediate sentence revisions produce cumulative snapshots through the
existing `asr_result/partial` event. Heartbeats and duplicate final sentence IDs
are ignored. Previews do not enter history, Corpus or TTS and never block audio
on a full result queue. Empty revisions retract only the active draft. A new
sentence cannot silently replace an unfinalized nonempty sentence: incomplete
provider responses fail instead of committing a transcript with missing text.
Sentence joins retain spacing for alphabetic scripts and compact Chinese and
Japanese text. After `task-finished`, the final transcript joins the same
language routing, quality checks, speaker metadata, sentence assembly,
translation and history path as ordinary recognition. Normal live completion
does not submit the same audio to ASR again; existing quality/language recovery
remains available for an abnormal result.

Each session caches one successfully finished WebSocket for the next turn.
Failed and cancelled tasks close their socket. If an idle connection has closed,
only task setup is repeated on a fresh connection, before any audio is sent.
There is no background probe or keepalive audio. The idle cache expires after
60 seconds; connection/start and recognition deadlines remain bounded.

The existing continuous-recognition option still selects overlapping,
revisable windows. Completed windows and diagnostic/recovery requests reuse
the very same WebSocket transport, pacing prerecorded input at 100 ms. Local
models and HTTP-only providers keep their existing paths. A model name alone
never selects live streaming; the catalogue capability and connector must agree.

## Local models and lifecycle

- Silero still gates speech, retains the onset, limits audio memory and avoids
  uploading long idle periods. Live streaming changes when audio is delivered,
  not who owns segmentation.
- GTCRN still supplies denoised frames to VAD and speaker analysis. Recognition
  keeps the original audio, matching the ordinary path.
- When speaker recognition is enabled, the local streaming speaker segmenter
  can close a turn on speaker change and attach its identity. The shared worker
  reuses that identity instead of running a second embedding for the same turn.
- Audio queues are bounded by the configured maximum turn plus pre-roll and
  overlap. The existing bounded job queue applies backpressure between turns.
  Language changes discard the old generation; pause/finish drains audio before
  the ordered completion fence. Scheduler permits transfer ownership even when
  cancellation races with a queued grant, so a cancelled request cannot leak
  an inference slot.

## Prompt and vocabulary semantics

The provider declares `asr_prompt_mode: context_bias`; it does not declare
semantic instruction-prompt support.

- The `ASR CONTEXT` Prompt Studio page renders unweighted lexical recognition
  context into `payload.input.context`. This text is a list of likely spoken
  terms, not an instruction to the model. It is bounded to 400 Unicode
  characters before Prompt Studio execution, so the displayed Request trace is
  the exact provider payload. A custom static composition above the limit is
  rejected rather than silently rewritten.
- XR Corpus vocabulary is independently converted to
  `payload.parameters.vocabulary` as structured `term -> weight` entries. It
  bypasses the text graph. The adapter validates the configured weight and
  filters optional Corpus terms to the provider's term-length, 2,000-entry,
  and super-hot-word limits; an unsuitable hint cannot fail the utterance.
- The `ASR PROMPT` page is used only by providers whose profile declares
  `asr_prompt_mode: instruction`. Its rendered text is delivered verbatim as a
  semantic instruction. It is never silently converted into Qwen recognition
  context or weighted vocabulary.

This distinction is part of the provider capability contract. Adding another
ASR provider must select `none`, `instruction`, or `context_bias` explicitly,
and must declare structured vocabulary support separately.

## Sentence ownership and service limits

The shared sentence assembler preserves an unfinished linguistic sentence
across duration-limited turns. Provider partials are display previews; local
resource limits are not treated as proof that a sentence is complete. Stable
translation and TTS remain owned by the shared sentence gate. See
[Qwen ASR](qwen-asr.md) for the common sentence behavior.

Provider pricing and account quotas apply to submitted audio. Bounded pre-roll,
overlap and quality recovery can add submitted duration. This is a separately
configured online service, not one of the free HTTP ASR presets.

Official protocol references, checked 2026-10-12:

- [Client events](https://help.aliyun.com/zh/model-studio/qwen-audio-asr-streaming-client-events)
- [Server events, including connection reuse after task-finished](https://help.aliyun.com/zh/model-studio/qwen-audio-asr-streaming-server-events)
- [Recognition context and vocabulary](https://help.aliyun.com/zh/model-studio/improve-asr-accuracy)
