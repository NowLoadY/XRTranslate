# Plugins and host features

Optional plugins are native child processes discovered under the application’s
`plugins/` directory. Each directory contains `plugin.json` and an executable;
filesystem presence defines installation. The host synchronizes directory changes
while running and checks only the declared minimum host version. Media,
Meeting, Agent, Vocabulary, SteamVR, OSC, Avatar, screen translation, Lyrics,
Big Walk and automatic input use this boundary.
Their source is absent from the desktop client's dependency graph;
removal stops their workers and removes their page and tools.

The [MIT protocol SDK](../crates/xrtranslate-plugin/README.md) owns the wire protocol
and process runner. `rust-client/src/plugin_host` adapts it to shared UI, CLI,
inference, model installation and application lifetime. Plugin manifests declare
contributions; the dispatcher forwards domain calls without interpreting them.
Cooperation belongs to consumers: Lyrics reads output requests from plugin state
and OSC consumes lyric events. Missing required capabilities produce a local error;
optional consumers report unavailability when used.

Separately distributed plugins can keep their source in private repositories;
the public SDK does not include their implementations. Release inclusion comes
from each manifest, independently of installation in a developer's environment.
Bundled plugin source lives in the independent workspace under `plugin-src/`.
It can move to a separate repository unchanged. The exported SDK supplies its
dependencies and build tool; the host build accepts finished plugin packages.
Plugins keep working across compatible host upgrades without being rebuilt.
They use host services without depending on the host's Rust compiler or UI build.

Installed manifests are the sole plugin page catalogue; no compiled plugin
controllers or static navigation registry remain. The host retains shared screen
capture/OCR, OSC transport and task services.

`crates/xrtranslate-client-core` holds the existing UI components and neutral
session/playback/file contracts once. The native Media, Meeting and Vocabulary
pages reuse them through the SDK's embedded adapter. On every platform, plugins
own layout and interaction while the host GPU draws their meshes and named shader
callbacks. Binary frames reuse unchanged drawings and carry lossless texture deltas;
embedded pages do not allocate a second GPU device or full-window render targets.
Shared 3D callbacks carry instance parameters while geometry and pipelines stay
in the host. UI and CLI invoke the same handlers. Video textures remain host-owned;
video pixels never cross the plugin pipe. The
host cancels a removed plugin's tasks and releases its playback resources.

Embedded pages are one presentation option. Small panels can use host controls;
games and intensive custom canvases can own a window and render directly with
their own engine. The host exchanges tasks and state with such a plugin, not
window frames. The SDK's window example demonstrates this option without adding
game rendering responsibilities to the host.

Media adopts the old `video_tasks.json` on first use; Meeting copies its old
SQLite database, including committed WAL content. Their records then live under
`<runtime>/plugins/data/<id>`. Vocabulary continues to use the shared bookmark
store, so words saved from translation history and the plugin have one source.
The Agent retains its conversation while the plugin remains installed and enabled;
configuration refreshes do not reset it.

SteamVR owns its original overlay worker, settings and page. Shared companion
geometry and animation live in the UI SDK; desktop and plugin surfaces sample
the same presentation. Generic surface state and voice-input events replace
direct access to the SteamVR controller. Removing the plugin clears that state
and ends its renderer without stopping core translation.

## Ownership and dependencies

| Owner | Responsibility |
|---|---|
| Shared infrastructure | Recognition, translation, audio, media import, language policy, persistence primitives, and reusable UI |
| Host | Compose capabilities and plugins, allocate shared resources, route actions and events, and coordinate application lifecycle |
| Plugin | Its domain state, settings semantics, persistence policy, workers, and presentation |

Shared infrastructure stays independent of concrete plugins. Shared requests and
events carry neutral data and opaque owners; adding a plugin does not introduce
plugin-named protocol variants or branches in the translation pipeline.

The SDK is a reusable dependency, not a requirement to execute every operation
in the host. Its base package owns process communication and lifecycle contracts;
UI integration and specialized platform adapters remain optional dependencies.
Plugins can reuse those libraries inside their own processes without requiring
the application to load a domain-specific runtime. A shared adapter is justified
by actual consumers and a stable responsibility, not by a possible future use.

The host owns shared application services and arbitration. A plugin owns its
domain decisions and dedicated workers, including optional external-device
connections. Shared adapters expose observed capabilities and resource lifetime;
they do not decide feature behavior, infer user intent, or assume a particular
plugin is installed. Resource acquisition and cleanup have one owner, including
when a plugin exits unexpectedly.

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
the requesting plugin. `translation.state` reports `Running`, `Stopped` or a
failure to that owner. History follows each task's `records_history` setting,
without a plugin allowlist.

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

## Installation and lifecycle

The manifest owns identity, navigation and tools. Configuration belongs to the
plugin; the host owns process enablement and resource cleanup.

Directory scans and process launch/retirement run on one serial I/O worker.
The host owns installed state and accepts completed launches before sending the
current environment. Abandoned launches and shutdown cleanup stay on that worker.
Navigation changes rebuild the page list; icons share immutable bytes, and tool
definitions are reused until installed contributions change. Protocol writers
sleep until output, cancellation or disconnection arrives; plugin task ticks
remain separate from rendering.

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
| Native `plugin-src/osc` / `chilloutvr-osc` | Chatbox composition, game wire profiles and original settings/page |
| Native `plugin-src/bigwalk` | Completed chat messages, bounded one-shot delivery and the local game bridge connection |
| Native `plugin-src/avatar` | Game selection, display composition, control intent and connections; uses shared OSC and exclusive live tasks |
| Client core `text_output` / host `system_metrics` | Shared formatting and banner controls / one system sampler |
| Native `plugin-src/lyrics` | Music observation, lyric lookup, synchronization, and bounded persistent cache; publishes text through the host to selected outputs |
| Native `plugin-src/vocabulary` | Wordbooks and saved vocabulary views; uses shared storage and stateless inference for examples |
| Native `plugin-src/meeting` | Meeting state, recording navigation, and persistence; consumes its normalized session results |
| Native `plugin-src/media` | Media tasks, playback, subtitles, dubbing and export; uses shared import and translation |
| Native `plugin-src/agent` | Persistent conversation, tool execution, context compression and progress |
| Native `plugin-src/vr-overlay` | SteamVR presentation and runtime; consumes shared host output |
| Native `plugin-src/ocr` | Screen-text deduplication, translation and status; uses shared region capture, OCR and task services |
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
Windows and Linux share one event-driven worker and one pending frame. The UI
keeps its last texture while rendering proceeds asynchronously; a completed frame
uses one GPU-local copy into that texture. Platform adapters own only the D3D11 /
D3D12 or OpenGL / Vulkan external-resource handoff, with matching adapters and
explicit GPU synchronization. Pixels never return to CPU memory. Hidden playback
consumes update notifications without polling, and worker/GPU work completes
before its resources are retired.

VRChat OSC and ChilloutVR OSC are independent executable plugins built from
the same library. They share composition, scheduling and UI; game profiles own packet
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

The host owns one system sampler and publishes `system.metrics`. Shared text
formatting and banner controls live in the UI SDK. Output plugins consume this
data without starting duplicate samplers. Shared OSC receive/OSCQuery remains a
host service; datagram destinations are owned and fenced per plugin, with final
cleanup packets on removal. Reviewed speech uses a tool advertised by an output
plugin, so the core voice controller has no chatbox implementation dependency.

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
