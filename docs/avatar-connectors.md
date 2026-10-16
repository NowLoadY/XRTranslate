# Game Plugins

Open **Game Plugins** in the sidebar, choose your **Game**, and turn on
**Enable** at the top of the page. Install its
[XRT-Avatar-Plugin component](https://github.com/NowLoadY/XRT-Avatar-Plugin)
using the game guide. The page is always available; **Enable** is off by default.
Switching games changes the active connection and clears its pending output.

| Connector | Game interaction | Host transport |
|---|---|---|
| VRChat | Expressions menu | OSCQuery profile |
| ChilloutVR | CCK Advanced Avatar Settings | OSCQuery profile |
| Overte | Avatar script and Tablet button | Paired local WebSocket |
| Resonite | Native controls through ResoniteLink | Paired local WebSocket bridge |
| Second Life | LSL attachment | HTTPS relay and local WebSocket bridge |
| Big Walk | Native chat | Local TCP bridge |

These are development-preview integrations. Platform setup and game-client
verification are maintained in the companion guides. ResoniteLink requires an
eligible session hosted by the user. Second Life requires an HTTPS relay.

## Setup and use

Finish XRTranslate setup and select languages, input and recognition settings.
Configure desired host outputs. For Big Walk, follow the [chat guide](bigwalk-chat.md).
Its audio source switches filter completed translations for chat.

For avatar controls in VRChat and ChilloutVR, enable OSC in the game;
for WebSocket connectors, copy the address and pairing token into the local
component or bridge using its guide. Pairing credentials are runtime values and
are not saved into shared game assets.

Wait for synchronization and Off, then operate the game control. Starting means
startup is pending; Active requires capture and backend readiness. Off cancels
pending startup and running work, with Stopping retained until cleanup finishes.
Read connection diagnostics on the page and session failures in host
notifications. Reconnection and connector changes start with the control Off.

For caption objects, enable **Display content** in **Game Plugins**. Select Header,
Messages or Footer in the preview to edit that region. Messages have their own
source filter, language format and duration. Header and Footer
can show custom text, system information or lyrics. Lyrics run when either Avatar
or OSC requests them.

Game controls start and stop their translation task. Stopping removes that task's
messages while configured header/footer content remains. Disabling display or
losing the connection clears the game objects. Existing OSC Chatbox, overlay
and TTS outputs keep their own settings.

## Ownership and configuration

`plugins::game_plugins` supplies the shared game selector for this group page.
The host activates its selected output while **Enable** is on and retires the
previous one. `plugins::bigwalk` consumes committed results and sends bounded,
one-shot chat messages; `plugins::avatar` manages structured
caption displays and game-controlled translation sessions. Each retains its own
settings and delivery lifecycle.

`plugins::avatar` owns connector selection, validated intent and feedback.
`avatar_host` turns shared controller effects into owned translation tasks.
`session_coordinator` owns input acquisition, task identity and cancellation.
The companion `xrt-connector-core` supplies the connector catalogue, parameter
contracts, text codecs and one pure lifecycle. The host pins that single SDK.

Each accepted request holds an exclusive lease on its selected live inputs.
Overlapping active, paused or pending tasks are preserved and the request is
rejected. Cancellation affects the requesting owner and retains its lease until
workers finish. Switching connectors, losing the connection, turning off
**Enable** or a required desktop input ends that operation.

Languages, source and recognition profiles are captured when requested; device
and prompt settings are resolved when the session is created. TTS and existing
output settings follow their owners' runtime rules. Display subscriptions use
shared host output; cancellation identifies the affected stream. Old connection
traffic and late worker callbacks
cannot activate a later binding.

OSC and WebSocket are transport capabilities. Their workers own sockets,
verification, bounded queues and cancellation. OSC peer discovery rules and
parameter maps come from the companion SDK; `plugins::avatar::osc_caption`
only schedules its encoded frames using the existing transport lease.
Game-specific rendering and wire details are maintained in the companion repo.
Chatbox retains its own destination.

`AvatarPlugin` consumes normalized `HostOutputEvent` values and composes one
`DisplaySnapshot` containing `header`, `messages` and `footer`. Only messages
carry remaining lifetimes. Source filtering, formatting and expiry stay in the
host. It supplies recent, unexpired candidates rather than a user-selected message
count. Capacity-limited connector layouts select the newest messages that fit
their objects, retaining chronological order and clipping an oversized latest
message when necessary. Native auto-sizing remains a presentation setting.
The protocol's eight-entry bound limits the candidate snapshot, not the number
of messages a surface must show.

The internal `TextOutputPlugin` supplies shared formatting, banner controls and
one owned system sampler through a handle injected by the host. OSC and Avatar
retain separate preferences. Lyrics has one worker, requested by either output;
the host distributes its text without introducing a new event bus.

Native text accepts replaceable previews; buffered displays use completed
captions. Stream cancellation removes only that stream's messages. Connection
changes clear all pending presentation. WebSocket coalesces full snapshots and
deducts queue time from message lifetimes. The companion SDK lays out OSC regions,
prioritizes body updates and selects changed chunks; the host confirms its cache
only after actual send. The Game Plugins page uses the same OSC layout for its preview.
Header/footer updates never restart the whole board. Superseded snapshots do not
form a playback queue, and hidden messages do not gain extra lifetime.

The [plugin architecture](plugin-architecture.md) owns host dependency rules;
the companion [connector protocol](https://github.com/NowLoadY/XRT-Avatar-Plugin/blob/main/docs/connector-protocol.md)
owns wire fields and client requirements. Existing `vrc_avatar` preferences are
accepted as an alias for the canonical `avatar` plugin identity.
