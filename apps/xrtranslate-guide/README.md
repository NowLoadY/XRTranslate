# Standalone guide

`xrtranslate-guide.exe` is a self-contained tutorial player. Double-click it to
connect to the running client. If no client is connected, **Open client** selects
and starts XRTranslate. A sibling release's client is found automatically.
The window contains a horizontally scrolling row of page pills, one brief
instruction and Previous/Next. Topics within a page use Previous/Next.
Close the window or press Escape to exit; the client stays open.

The player embeds `xrt-guide`'s planning library, lessons and five-language audio. It does **not**
require the guide plugin or `guide.presentation.v1` in the target client.
It uses the existing loopback Director operations to navigate and open views;
registered targets also receive the client's usual read feedback. Older custom
controls without registered names are explained in their opened view.
An unavailable Director protocol produces a short error in the player.
Each selection refreshes the client's language before playing its matching
clip, while view preparation continues. Previous/Next fades the previous voice
over 180 ms and starts the new voice; closing the window stops all speech.
The 540×300 window uses XRTranslate's native window and executable icons.
Audio uses the system output device and requires no connection to a TTS service.

```powershell
cargo build -p xrtranslate-guide --release --locked
```

The only file required by the user is `target/release/xrtranslate-guide.exe`.
Fonts come from Windows. No Python, model runtime, loose lesson files or plugin
installation is needed. Distribute source under the repository's AGPL license.

For automation, `--port` selects the client's Director port, `--client` supplies
its executable, and optional `--director-port` exposes the player's own controls
and frame capture on a **different** port. No automation settings appear in the UI.

Planning is shared with [the bundled guide](../../plugin-src/guide/README.md).
The existing [video workflow](../../tools/demos/README.md) still reads those same
lessons through the bundled plugin; it retains ownership of timing and encoding.
The standalone player does not add a second recorder or a second lesson catalogue.
