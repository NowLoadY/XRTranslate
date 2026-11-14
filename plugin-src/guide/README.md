# User Guide

`xrt-guide` is a native planning plugin, bundled with XRTranslate. Its library is
also used by the [standalone player](../../apps/xrtranslate-guide/README.md). Clicking
**User Guide** starts it through the normal plugin connection. Each topic waits
for Previous, Next or a direct selection; Exit and Escape stop the guide.

`assets/lessons.json` owns the localized instructions, topic IDs, required
capabilities, highlight targets and view paths. The plugin filters this catalogue
using the host's available pages and capabilities, and owns the selected topic.
The host keeps its floating controller, Avatar inbox and UI Director view
preparation/highlights. No model, HTTP server or second UI is needed by the plugin.

`assets/audio/` contains the five-language narration and its text/duration manifest.
Both executables embed these same clips. `Narrator` starts the selected clip
immediately while the previous voice fades over 180 ms. Exit stops both voices;
catalogue updates do not restart speech, and audio never advances the lesson.
It uses the client's existing audio player, extracted into `xrtranslate-audio`
so the standalone player does not need the full client UI or model runtime.

Build just this package from the host checkout:

```powershell
./plugin-src/build.ps1 -Sdk . -Output plugins -Plugin guide -TargetDirectory target -Locked
```

The package is `plugins/guide/`, including `xrt-guide.exe` on Windows. Edit the
lesson source and rebuild this package to update instructions independently of
the client. Windows, Linux and Android packaging use the existing plugin pipeline.

## Narration assets

Finished narration is stored in `assets/audio/`. Its manifest contains only
localized text, clip paths and durations. Update clips and matching metadata when
copy changes; the build rejects missing or stale narration. Both playback and
video recording work offline from these assets.

Voice enrollment, reference preparation and synthesis are local authoring work,
kept outside this repository. Client and plugin builds do not require those tools,
reference samples, authoring settings or a speech-service account.

## Host interface

The shared types are in `xrtranslate_plugin::guide`. The host supplies
`guide.context` when navigation, language or available features change. The plugin
publishes `state`; the host acknowledges a prepared revision with
`guide.presented`. Stale acknowledgements cannot complete a later topic.
Stopping or unloading the plugin clears the presentation and pending view work.

The in-client presentation requires `guide.presentation.v1`. Earlier clients can
instead use `xrtranslate-guide.exe`, which supplies its own window and controls
over their existing Director interface. Future lesson and sequencing changes
only require replacing the guide package or rebuilding the standalone player.

## Automation and recording

The floating controller and external Director clients use the same operations:

| Operation | Purpose |
|---|---|
| `guide.catalog` | Available localized pages and stable topic IDs |
| `guide.start` | Start on the current or specified `page` |
| `guide.select` | Select `page` and optional `topic` ID |
| `guide.previous` / `guide.next` | Move one topic; finish at the end |
| `guide.status` | Current selection, revision and view readiness |
| `guide.stop` | Exit |

Recording remains in [tools/demos](../../tools/demos/README.md). It reads the
catalogue, selects each topic, waits for that revision's `ready`, and captures
the existing host rendering. Captions and chapter text come from the same lesson.
Video timing, framing and encoding remain in the recorder. There is no automatic
advance timer in the interactive guide.
Each catalogue step includes its audio asset and duration. Recording places the
same clips on the video timeline and leaves a short pause after each instruction.
