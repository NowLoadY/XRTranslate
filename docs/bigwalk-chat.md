# Big Walk chat

1. Install the [XRT Big Walk bridge](https://github.com/NowLoadY/XRT-Avatar-Plugin/blob/main/docs/adapters/bigwalk/installation.md)
   and launch the modded game.
2. In XRTranslate, open **Game Plugins**, choose **Big Walk**, and turn on **Enable**.
3. Join a room. When the page shows **Connected**, start microphone translation
   and speak. You can also translate typed text or send it directly.

Microphone translations are sent by default. **Audio sources** chooses which
audio translations reach chat; system audio is optional. **Format** chooses
translation only or bilingual text. Other players receive normal game chat
and do not need the bridge.

Only completed translations are sent, at least two seconds apart. **Last sent**
shows the text accepted by the game's chat method. Clearing XRTranslate's
history does not remove chat messages.

## Delivery

Up to four messages wait for at most eight seconds. A full queue skips new
messages; disconnecting, leaving or stopping output discards pending text.
Unconfirmed messages are not retried. Long text is shortened to one message
within the adapter's 256 UTF-16-unit budget, sharing space between languages.
This is an adapter limit, not a documented game limit.

Both applications run on the same computer, using loopback TCP port `38647`.
This community mod uses the native chat command; Big Walk has no published
external chat API. Multiplayer acceptance is still pending.

[Adapter protocol and builds](https://github.com/NowLoadY/XRT-Avatar-Plugin/tree/main/adapters/bigwalk) ·
[BigTalk reference](https://thunderstore.io/c/big-walk/p/gogogadgetjustice/BigTalk_ConversationStarters/)
