# XRTranslate built-in plugin architecture

XRTranslate plugins are statically linked Rust modules. This avoids exposing a
Rust dynamic-library ABI while keeping feature ownership, host access, and
lifecycle explicit. A plugin has a stable string ID, contributes declarative UI
metadata, owns its domain runtime, and uses shared capabilities through neutral
typed contracts.

## Dependency rule

Recognition, translation, audio capture, and media import are shared
infrastructure. They must never import a concrete plugin or contain variants
named after one. Plugins configure and consume those capabilities; the host only
composes both sides.

```text
                         +--------------------+
                         |  network / audio   |
                         |   media_import     |
                         +---------^----------+
                                   |
                     neutral request/event contracts
                                   |
                 +-----------------+-----------------+
                 |                                   |
       +---------+----------+              +---------+----------+
       | host coordination  |<------------>| plugin controller  |
       | resource arbitration| typed action | and event adapter  |
       +--------------------+              +--------------------+
```

The neutral session contracts live under `session_coordinator`:

- `TranslationTask` captures the input, language selection, and optional plugin
  binding before startup. `TranslationInput` supports `Text`, live audio, and
  an audio file; all enter the host's `start_translation_task` path.
- `TranslationSessionPlugin` lets a plugin describe an active session through
  `PluginSessionBinding`; the binding carries an opaque owner, output policy,
  and lifecycle requirements. Text producers can use
  `PluginSessionBinding::text` and `TranslationTask::text` without supplying
  audio settings or implementing the active-audio-session trait.
- `SessionEventSubscriber::on_translation_event` receives normalized
  `TranslationEvent` results for an owner selected by `accepts_owner`. Blocking
  persistence belongs on a plugin-owned worker.
- `HostOutputSubscriber` receives captions after host history merging. External
  presentation plugins do not need a branch inside the event pump.
  `CommittedTranslation` also supplies validated, non-revisable translated
  segments with stable stream/turn/segment identity. Outputs that insert text
  consume this event once, rather than interpreting caption preview updates.
  `StreamCancelled` discards any queued output for an explicitly cancelled
  stream; natural `StreamEnded` preserves completed output awaiting delivery.
- `TranslationSessionOwner::Plugin` stores opaque plugin metadata. Adding a new
  session-using plugin must not modify the owner enum or network protocol.

Plugins supply content and language intent, then consume results. Backend
startup, recognition, translation providers, prompts, and connections belong to
the shared infrastructure. Plugins must neither import the network's
`SessionEvent` nor reconstruct translation results by scanning host UI history.
Concrete plugin imports are likewise forbidden in shared infrastructure.

### Result and lifecycle contract

`TranslationEvent` has four forms:

- `Segment(TranslationSegment)` delivers source text and an optional translation
  with stable segment identity and available recognition metadata.
- `ReplaceSegments` delivers a complete, ordered replacement batch for a
  stream's source or translation results. The shared adapter collects all
  parts and rejects stale revisions before publishing the batch. Consumers
  replace the relevant batch, including removing superseded rows.
- `StreamEnded` seals the current stream span; it does not imply that the whole
  task or conversation has finished.
- `Finished` carries `TranslationOutcome::Completed`, `Cancelled`, or
  `Failed(String)`. Plugins do not interpret connection-status strings.

Live ASR, including overlapping-window recognition, submits a sentence for
translation only after its ending and contents stay unchanged for 300 ms.
An unfinished tail remains recognition preview until the input ends. Source
snapshots invalidate superseded segment identities; unchanged completed
translations are retained without being requested or spoken again.
The host can display incremental translation previews, but only validated
completed translations enter plugin results, conversation context, and TTS.
Speech synthesis runs independently and sends each generated audio chunk to
playback immediately. Providers without streaming audio keep their normal
bounded text-chunk synthesis.

Language selection also carries neutral `TranslationOptions`. An optional
`additional_target_lang` is fixed for the task and does not follow automatic
direction changes of the main language pair. It is validated against the same
translation-model language catalogue. Each target uses its own conversation
context; plugins must not launch a second session to obtain the extra result.
Completed `additional_translations` carry their target language, translated
text, and terminology matches on the original segment identity. Main and
additional results share replacement, cancellation, and terminal semantics.
If the resolved main target matches the fixed additional target, the backend
skips the additional context preparation and translation, and emits only the
main result. The fixed selection and its independent history remain available
for when the main target changes again; skipped turns do not enter that history.
Targets are compared using canonical language identities, so Simplified and
Traditional Chinese remain distinct outputs. A fixed target matching only the
recognized source still produces its own terminology-processed recognition.

The explicit `asr_only` mode keeps one input language and bypasses model
translation. The final terminology-processed text still travels through the
normal translated-result field with the `asr_only` tag. Consumers must use
that field for insertion or display and must not substitute the raw source or
display both as if they were a bilingual pair. Empty translated previews do
not authorize raw-source fallback in this mode.

`HostOutputEvent::Caption` preserves both `additional_translations` and
`asr_only` after history merging. OSC attaches unlabelled additional
results to the same message, counts all languages against its existing
Unicode length limit, and keeps the same output rate and expiry rules.
SteamVR places each target in the same subtitle card, reserves visible rows
for additional targets, and shares this layout with its desktop preview.
Both plugins omit the source for tagged ASR-only output. These display rules
remain inside their plugin subscribers and renderers; shared translation
infrastructure does not contain plugin-specific formatting.

Each channel owns its result adapter and cancellation scope. Domain subscribers
receive results before host presentation policy is applied, so `PluginOnly`
tasks still receive complete results and terminal outcomes. `Host` additionally
enables the normal host presentation path. Cancellation invalidates queued and
late events; startup failures also use the same typed terminal contract.

Text requests capture their owner and language pair when accepted. Requests in
the same conversation/language/output-policy scope are serialized and reuse
shared context. A plugin can use `PluginSessionOwner::in_conversation` to keep
continuity across independently identified requests; otherwise each operation
is its own conversation. Context stays isolated between plugins and conversation
IDs. Acceptance does not mean translation has completed. Text consumers use the text and
lifecycle fields without assigning meaning to audio timing or speaker fields.

The capability is composed through typed plugin actions and explicit host
registration. The screen translation plugin receives recognized text, submits
it through `TranslationTask::text`, and consumes `TranslationEvent` with its
operation identity. It does not select translation providers, build prompts,
or manage connections. Successive OCR requests retain the same conversation;
new observations do not cancel accepted translations. A new capture region or
language selection cancels that conversation's requests and starts a fresh one.
OCR translations use host presentation and join the shared result bubbles;
the result window's OCR header contains only the current recognized text.

Screen capture and local text recognition live in `screen_capture`,
`ocr_capture`, and `ocr_runtime`. Their worker reuses the configured model,
deduplicates unchanged content, and publishes bounded revision-tagged results.
The latest identical image reuses its recognized text, including across region
revisions. Whitespace-only differences do not create a new translation, and a
brief empty observation does not clear the displayed subtitle. Recognition runs
on its own worker, samples the latest screen, and never blocks or gates the
shared translation queue. Its sampling interval includes recognition time.
Moving or resizing the frame pauses capture and invalidates old results while
retaining the capture session and loaded model. The host composes these
capabilities with the plugin; model resources use the
same catalogue, setup, download, and deletion lifecycle as other models.
Disabling OCR releases its worker, and resource deletion waits for release
without blocking the UI. OCR has no standalone page or separate plugin switch.
Enabling an OCR model on a supported desktop platform exposes its input and
region controls in the shared floating window. The translation page supplies
the language selection; translated results use the same host presentation.

Desktop subtitles and screen translation share `overlay_manager`,
`overlay_ipc`, and `overlay_native`. The window presents host state and emits
typed actions; it does not run capture or translation. The host applies the
same translation and input controls used by the main UI. Platform adapters
provide positioning and input regions, leaving transparent areas interactive
for the applications underneath. The capture frame excludes controls. Audio
captions and OCR results share a separate movable result window, initially
placed beside the controls. Its position and size stay where the user puts
them. If it overlaps the capture area, those pixels are excluded from
recognition; an occluded empty result preserves the previous translation.
OCR is an independent input switch. Editing or confirming its range changes
the frame presentation without changing OCR enablement or the other inputs.

The shared text composer copies each successfully completed host text request
to the clipboard, joining its translated segments in order into continuous text
with script-aware spacing rather than one line per sentence. Completion waits
for the result pump's terminal acknowledgement; previews, failures, cancellation,
and plugin-owned text requests never change the clipboard through this path.
History presentation continues through the same shared translation events.

On Windows the floating controls include an opt-in automatic-input output.
It starts disabled and delivers only committed translations produced after the
current editable foreground control gained focus in another application. Results
without a supported target are discarded; refocusing never replays old results.
The worker makes one delivery attempt and rejects focus changes after a result
was produced. It transfers the exact translated text, preserving spaces, line
breaks, and Unicode without adding separators or a submit keystroke. It also
preserves the clipboard. Disabling the output, stopping
translation, closing the floating window, or clearing history discards pending
input. The switch is a session preference and is not persisted across launches.

Android text selection and sharing enter through a windowless Activity and a
bounded foreground service. `android_text_actions` drives the same text-task
capability without a rendering loop, with conversation scope supplied by the
calling application. Only a matching, completed request is copied, without a
completion notification. Missing configuration reports a setup message without opening
the main UI. MainActivity and the service share leased native processes. Completed
background conversations retain a short, bounded idle lifetime; their connections
and backend lease are released together after inactivity.
Home-screen widgets consume completed host output through `HostOutputSubscriber`
and the same bounded result store used by the background service. They display
recent results; tapping opens the main translation page.

Active Android microphone streams share a microphone foreground service and
release it when the last input stops. Once recording has started, leaving the
UI does not interrupt capture, recognition, translation, or meeting storage.
The session event pump and meeting writers run independently of rendering;
the foreground service supplies the required system notification and wake lock.

### Recognition metadata is fact, not presentation policy

The shared recognition/translation path may publish neutral facts that several
consumers need, but it must not calculate a Meeting-, Player-, or OSC-specific
presentation. The current segment contract includes:

- stable turn and segment identity, segment order, and absolute source range;
- speaker identity, revisability, and continuous-window overlap;
- timing provenance (`utterance_window`, `estimated_text_partition`, or
  `merged_windows`) so a subtitle consumer knows whether a range was observed
  or inferred;
- the reason the recognition boundary was emitted (silence, adaptive silence,
  duration limit, speaker change, or input boundary).

A plugin decides how those facts become subtitle visibility, cue replacement,
export duration, meeting rows, or external captions. In particular, an
estimated text partition is not word alignment. Model-specific cosine distance
is also not exposed as speaker confidence: it is an internal clustering score,
not a calibrated probability. If a future recognizer provides genuine token or
word timestamps, add them as an optional neutral alignment contract rather than
embedding subtitle rules in the backend.

Speaker identity is part of the recognition result, not a plugin capability
toggle. Session plugins cannot enable or disable diarization. Presentation
plugins such as OSC may independently decide whether to render the supplied ID.

Translation conversation context is also shared infrastructure, never plugin
state. XR Corpus owns one bounded history per backend session. History is keyed
by stable logical speech-turn identity rather than subtitle rows: a Speak
utterance with several translation segments is committed once, and repeated
continuous-window revisions update the same turn instead of appending overlap.
Prompts may use neutral speaker identity, prior completed turns, and source
context surrounding the exact current segment, but plugins cannot inject,
retain, or reorder model history. This keeps Meeting, Player, OSC, and future
consumers on identical recognition and translation semantics.

### User-composable translation prompts

User-defined prompt composition is a shared translation capability, not
plugin state and not XR Corpus presentation policy. Keep the boundary in three
layers:

- XR Corpus selects and bounds neutral context facts. Its protocol may expose
  relevant terminology, recent bilingual turns, the previous overlapping
  revision, and source text surrounding the exact current segment. It must not
  store user templates, UI block ordering, arbitrary instructions, or
  provider-specific message roles.
- `xrtranslate-prompt` owns the shared prompt graph, validation, composition,
  and saved library. The host selects the active graph and passes it to text
  and audio sessions. Prompt Studio edits this same domain; plugins keep no
  separate prompt state.
- `xrtranslate-inference::translation::profile` selects the provider target and
  renders the graph with the current input, language pair, and shared context.
  Plugins do not construct provider messages. Reference context must remain
  distinct from the current input.

The host default composes directly from the structured `context_data` and
`prompt_terms` fields. The translation protocol does not carry a pre-rendered
prompt, so new composition code cannot accidentally reintroduce provider or UI
policy into XR Corpus.

### Scheduling is a shared infrastructure policy

Plugins never choose model thread counts, queue sizes, or concrete scheduler
implementations. A neutral session is classified as `realtime` or `offline`
from its lifecycle contract: live capture is latency-sensitive, while finite
media input is throughput-oriented. The backend schedules both classes against
the configured ASR and translation slot counts, prioritizes realtime work, and
periodically admits offline work so it cannot starve.

Queueing remains bounded in every mode. Natural EOF and an explicit graceful
finish preserve ordered results and drain queued work; user cancellation or a
task switch closes the session and discards work that has not completed. Do not
turn an overload error into a larger hidden queue, and do not add a
plugin-specific model pool to make one importer faster. Extend the neutral
workload/lifecycle contract when a genuinely different scheduling requirement
appears.

### Model providers and assets are separate extension points

Model assets are immutable package metadata; providers are runtime behavior.
Do not encode a model size as a new provider and do not scatter model IDs,
prompt rules, launch arguments, or UI labels through plugin and host code.

- `xrtranslate-assets` owns the manifest catalogue, active asset resolution,
  installation metadata, and preflight checks. Consumers query by stable asset
  ID or capability instead of matching fields such as “normal” and “big”.
  Runtime files are selected by declared role (weights, projection, and future
  roles), never by their position in a manifest array.
- `xrtranslate-config` resolves the selected provider's common local-runtime
  contract: endpoint, model asset, context window, output budget, and slots. It
  does not decide which concrete inference family implements that provider.
- The backend provider plan is the single composition boundary for model
  family support. It validates provider/asset compatibility and creates model
  servers and inference adapters. Pipelines consume the plan and never branch
  on provider names.
- `xrtranslate-inference::translation::profile` owns translation request
  profiles, sampling parameters, and output cleanup. ASR adapters live under
  `xrtranslate-inference::asr::providers`; both capability domains keep
  transport and authentication in the adapter.
- A provider may select a declared transport such as `local`, `openai`, or a
  provider-native `websocket`. Local routes resolve immutable assets and
  managed runtimes; remote routes resolve a model identifier and let their
  adapter implement the advertised wire contract. ASR and translation can
  independently be local, remote, or mixed without changing the session
  pipeline. Remote providers do not require a local model asset or
  llama-server executable.
- Desktop model selection is keyed by provider plus capability (and level when
  writing a choice). Provider setting fields use declarative descriptors;
  unknown configuration fields retain the generic editor fallback.

Adding another size or quantization for an existing family should normally be
a catalogue-only change (stable asset ID plus manifest). Adding a genuinely
different provider requires one inference adapter/profile and one backend
runtime-plan registration, plus any required manifest and configuration
descriptor. It must not
require changes to session plugins, the generic pipeline, or model-install UI
control flow. A backend architecture test requires every catalogued provider to
have a registered runtime profile, so the UI cannot expose an installable local
provider that the backend cannot start.

The detailed implementation path and ASR text-capability matrix are maintained
in [Provider integration](providers/README.md).

## Ownership boundary

The host owns capabilities shared by features or requiring exclusive access:

- backend process and translation-session allocation;
- microphone and system-audio capture;
- streaming media-audio import and resampling;
- navigation, the persisted application-settings envelope, localization entry
  points, and the shared UI kit;
- generic recognition/translation event delivery and user-visible errors.

A plugin owns its domain state, schema, domain persistence, workers, UI, and
assets. The host may persist a plugin's settings value, but the plugin owns that
value's meaning and migration. Plugin UI may update plugin-owned controller or
draft state directly; effects requiring host capabilities must be returned as a
typed action. Plugin UI must never receive `&mut XRTranslateApp`.
Shared UI components and focused host snapshots remain reusable: result
ownership does not require a plugin-specific copy of histories, controls, or
visualizations.

Graph-based plugin pages use `ui::graph_editor` as the single owner of editor
interaction semantics: graph switching, node/link selection, multi-node drag,
box selection, wire creation and atomic rewiring, cancellation, keyboard
commands, history stacks, viewport navigation, node placement, and navigation
hints. `ui::graph_canvas` owns domain-neutral geometry and painting primitives.
Prompt Studio and Audio Studio project their domain node/link identifiers onto
those shared modules; they may render different node bodies and validate
different graph rules, but must not introduce a second editor state machine or
copy the shared operations into a plugin-private controller.

```text
plugin UI/controller --typed action--> TranslationTask (text / live / file)
                                              |
                                      shared translation
                                              |
                         +--------------------+-------------------+
                         |                                        |
                  TranslationEvent                      host history / overlay
                         |                              (Host output policy)
                SessionEventSubscriber                            |
                                                         HostOutputSubscriber
```

## Metadata and runtime contracts

`plugins::PluginDescriptor` is declarative metadata used by navigation and
settings. It contains the stable ID, translated label key, ordering, icon, page
scroll policy, settings contribution, and default enablement.

`plugins::PluginRegistry` catalogues standalone plugin pages and their persisted
enablement preferences. Embedded inputs retain stable plugin IDs and translation
bindings, but use their shared capability's configuration and UI instead of a
page entry or a duplicate enablement switch. The registry is not a polymorphic
runtime container. Concrete plugin instances remain in
their modules and the statically linked host adapter still registers page
rendering, settings rendering, session bindings, subscribers, and lifecycle
hooks explicitly. This explicit composition is intentional until all plugins
share a real behavior seam; metadata alone must not pretend to remove typed
runtime dispatch.

Core Studio infrastructure is owned outside the plugin catalogue:

- `ui::pages::prompt_studio` opens a read-only style card picker. Saved style texts
  come from the existing prompt library; selection changes only the active graph's
  bound style node and uses the existing save/session-update path. The graph editor
  and its overlays live in `prompt_studio/editor`, while card presentation and
  selection live in `prompt_studio/style_picker`. Hover and selection reuse the
  shared animation system. The editor entry and mutation boundary require both
  beta updates and a manually created `runtime/debug.md` whose first line is exactly
  `hello` (LF and CRLF are accepted). Revoking either condition returns to cards.
  The marker is never created by the app, is ignored by Git, and is rejected in
  release staging. Returning to Prompt Studio from another page opens the cards.
- `audio_studio`: the default audio graph, saved user graphs, validation,
  core persistence, and routing controller. `ui::pages::audio_studio` owns its
  page, alongside `ui::pages::prompt_studio`. It compiles its graph into the
  neutral host audio route contract; the host owns microphone, system-loopback,
  application process-loopback, render-output, and bounded TTS PCM routing.
  A system-audio node stores a typed endpoint/application capture choice rather
  than encoding applications as fake devices. Shared audio code never imports
  Audio Studio node types. Optional VoiceMeeter metadata appears in
  the host snapshot only when the host found the installed Remote API. The
  graph stores only the selected B1/B2/B3 intent; Windows registry discovery,
  DLL lifetime, strip control, and restoration remain host responsibilities.
  The only built-in graph contains recognition, microphone, application audio,
  TTS, monitoring and app-microphone paths. Users can create, duplicate, rename,
  switch and delete their own graphs. Exactly one graph supplies the host route;
  switching retains each graph's edits, and Save persists the entire collection.
  Graph replacement and deletion require an inline confirmation. The default
  graph cannot be renamed or deleted. Schema 6 preserves schema 5's existing
  graph and adds inactive user graphs; older unsupported documents retain their
  existing reset-to-default migration. Structural validation decides whether a
  route can run. A separate risk report
  follows active graph paths, classifies blocking feedback and non-blocking
  contamination/acoustic risks, and maps every diagnosis back to its nodes and
  links so the graph remains the explanation of the real audio path.
  Audio-route lifecycle is independent from recognition/translation lifecycle:
  Audio Studio reconciles render outputs separately from the core ASR input;
  enabling or disabling the ASR path starts or stops translation. The selected graph synchronizes its
  ASR branch immediately when its source, device, application, or relevant link
  changes; enabling the real-time render route is not a prerequisite.
  Mixer nodes allocate one stable socket per connection plus one empty socket
  for the next connection. The on/off state belongs to the link, so the canvas,
  validator, risk analyzer, compiler, and Translation-page projection all read
  the same routing fact instead of maintaining a page-specific mode flag.
  Translation state shown in Audio Studio is a read-only host snapshot. If an enabled route selects a VoiceMeeter
  endpoint, the host starts VoiceMeeter automatically and shuts it down later
  only when that process was originally started by XRTranslate.
  The selected graph is the source of truth for ASR when Translation starts. A
  valid ASR branch and the Translation page consume one typed host input
  selection. Application identity is stable and visible on the Translation
  page; a live PID is resolved from automatic discovery only when capture
  starts. The page must never display an endpoint while the executor is actually
  using an application target. ASR input changes are rejected while translation
  is running rather than silently changing only the future or displayed state.

Current plugin ownership is:

- `plugins::auto_input`: embedded automatic text output, bounded worker handoff
  and identity deduplication, and a `HostOutputSubscriber`. It has a stable ID
  but no standalone page. Its worker is started lazily, sleeps while disabled,
  releases its native adapter on disable, and is stopped and joined on drop.
  `focused_input` supplies platform focus validation and Unicode insertion;
  neither layer starts translation or reads UI history. Unsupported platforms
  do not expose the floating-window toggle.
- `plugins::osc`: OSC settings, UDP listener/writer, caption formatting,
  preview/settings UI, mute-state capability, and a `HostOutputSubscriber`.
  The Translation and OSC pages reuse the same text composer. Translation
  requests from either page belong to the host's shared text task; OSC does not
  create a separate translation session. OSC projects the host's main language
  selection (including ASR-only mode) and returns typed route-change actions to
  edit those same controls. Its composer has no independent language state;
  the fixed extra target remains configured by the shared Translation controls
  and is not an extra selectable main target. Language edits are applied before
  a text submission from the same frame. Legacy persisted OSC typing-language
  fields remain readable for compatibility but no longer control submissions.
  Its optional direct-message mode
  sends the typed text through the OSC output without translation.
- `plugins::meeting`: meeting store, controller, recording, meeting UI, a
  `TranslationSessionPlugin` binding, and a non-blocking
  `SessionEventSubscriber` that persists normalized results. It uses
  `PluginOnly` output and requests host-owned `media_import` for files.
- `plugins::player`: media tasks, playback, subtitles, player UI, and a
  `TranslationSessionPlugin` binding with `Host` output. Its
  `SessionEventSubscriber` queues normalized results for subtitle updates,
  filtered by the active operation. It uses the same host-owned `media_import`
  capability for transcription.
- `plugins::vr_overlay`: SteamVR overlay runtime, rendering, settings, and UI.
  Its `HostOutputSubscriber` consumes shared captions; it does not start a
  separate translation pipeline.
- `plugins::ocr`: screen-text state and an owner-filtered
  `SessionEventSubscriber` for completion and failure. The host supplies recognized
  content and submits its text binding with `Host` output to the shared translator,
  which publishes translations to the shared result bubbles. Capture
  and local recognition use the shared resource and worker lifecycle described
  above.

Disabling always hides the plugin page and normalizes navigation. A plugin with
in-flight exclusive work rejects disablement until the work ends. Runtime
activation is capability-specific: OSC activates/deactivates its network
output, while idle Meeting/Player state remains constructed and performs no
active capture/translation work. Every plugin must document whether an idle
worker remains alive and how shutdown joins or drains it.

## Adding another built-in plugin

1. Create `rust-client/src/plugins/<id>/`. Keep its domain model, controller,
   persistence, workers, UI, tests, and assets beneath that boundary.
2. Add a descriptor and stable lowercase `PluginId`. IDs are persisted and must
   never be reused for a different feature.
3. Expose host-dependent UI effects as typed actions. Accept only a focused
   snapshot or capability handle; never accept `&mut XRTranslateApp`.
4. Have the host action adapter submit a `TranslationTask` for translation
   input. Supply an opaque owner and `PluginSessionBinding` when the plugin
   owns the operation; inputs contributing to the main translation task use
   host ownership, as the shared text composer does. Use the text constructors
   for extracted or typed text; active audio-session plugins implement
   `TranslationSessionPlugin`. Choose an operation identity that matches the
   intended conversation lifetime. Do not add plugin-specific fields to shared
   requests or network events.
5. For task results, implement `SessionEventSubscriber`, filter by owner, and
   handle segment replacement and all terminal outcomes. For host captions,
   implement `HostOutputSubscriber`. Register the adapter in the host
   composition list; do not branch on concrete plugins in the generic event
   pump or depend on a visible history panel.
6. Register the statically typed runtime instance, page/settings renderer, and
   lifecycle hooks in the host adapter. These are currently explicit because
   plugin UI/action types are intentionally not erased behind `Any` or a broad
   catch-all command enum.
7. Define activation, deactivation, busy-disable, and shutdown behavior,
   including in-flight work, worker joins, and persisted configuration.
8. Add descriptor/ID migration tests, session-binding and subscriber tests when
   applicable, plus enable/disable/re-enable/shutdown lifecycle tests.

An independently distributed plugin ABI, sandbox, permission manifest, and
version negotiation remain out of scope. Those require a separate process
protocol rather than arbitrary Rust dynamic-library loading.

Architecture cleanup and plugin work must also follow the invariants and
extraction gates in [the refactoring contract](refactoring-contract.md).
