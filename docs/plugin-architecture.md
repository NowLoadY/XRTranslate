# Plugins and host features

Optional plugins are native child processes discovered under the application’s
`plugins/` directory. Each directory contains `plugin.json` and an executable;
filesystem presence defines installation. The host synchronizes directory changes
while running and checks only the declared minimum host version. Motion, Media,
Meeting, Agent, Vocabulary, Lyrics, Big Walk and automatic input use this boundary.
Their source is absent from the desktop client's dependency graph;
removal stops their workers and removes their page and tools.

The [MIT protocol SDK](../crates/xrtranslate-plugin/README.md) owns the wire protocol
and process runner. `rust-client/src/plugin_host` adapts it to shared UI, CLI,
inference, model installation and application lifetime. Plugin manifests declare
contributions; the dispatcher forwards domain calls without interpreting them.
Cooperation belongs to consumers, such as `plugins/osc/integrations.rs` connecting
lyric events to OSC outputs. Missing required capabilities produce a local error;
optional consumers report unavailability when used.

Motion source is maintained in a separate private repository in the ignored
`private-plugins/` directory. Public releases do not compile or package it.
Other independent plugin source lives under `plugin-src/`. These plugins need the
host; independence does not require a second application or UI implementation.

OCR and avatar/OSC/VR output still have compiled host bindings. The static feature
registry remains separate from the installed-plugin catalogue until those
resource and connection lifecycles are migrated.

`crates/xrtranslate-client-core` holds the existing UI components and neutral
session/playback/file contracts once. The native Media, Meeting and Vocabulary
pages reuse them through the SDK's egui surface: input enters the plugin; drawing
and the Director registry return to the host. UI and CLI invoke the same handlers.
Video textures remain host-owned; video pixels do not travel through JSON. The
host cancels a removed plugin's tasks and releases its playback resources.

Media adopts the old `video_tasks.json` on first use; Meeting copies its old
SQLite database, including committed WAL content. Their records then live under
`<runtime>/plugins/data/<id>`. Vocabulary continues to use the shared bookmark
store, so words saved from translation history and the plugin have one source.
The Agent retains its conversation while the plugin remains installed and enabled;
configuration refreshes do not reset it.

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
small contracts, as it distributes lyric text to selected outputs.

Director exposes plugin-owned operations through `ToolProvider`. The native CLI
and the headless Agent plugin share this contract; the dispatcher only discovers
and forwards calls, without interpreting plugin tool names. UI actions and
semantic tools reuse the same domain state and operations. The Agent owns its
execution environment; domain working documents remain with their plugins.
Providers are discovered through a paged catalogue; full schemas are requested
only when needed. Ordinary plugin page controls already use the shared UI
provider and do not need duplicate plugin-specific commands.
See [Director CLI and Agent design](design/director-agent.md).

## Translation and result contracts

`session_coordinator` is the shared integration boundary:

- `TranslationTask` captures input, language intent, context, and an optional
  plugin binding before startup. Text, live audio, and imported audio use this contract.
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

### Task context

The input owner selects `TaskContext`. A `Snapshot` supplies the current reference
facts; a `Conversation` supplies an explicit scope and optional initial facts.
`ContextFacts` contains bilingual turns and surrounding source text, consumed by
the existing Prompt Studio context nodes. Output visibility does not affect
context. A reusable conversation transport keeps its declared scope alive;
snapshot transports can be reused, while each task starts with fresh vocabulary
activation and its own reference facts.

| Input owner | Context scope |
|---|---|
| Live translation | One conversation shared by its audio and text inputs |
| Meeting | The plugin selects recent statements and nearby source from the meeting, preserving speaker boundaries; resumed recording keeps recent history, while audio reprocessing starts fresh |
| Media files | The media plugin manages context within each translation task |
| Live media streams | One translation run |
| Authored subtitles | Current cue's timeline neighbors and completed translations with their actual language pairs, read when submitted |
| Screen and quick translation | Current text snapshot |
| Avatar control | Its owned live operation |

The owner decides when work starts, resumes, or clears. The runtime maintains
bounded completed conversation turns directly in the inference path; it does not
wait for UI events or interpret plugin IDs. Domain records remain owned by their
plugins. Subtitle edits affect the next snapshot; seeking the player does not
change translation context. Output subscribers do not maintain translation
memory. Meeting references use bilingual records where the actual route is known;
legacy or untranslated records contribute source text without repeating those pairs.
Conversation startup source gives way to newly completed turns. Prompt
Studio shows the current request's runtime facts read-only while private results
remain outside host presentation.

Media files can produce translations while recognition continues. The media plugin
owns subtitle context and preserves authored subtitle text and timing. Media work
keeps its context separate from live translation.

Meetings preserve recognized statements while translations catch up. Live meetings
use a short, limited wait for nearby statements; imported recordings proceed at the
available processing speed. Ending a meeting finishes accepted translations before
marking it complete. Meeting context remains separate from live translation.

Online inputs keep the same plugin boundaries. Meeting setup accepts live audio
and RSS live items; the media import page accepts recorded episodes and finite
online media. Each page owns its selection and rejects unsupported content in
place. There is no shared entry page or automatic plugin routing. RSS discovery,
HTTP range reads and streaming decoding live in `media_import`, independently of
either plugin. Online recordings use the existing media subtitle job; online
meetings use the existing meeting recognition and translation jobs. Input EOF
completion is independent of inference priority, so a live network source can
drain cleanly without becoming offline work. See [online audio](online-audio.md).

## Compiled feature registration and lifecycle

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
| `plugins::osc` | Native Chatbox composition and game wire profiles; exposes the shared OSC receive/OSCQuery transport |
| `plugins::game_plugins` | Avatar game connection enablement and page composition |
| Native `plugin-src/bigwalk` | Completed chat messages, bounded one-shot delivery and the local game bridge connection |
| `plugins::avatar` | Game display composition, verified control intent and connection lifecycle; `avatar_host` composes owned translation sessions |
| `plugins::text_output` | Internal shared text formatting, banner controls and one owned system sampler |
| Native `plugin-src/lyrics` | Music observation, lyric lookup, synchronization, and bounded persistent cache; publishes text through the host to selected outputs |
| Native `plugin-src/vocabulary` | Wordbooks and saved vocabulary views; uses shared storage and stateless inference for examples |
| Native `plugin-src/meeting` | Meeting state, recording navigation, and persistence; consumes its normalized session results |
| Native `plugin-src/media` | Media tasks, playback, subtitles, dubbing and export; uses shared import and translation |
| Native `plugin-src/agent` | Persistent conversation, tool execution, context compression and progress |
| `plugins::vr_overlay` | SteamVR presentation and runtime; consumes shared host output |
| `plugins::ocr` | Screen-text workflow; uses host capture, recognition, and translation capabilities |
| Native `plugin-src/auto-input` | Automatic text output; consumes committed results through a platform input capability |

Desktop fullscreen history exposes an explicit lookup action on selected words
and short phrases. Its in-app card shares the quick-translation task lifecycle
and vocabulary store. Selection keeps a snapshot of the chosen text and its
sentence while the live feed continues; lookup language changes stay local to
the card. The first bookmark goes directly to the default wordbook, with other
wordbooks available from the saved star. Saved examples retain their own
sentence language so older examples keep the correct translation direction.

`audio_playback` owns local audio decoding, seeking, and output. Media and meeting
plugins use it independently. Meetings retain each recording run's time offset
and source file so transcript navigation resolves to the matching audio track;
the plugin owns playback selection and transcript following.

The shared MPV backend owns its playback state snapshot and video worker.
The native media plugin controls a resource allocated to its process by the host.
UI reads consume observed state and playback actions are queued asynchronously.
Video work stays bounded, and the worker completes before its target resources
are retired. Hidden playback continues without holding UI graphics resources
while waiting for the playback clock.

VRChat OSC and ChilloutVR OSC are independent registered instances of the same
plugin. They share composition, scheduling and UI; game profiles own packet
arguments and text limits. Settings and manual input remain per output. The host
allows one publisher per configured endpoint and transfers ownership before
enabling another, preventing delayed clears from the old writer. The host owns
one OSC receive/OSCQuery service; Chatbox writers never open duplicate listeners.
See the [OSC Chatbox guide](osc-chatbox.md) for setup.

**Game Plugins** is the Avatar connector page. Its **Enable** switch, off by
default, activates the game connection and owned session. Big Walk is a separately
installed native plugin with its own page, settings and process lifetime.

Big Walk consumes committed translations with their stable result identity and
audio provenance. It reuses text-output formatting and the shared text composer.
Its worker sends each accepted message once; connection and game-session changes
discard pending messages. The game adapter owns native API calls on the game
thread. See [Big Walk chat](bigwalk-chat.md) for setup.

Avatar connectors use the Game Plugins page for connection and
header/message/footer composition. Their single companion SDK owns the
connector catalogue, pure lifecycle, region
layout and buffered text codecs. Shared OSC transport owns discovery, verified
snapshots and revocable paced writes independently of Chatbox enablement.
WebSocket sends one latest display snapshot separately from ordered controls.

`TextOutputPlugin` is an internal capability plugin, composed explicitly by the
host rather than exposed as another output destination. It owns the shared
system sampler and joins it on shutdown. OSC and Avatar receive cloneable
`TextOutputHandle` values and use its public formatting and editor components;
neither owns or reads the other's private runtime or settings.

Avatar consumes the existing host output subscription. It retains message
identity and expiry, accepts native previews and promotes completed text for
buffered displays. The host supplies bounded recent candidates; connector layouts
own fitting to their objects. Cancellation clears affected messages; connection
loss clears the entire display. Header/footer visibility is independent of
translation activity. The [Game Plugins guide](avatar-connectors.md) owns setup and connector rules.

Lyrics has no page. It runs when requested by an active output with a lyric
region. One worker observes playback and looks up lyrics; the host supplies
output IDs, text budgets and timing offsets. The worker publishes title/artist
and lyric text per output and clears removed subscribers. Platform adapters
distinguish missing playback progress from a valid song-start position.
Presentation and bounded cache ownership remain in the lyric plugin; offsets
do not alter shared playback, song metadata or cached cues. Read-only playback
status and cache maintenance belong to its settings contribution.

Quick translation and history archive also use the shared task and subscriber
contracts. Audio Studio and Prompt Studio remain core infrastructure, outside
the plugin catalogue. Their domain controllers use shared host capabilities and
UI components rather than duplicating pipelines or editor interaction logic.

## Extending the system

For a native plugin, declare its manifest, implement the SDK and copy its built
directory into `plugins/`. UI, tools and model cards come from that declaration;
no host registration is required. Keep domain logic in the plugin and reuse
existing services and events before introducing a new shared seam.
Provider connections, prompts, resource installation, and UI primitives retain
their existing owners.

Related contracts: [platform](platform-architecture.md),
[prompts](prompt-architecture.md), [UI](ui-architecture.md), and
[refactoring](refactoring-contract.md).
