# Avatar connectors

**Settings → Plugins → Avatar translation controls** connects game components to
XRTranslate. Select the game connector, enable the plugin, and install its
[XRT-Avatar-Plugin component](https://github.com/NowLoadY/XRT-Avatar-Plugin).
The plugin is disabled by default.

| Connector | Game interaction | Host transport |
|---|---|---|
| VRChat | Expressions menu | OSCQuery profile |
| ChilloutVR | CCK Advanced Avatar Settings | OSCQuery profile |
| Overte | Avatar script and Tablet button | Paired local WebSocket |
| Resonite | Native controls through ResoniteLink | Paired local WebSocket bridge |
| Second Life | LSL attachment | HTTPS relay and local WebSocket bridge |

These are development-preview integrations. Platform setup and game-client
verification are maintained in the companion guides. ResoniteLink requires an
eligible session hosted by the user. Second Life requires an HTTPS relay.

## Setup and use

Finish XRTranslate setup and select languages, input and recognition settings.
Configure desired host outputs. For VRChat and ChilloutVR, enable OSC in the game;
for WebSocket connectors, copy the address and pairing token into the local
component or bridge using its guide. Pairing credentials are runtime values and
are not saved into shared game assets.

Wait for synchronization and Off, then operate the game control. Starting means
startup is pending; Active requires capture and backend readiness. Off cancels
pending startup and running work, with Stopping retained until cleanup finishes.
Read connection diagnostics in plugin settings and session failures in host
notifications. Reconnection and connector changes start with the control Off.

**Send captions to the game component** enables owner-scoped text delivery
to prepared VRChat and ChilloutVR caption objects and WebSocket game components.
Each connector discovers the component's display capability; its renderer determines
visibility. Existing OSC Chatbox, overlay and TTS settings remain owned by their
respective host plugins.

## Ownership and configuration

`plugins::avatar` owns connector selection, validated intent and feedback.
`avatar_host` turns shared controller effects into owned translation tasks.
`session_coordinator` owns input acquisition, task identity and cancellation.
The companion `xrt-connector-core` supplies the connector catalogue, parameter
contracts, text codecs and one pure lifecycle. The host pins that single SDK.

Each accepted request holds an exclusive lease on its selected live inputs.
Overlapping active, paused or pending tasks are preserved and the request is
rejected. Cancellation affects the requesting owner and retains its lease until
workers finish. Switching connectors, losing the connection, disabling the
plugin or turning off a required desktop input ends that operation.

Languages, source and recognition profiles are captured when requested; device
and prompt settings are resolved when the session is created. TTS and existing
output settings follow their owners' runtime rules. Host output subscriptions
filter by exact session owner. Old connection traffic and late worker callbacks
cannot activate a later binding.

OSC and WebSocket are transport capabilities. Their workers own sockets,
verification, bounded queues and cancellation. OSC peer discovery rules and
parameter maps come from the companion SDK; `plugins::avatar::osc_caption`
only schedules its encoded frames using the existing transport lease.
Game-specific rendering and wire details are maintained in the companion repo.
Chatbox retains its own destination.

`plugins::avatar::display::TextDisplay` accepts plain-text replacement and clear,
with an opaque stream/result identity and a renewable lifetime. It has no
translation, lyric or clock message variants. The host chooses the content
producer; connectors only adapt the text to their display capability. A producer
refreshes content while it should remain visible and clears when relinquishing
its output. Clearing revokes pending older output; a buffered replacement also
supersedes the previous transfer.

The translation subscriber separately validates its session owner and extracts
normalized results. Native text accepts revisions; buffered displays prefer the
latest completed sentence. An empty authoritative replacement clears either
transport. Repeated text renews its lifetime without restarting an unfinished
transfer. Existing lyric and clock routing keeps its own host settings; those
producers can use the same display interface when selected for an Avatar.

The [plugin architecture](plugin-architecture.md) owns host dependency rules;
the companion [connector protocol](https://github.com/NowLoadY/XRT-Avatar-Plugin/blob/main/docs/connector-protocol.md)
owns wire fields and client requirements. Existing `vrc_avatar` preferences are
accepted as an alias for the canonical `avatar` plugin identity.
