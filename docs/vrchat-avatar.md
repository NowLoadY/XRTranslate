# VRChat Avatar controls

XRTranslate includes an opt-in built-in plugin that lets a prepared VRChat Avatar
request translation through its Expressions Menu. The [shared connector guide](avatar-connectors.md)
defines connector selection and common session behavior. The companion
[XRT-Avatar-Plugin](https://github.com/NowLoadY/XRT-Avatar-Plugin) repository owns
its VRChat adapter's editor package, parameter manifest and shared connector core.
XRTranslate owns the desktop adapter and runtime resources. Host source is implemented; Unity import,
Avatar upload and end-to-end behavior with a real VRChat client remain unverified.

## Setup

1. Use a build with **Settings → Plugins → Avatar translation controls**. Finish
   XRTranslate setup and select the languages, microphone/system audio source
   and recognition settings. Configure the existing outputs you want to use.
2. In an existing Avatar authoring project, follow the companion
   [installation guide](https://github.com/NowLoadY/XRT-Avatar-Plugin/blob/main/docs/adapters/vrchat/installation.md)
   to add the `XRT → Translation` menu and upload with the official SDK. No Unity
   installation is part of the desktop integration.
3. Select that Avatar in VRChat, enable VRChat OSC, then enable **Avatar translation
   controls** in XRTranslate. The default is disabled.
4. Wait for synchronization to finish and the plugin settings to report Off.
   Use `Expressions → XRT → Translation` to start or stop its session.

The menu tick represents a request. Starting means startup is pending; Active
requires capture and backend readiness. Off requests cancellation, with Stopping
retained until cleanup finishes. Error and Unavailable include diagnostics in
plugin settings. A failed or interrupted operation resets the request and cannot
restart merely because an old On value is still present.

Avatar control uses the current output path. Each Avatar using the controls
needs its own installation. To display translated text on a prepared caption
object, enable **Send captions to the game component**. The host discovers its
layout and sync budget automatically; a menu-only Avatar continues to work.
Chatbox, overlay and TTS retain their own output settings.

Caption objects receive completed translations from the Avatar's session. New
sentences replace pending text, clear the previous display, and fill a 128-glyph
cache in held chunks. Text clears 15 seconds after its last chunk is sent.
Horizontal and portrait presets use 32 or 8 columns; custom 16-column objects
are also supported.
The selected Avatar package supplies the font, mesh, shader and receiver.
Other players see the Avatar's synchronized bus through VRChat's normal Avatar
rendering and visibility settings.

## Sessions and settings

Each accepted request creates an owned task through the existing translation
pipeline. The selected languages, source and recognition profiles are captured
at request time, including while the backend starts. Change these before the
next Avatar session. Device and prompt configuration are resolved when the
session is created. Shared output settings retain their existing runtime behavior;
the companion [host integration contract](https://github.com/NowLoadY/XRT-Avatar-Plugin/blob/main/docs/adapters/vrchat/host-integration.md)
documents setting lifetimes.

The Avatar task holds an exclusive lease for its live inputs. A request overlapping
an active, paused or pending live task is rejected and leaves that task intact.
Other live tasks also cannot take over an Avatar-owned input. Selecting both
inputs overlaps either one; separate inputs and file/text tasks remain independent.

Menu Off cancels only the Avatar owner, including pending startup, capture and
pending result/TTS delivery. Its lease remains until the network workers exit.
Desktop input-off ends an Avatar session using that input; the desktop global
Stop still stops application work. Mute-follow gates audio admission without
releasing capture, so muting is not equivalent to stopping the session.

## Connection behavior

Avatar control and existing OSC receive consumers share one transport owner.
Control advertises an available local receive port through OSCQuery; it does not
open a second fixed-port 9001 listener. Caption capability discovery and writes
use that same verified binding. The installed receiver reserves 24, 40 or 72
synced bits, independently of display length.
Chunks are held for at least 200 ms after local transmission; game replication
remains best effort. A newer sentence cancels queued older chunks. Stop,
Avatar change and disconnect retire the writer and clear where the current
binding remains reachable. Chatbox output retains its configured
destination and can be disabled independently of Avatar control. Discovery,
queries and socket work run off the UI thread.

The adapter validates the local endpoint, Avatar identity, parameter types and
protocol version through fresh OSCQuery snapshots. Every binding first resets
the toggle and waits for a matching authoritative Off confirmation. Raw UDP
values and old snapshot responses cannot open that gate. Subsequent failures,
completion and rejected requests during cleanup require a new reset confirmation.
Periodic reconciliation recovers lost updates without treating an unchanged
toggle as a heartbeat.

Avatar change, connection loss, plugin disable or shutdown cancels the owned
operation. Reconnection requires a new binding and starts Off. A world change
alone does not stop a valid unchanged binding. The initial target is XRTranslate
and VRChat on the same Windows PC; local Linux source checks do not establish
VRChat/Proton compatibility.

If a menu request returns to Off, read the plugin diagnostic and normal host
error notification. Check setup, input ownership and current connection before
retrying. For an unavailable connection, check VRChat OSC, the installed Avatar,
the parameter version and local discovery. Do not replace another OSC tool's
configuration or launch a competing fixed-port receiver to resolve discovery.

## Source map

| Module | Responsibility |
|---|---|
| `plugins::avatar` | Settings, verified control intent, reset/binding state, owner-scoped captions and host actions |
| `plugins::avatar::osc_caption` | Shared caption capability, BMP glyph layout and paced OSC bus encoding |
| `avatar_host` | Owner-scoped acquisition, readiness, cancellation and terminal outcomes |
| `plugins::osc::transport` | Shared local discovery, queries, receiver and parameter writes |
| `session_coordinator` | Neutral task, owner, exclusive-input and result contracts |
| Companion control core | I/O-free lifecycle and stale-operation protection |

The [plugin architecture](plugin-architecture.md) owns host dependency rules.
The companion repository owns the
[wire protocol](https://github.com/NowLoadY/XRT-Avatar-Plugin/blob/main/docs/adapters/vrchat/control-protocol.md),
[compatibility criteria](https://github.com/NowLoadY/XRT-Avatar-Plugin/blob/main/docs/adapters/vrchat/compatibility.md)
and [presentation boundary](https://github.com/NowLoadY/XRT-Avatar-Plugin/blob/main/docs/adapters/vrchat/presentation.md).
