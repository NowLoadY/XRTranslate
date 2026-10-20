# VRChat avatar

Control XRTranslate from **Expressions → XRT → Translation** and show
translations on your avatar. Each avatar needs its own installed controls.

## Setup

1. Follow the [avatar installation guide](https://github.com/NowLoadY/XRT-Avatar-Plugin/blob/main/docs/adapters/vrchat/installation.md)
   to add captions or just the translation menu, then upload the avatar for PC.
2. In XRTranslate, set your languages and audio input. Open **Game Plugins**,
   choose **VRChat**, and turn on **Enable**.
3. Wear the uploaded avatar and enable VRChat OSC. When XRTranslate shows
   **Ready**, use **Expressions → XRT → Translation** to start or stop.

After changing avatars or reconnecting, wait for **Ready** and start again.
If a request fails, read the page's diagnostic before retrying. Check game OSC,
the uploaded avatar and whether another translation task is using your input.

For local testing without uploading, use **Tools → XRT-Avatar-Plugin → Local Test**
in Unity with Gesture Manager installed. Click **Start test**, then operate its
**XRT → Translation** menu and inspect the scene captions. Keep the real client's
OSC off during the test; **Stop test** closes the temporary connection.

## Captions

**Show captions** shows or hides the display. Click **Header**, **Messages** or
**Footer** in the preview to edit content. Stopping clears that task's messages;
independent header/footer content remains. Chatbox, overlay and TTS have their
own settings. A menu-only avatar also works.

The display holds 128 glyphs. Horizontal and portrait presets use 32 or 8
columns; custom 16-column objects are supported. Each configured header/footer
uses one row. Only changed chunks are sent, at intervals of at least 200 ms.
Other players see captions through normal avatar visibility and shader settings.

## Integration notes

- Controls and captions share the verified OSC transport and OSCQuery binding.
  Discovery uses an available local port; do not add a competing fixed-port receiver.
- Each new binding resets the game toggle and requires a fresh Off confirmation.
  Old UDP values or snapshots cannot start a session. Failures require a new reset.
- The task owns its inputs until cleanup finishes. Menu Off cancels that task;
  global Stop stops application work. Muting gates audio without releasing capture.
- Avatar changes, disconnects and disable end the task. A world change alone
  keeps a valid binding. Reconnection starts Off in the game and **Ready** in XRTranslate.

The initial target is VRChat and XRTranslate on the same Windows PC. Unity local
communication/rendering was checked with patched preview.7; upload and real-game
end-to-end behavior remain unverified. Linux source checks do not establish
VRChat/Proton compatibility.

[Shared host behavior](avatar-connectors.md) ·
[Control protocol](https://github.com/NowLoadY/XRT-Avatar-Plugin/blob/main/docs/adapters/vrchat/control-protocol.md) ·
[Presentation](https://github.com/NowLoadY/XRT-Avatar-Plugin/blob/main/docs/adapters/vrchat/presentation.md) ·
[Compatibility](https://github.com/NowLoadY/XRT-Avatar-Plugin/blob/main/docs/adapters/vrchat/compatibility.md)
