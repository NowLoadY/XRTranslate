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

- `TranslationSessionPlugin` lets a plugin describe an active session through
  `PluginSessionBinding`; the binding carries an opaque owner, output policy,
  and lifecycle requirements rather than a Meeting/Player enum variant.
- `SessionEventSubscriber` receives the generic `SessionEvent` stream. A plugin
  adapter must enqueue blocking persistence work on its own worker.
- `HostOutputSubscriber` receives captions after host history merging. External
  presentation plugins do not need a branch inside the event pump.
- `TranslationSessionOwner::Plugin` stores opaque plugin metadata. Adding a new
  session-using plugin must not modify the owner enum or network protocol.

The dependency rule is intentionally stronger than “the network happens not to
call a plugin today”: concrete plugin imports are forbidden in shared
infrastructure.

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
- Shared host/inference configuration owns the user's composition: enabled
  block IDs, ordering, per-block limits such as the most recent N turns, and
  editable text blocks. Meeting, Player, OSC, and future plugins all consume
  the same resolved composition rather than maintaining separate prompt state.
- `xrtranslate-inference::translation::profile` applies the resolved reference
  context to each provider's required system/user message shape and retains the
  non-editable current-input/output boundary. A custom block must never be able
  to relabel historical or surrounding text as the current input.

The host default composes directly from the structured `context_data` and
`prompt_terms` fields. The translation protocol does not carry a pre-rendered
prompt, so new composition code cannot accidentally reintroduce provider or UI
policy into XR Corpus.
Do not introduce a template trait or editor abstraction until the shared
composer and its first UI consumer are implemented together.

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

```text
audio/backend session
        |
        v
typed SessionEvent -----> SessionEventSubscriber(s)
        |
        +---------------> host history / overlay
                                  |
                                  v
                         HostOutputSubscriber(s)

plugin UI --typed action--> host capability command
```

## Metadata and runtime contracts

`plugins::PluginDescriptor` is declarative metadata used by navigation and
settings. It contains the stable ID, translated label key, ordering, icon, page
scroll policy, settings contribution, and default enablement.

`plugins::PluginRegistry` is a catalogue plus persisted enablement preferences;
it is not a polymorphic runtime container. Concrete plugin instances remain in
their modules and the statically linked host adapter still registers page
rendering, settings rendering, session bindings, subscribers, and lifecycle
hooks explicitly. This explicit composition is intentional until all plugins
share a real behavior seam; metadata alone must not pretend to remove typed
runtime dispatch.

Current ownership is:

- `plugins::osc`: OSC settings, UDP listener/writer, caption formatting,
  preview/settings UI, mute-state capability, and a `HostOutputSubscriber`.
- `plugins::meeting`: meeting store, controller, recording, meeting UI, a
  `TranslationSessionPlugin` binding, and a non-blocking
  `SessionEventSubscriber`. It requests host-owned `media_import` for files.
- `plugins::player`: media tasks, playback, subtitles, player UI, and a
  `TranslationSessionPlugin` binding. It uses the same host-owned
  `media_import` capability for transcription.

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
4. If it uses recognition/translation, implement `TranslationSessionPlugin` and
   return a `PluginSessionBinding`. Do not add a plugin-specific session-owner
   variant or field to `SessionConfig`/`SessionEvent`.
5. If it consumes results, implement `SessionEventSubscriber` or
   `HostOutputSubscriber` and register the adapter in the host composition
   list. Do not add a concrete-plugin branch to the generic event pump.
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
