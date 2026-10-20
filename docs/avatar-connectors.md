# Game Plugins

1. Set your languages and audio input in XRTranslate.
2. Open **Game Plugins**, choose your game, and turn on **Enable**.
3. Open **Setup guide** to install the game component, then connect:

| Game | Connection | Start translation |
|---|---|---|
| VRChat | Enable game OSC | **Expressions → XRT → Translation** |
| ChilloutVR | Enable game OSC | **Advanced Avatar Settings → XRT Translation** |
| Overte | Paste **Copy connection info** into the tablet | Tablet play button |
| Resonite | Paste **Copy connection info** into the connector | Connector or native avatar control |
| Second Life | Paste **Copy connection info** into the relay connector | Touch the attachment → **Start** |
| Big Walk | Launch the modded game and join a room | XRTranslate microphone translation |

Avatar connections show **Ready** before starting. After changing avatars or
reconnecting, start again from the game. Big Walk shows **Connected** when chat
is available; see the [chat guide](bigwalk-chat.md).
OSCQuery connection loss appears on the plugin page while discovery retries.

Game-client verification is recorded in each game's guide. ResoniteLink needs
an eligible session you host; Second Life needs an HTTPS relay.

## Captions

Translations appear by default. **Show captions** shows or hides the whole
display. Click **Header**, **Messages** or **Footer** in the preview to edit it.

- **Messages:** audio sources, language format and display seconds.
- **Header / Footer:** optional text, time, device status or lyrics.
- **Trash icon:** clear messages now.

Stopping translation clears its messages. Header/footer content remains until
its source is cleared, captions are hidden or the connection closes. Chatbox,
overlay and TTS keep their own settings.

## Host integration

`plugins::game_plugins` activates one selected game while **Enable** is on.
Switching games retires the old connection and clears pending output.
`plugins::avatar` owns game controls and caption composition;
`plugins::bigwalk` sends completed translations to native chat.

`avatar_host` turns validated game requests into owned translation tasks.
`session_coordinator` owns input leases and cancellation. Requests cannot take
over an input used by an active, paused or pending task. Cancellation keeps the
lease until workers finish. Disconnect, disable or a required input turning off
ends the owned task; stale traffic cannot restart it.

Languages, source and recognition profiles are captured at request time. Device
and prompt settings are resolved at session creation. Shared outputs follow
their existing runtime settings.

The pinned companion `xrt-connector-core` owns connector contracts and the pure
control lifecycle. Existing OSC and WebSocket workers own discovery, sockets
and delivery. OSC caption frames use the verified transport lease; Chatbox
retains its own destination.

`DisplaySnapshot` contains header, messages and footer. Messages expire
independently; capacity-limited displays keep the newest content that fits.
The eight-message wire bound limits candidates, not the visible message count.
Native text accepts previews; buffered OSC displays use completed messages.
Queue time reduces remaining lifetime, and a new snapshot replaces obsolete
work. Header/footer updates do not restart the board.

Shared formatting and system sampling belong to `TextOutputPlugin`. Lyrics
uses one host worker, requested by either OSC or Avatar output. Existing
`vrc_avatar` preferences remain an alias of `avatar`.

[Host architecture](plugin-architecture.md) ·
[VRChat integration](vrchat-avatar.md) ·
[Companion protocol](https://github.com/NowLoadY/XRT-Avatar-Plugin/blob/main/docs/connector-protocol.md)
