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
Header and footer can show custom text, system information, lyrics or an animated
text pet. Pets automatically use an empty message area for little scenes and
yield to messages and selected lyrics. Their space is included in the preview budget.

The header, message area, microphone tail and footer definitions sit to the right of their
preview regions. Their rows follow the rendered text height as it wraps or scrolls.
Open a dropdown to choose its content and edit custom text. Under **Microphone
tail**, choose **Custom Text** and enter a suffix such as `喵`. It is appended
verbatim to each microphone translation, including additional target languages;
original text, system audio and typed messages are unchanged. Select **None** to
disable it while retaining the saved text. Empty text adds nothing.

The suffix is part of its translation before layout and budgeting, so it shares
the message's scrolling, priority and expiry. It is not a separate footer.
In microphone review mode, the prepared preview includes the tail and refreshes
when its settings change. Explicitly reviewed text retains its existing length
check and is sent exactly as prepared.

When retained text exceeds its allocated space, it scrolls inside that space.
Each displayed language has its own window; source and speaker labels stay fixed.
Shorter languages return unused space to the other languages. Scrolling does not
bring back older messages discarded by the layout or extend a message's lifetime.
Timed messages hold the beginning, move toward the end within their remaining
lifetime, and hold the end. A content or budget change restarts that window.
Overflowing persistent banners loop. Animation updates are silent and use the
same send cooldown as other revisions, including when no new messages arrive.

VRChat allows [144 characters and up to nine displayed lines](https://docs.vrchat.com/docs/osc-as-input-controller#chatbox), including wrapping. ChilloutVR's official OSC documentation does not specify a text limit; its configurable application budget starts at 144 characters. Updates are coalesced at 0.6-second intervals.

ChilloutVR updates the bubble without sound or chat-history entries, so changing
lyrics and partial translations do not fill the conversation history. Players
can control visibility of OSC messages in the game's ChatBox settings.

With Lyrics in both header and footer, the header shows title/artist and the
footer shows the available lyric. A single lyric region shows lyrics with a
title fallback. Each output keeps its own timing offset: rabbit is earlier,
turtle is later, and reset restores the default delay.

Select **Lyrics** in **Message area** to show multiple lyric lines while no
messages remain. The current line is marked with `›`; surrounding lines fit the
available space automatically. If header, message area and footer all select
Lyrics, they combine into one display with song information and multiple lyric
lines. New messages take priority, and the lyric display returns when they expire.

Lyrics and chatbox text share the SDK's grapheme-safe text viewport. Lyrics still
follow song progress; media titles loop independently. Single-line banner lyrics
arrive windowed to their requested budget and are not scrolled a second time.
