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

**Send captions to the game component** enables owner-scoped text delivery for
WebSocket connectors. Their renderers determine visibility. Existing OSC Chatbox,
overlay and TTS settings remain owned by their respective host plugins.

## Ownership and configuration

`plugins::avatar` owns connector selection, validated intent and feedback.
`avatar_host` turns shared controller effects into owned translation tasks.
`session_coordinator` owns input acquisition, task identity and cancellation.
The companion `xrt-connector-core` supplies one pure lifecycle for all connectors.

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

OSC profiles share discovery, authoritative reset queries and sockets with the
OSC transport owner. They do not alter the Chatbox destination. WebSocket games
share a bounded local transport, session fencing, reset acknowledgements and
heartbeat checks. Platform-specific bridge details stay in the companion repo.

The [plugin architecture](plugin-architecture.md) owns host dependency rules;
the companion [connector protocol](https://github.com/NowLoadY/XRT-Avatar-Plugin/blob/main/docs/connector-protocol.md)
owns wire fields and client requirements. Existing `vrc_avatar` preferences are
accepted as an alias for the canonical `avatar` plugin identity.
