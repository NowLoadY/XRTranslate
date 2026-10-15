# Built-in plugin architecture

Built-in plugins are statically linked Rust modules with stable IDs, explicit
ownership, and typed host integration. This document defines how plugins share
capabilities and participate in application lifecycle.

## Ownership and dependencies

| Owner | Responsibility |
|---|---|
| Shared infrastructure | Recognition, translation, audio, media import, language policy, persistence primitives, and reusable UI |
| Host | Compose capabilities and plugins, allocate shared resources, route actions and events, and coordinate application lifecycle |
| Plugin | Its domain state, settings semantics, persistence policy, workers, and presentation |

Shared infrastructure stays independent of concrete plugins. Shared requests and
events carry neutral data and opaque owners; adding a plugin does not introduce
plugin-named protocol variants or branches in the translation pipeline.

Plugins receive focused snapshots or capability handles and return typed actions
for host effects. They do not receive the root application object or access
another plugin's private state. The host connects collaborating plugins through
small contracts, as it connects the lyric text output to OSC.

## Translation and result contracts

`session_coordinator` is the shared integration boundary:

- `TranslationTask` captures input, language intent, and an optional plugin
  binding before startup. Text, live audio, and imported audio use this contract.
- `TranslationSessionPlugin` describes session ownership, output policy, and
  lifecycle requirements through `PluginSessionBinding`.
- `SessionEventSubscriber` consumes normalized `TranslationEvent` values for
  its accepted owners. Plugins do not consume raw network events or reconstruct
  results from visible UI history.
- `HostOutputSubscriber` consumes shared captions or committed translations for
  external presentation and output.

`TranslationInput::ExclusiveLive` acquires an owner-scoped lease for live input.
Admission rejects overlap with active, paused or pending live tasks, and ordinary
live requests cannot enter an existing exclusive lease. Cancellation stops only
the owning task; its lease remains until the owned workers have exited. The host
reports readiness from actual capture and backend state, and completion only
after cleanup. These are neutral input and lifecycle contracts, independent of
the requesting plugin.

Result semantics are shared by every consumer:

| Result | Meaning |
|---|---|
| Preview | Replaceable presentation state, separate from persisted results and irreversible output |
| Segment | Identified source text with optional translation and recognition metadata |
| ReplaceSegments | Complete ordered replacement of the affected results, including removal of superseded entries |
| StreamEnded | End of a stream span, without implying the whole task has finished |
| Finished | Typed completion, cancellation, or failure of the task |

Consumers preserve result identity and revision order. ASR-only results remain
valid. Source and translation previews update independently; only completed,
validated translations enter conversation context, TTS, or automatic insertion.
Sentence completion belongs to the shared pipeline for both streaming and
pseudo-streaming recognition. Plugins apply presentation policy to recognition
facts without changing their meaning.

## Registration and lifecycle

`PluginDescriptor` declares identity, navigation, settings contribution, and
default enablement. `PluginRegistry` manages the catalogue and persisted
preferences. A plugin may have no standalone page. Metadata does not own runtime
instances; the host explicitly registers typed instances, actions, subscribers,
and lifecycle hooks.

Each plugin owns activation, deactivation, cancellation, and worker cleanup.
Enablement and active work are separate: work starts when the consuming
capability needs it. Disabling removes unavailable navigation and either stops
owned work or reports why exclusive work must finish first. Shutdown releases
resources and joins owned workers. Persistent user data has a separate lifetime.

Queues and caches remain bounded. Blocking I/O stays off UI and event-delivery
paths. The shared scheduler owns model concurrency and workload priorities;
plugins do not create separate model pools. Natural completion drains accepted
work, while cancellation discards pending work for the cancelled owner without
affecting unrelated tasks.

## Feature ownership

| Module | Owns and consumes |
|---|---|
| `plugins::osc` | Shared OSC receive/OSCQuery transport and Chatbox presentation; consumes shared host output |
| `plugins::avatar` | Verified Avatar control intent, binding/reset state and typed actions; `avatar_host` composes owned translation sessions |
| `plugins::lyrics` | Music observation, lyric lookup, synchronization, and bounded persistent cache; publishes text through the host to OSC |
| `plugins::vocabulary` | Wordbooks and saved vocabulary views; uses shared storage and stateless ask for example work |
| `plugins::meeting` | Meeting state, recording, and persistence; consumes its normalized session results |
| `plugins::media` | Media tasks, playback, subtitles, and export; uses shared import and translation |
| `plugins::vr_overlay` | SteamVR presentation and runtime; consumes shared host output |
| `plugins::ocr` | Screen-text workflow; uses host capture, recognition, and translation capabilities |
| `plugins::auto_input` | Automatic text output; consumes committed results through a platform input capability |

Avatar control has settings and no page. Its control core is maintained in the
companion Avatar repository and pinned by the host. The OSC capability owns
discovery, receive sockets, snapshots and parameter writes; control demand is
independent of Chatbox-output enablement. Its bounded, revocable batch writes
provide send-time pacing without owning any caption format. Avatar caption
layout and bus encoding remain in the game-specific adapter; completed text
comes through the existing owner-scoped result subscriber. The adapter receives
verified snapshots and capability handles, while `avatar_host` reports actual
session outcomes.
An Avatar toggle neither starts a parallel translation pipeline nor changes
another session's ownership. Setup and compatibility belong to the
[Avatar connector guide](avatar-connectors.md).

Lyrics has no page. It runs only when available and requested by active OSC
lyric output. Its platform adapters use system media interfaces and distinguish
missing playback progress from a valid song-start position. The plugin owns
the presentation policy: synchronize music with an available timeline and
timed lyrics, otherwise show its title; other media supplies titles only.
The plugin owns the bounded text window, its music marker, and title/lyric
scrolling; the host's output budget includes the marker.
Read-only playback status is exposed in plugin settings. Cache maintenance
belongs to the lyric plugin, independently of display activity.

Quick translation and history archive also use the shared task and subscriber
contracts. Audio Studio and Prompt Studio remain core infrastructure, outside
the plugin catalogue. Their domain controllers use shared host capabilities and
UI components rather than duplicating pipelines or editor interaction logic.

## Extending the system

Add a stable descriptor, keep domain logic in the plugin module, and register
only the host actions, subscribers, and lifecycle hooks that the feature needs.
Reuse existing task and output contracts before introducing a new shared seam.
Provider connections, prompts, resource installation, and UI primitives retain
their existing owners.

Related contracts: [platform](platform-architecture.md),
[prompts](prompt-architecture.md), [UI](ui-architecture.md), and
[refactoring](refactoring-contract.md).
