# OSC Chatbox

Enable **VRChat OSC** or **ChilloutVR OSC** in XRTranslate's plugin settings,
then open its sidebar page and turn on output. Speech, typed translations,
header and footer use the same layout controls in both games.

| Game | In-game setup | Destination |
|---|---|---|
| [VRChat](https://docs.vrchat.com/docs/osc-overview) | Enable OSC in the Action Menu | Native Chatbox |
| [ChilloutVR](https://docs.chilloutvr.net/chilloutvr/game/osc/) | Settings → Implementation → OSC; enable OSC messages in Audio & Comms → ChatBox | Native chat bubble |

Both games receive on `127.0.0.1:9000` by default. Change the plugin's destination
if the game uses another address or port. Enabling an output at an occupied
destination turns off the other output while keeping its layout. Separate
destinations can run together.

## Content

Choose microphone or system audio, translation format, message separator and
display duration. Recent messages fill the available text budget automatically.
Header and footer can show custom text, system information or lyrics. Their
space is included in the same budget shown by the preview.

VRChat allows [144 characters and up to nine displayed lines](https://docs.vrchat.com/docs/osc-as-input-controller#chatbox), including wrapping. ChilloutVR's official OSC documentation does not specify a text limit; its configurable application budget starts at 144 characters. Updates are coalesced at 0.6-second intervals.

ChilloutVR updates the bubble without sound or chat-history entries, so changing
lyrics and partial translations do not fill the conversation history. Players
can control visibility of OSC messages in the game's ChatBox settings.

With Lyrics in both header and footer, the header shows title/artist and the
footer shows the available lyric. A single lyric region shows lyrics with a
title fallback. Each output keeps its own timing offset: rabbit is earlier,
turtle is later, and reset restores the default delay.
