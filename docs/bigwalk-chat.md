# Big Walk chat

1. Install the [XRT Big Walk bridge](https://github.com/NowLoadY/XRT-Avatar-Plugin/blob/main/docs/adapters/bigwalk/installation.md)
   in your BepInEx game profile and launch Big Walk.
2. In XRTranslate, enable **Big Walk** in Settings → Plugins, open its sidebar
   page and turn on **Send to Big Walk**.
3. Join a game, start microphone translation, and speak. **Connected** means
   the bridge has found your local player. The text editor also supports typed
   translation and direct sending.

The bridge invokes the game's native chat command. Other players receive normal
game chat and do not need the bridge. This is a community-mod integration; Big
Walk does not publish an external chat API.

Microphone translations are selected by default. System audio is optional, so
other players' speech is not automatically repeated into chat. The format menu
offers translation only, bilingual lines, or a single bilingual line.

Only completed messages are sent. Messages are at least two seconds apart, with
up to four waiting for at most eight seconds. A full queue skips incoming messages;
disconnecting, changing game sessions, or stopping output discards the queue.
Unconfirmed messages are not retried. **Last sent** shows the text accepted by the
game's chat method, not a receipt from other players. Clearing XRTranslate's
history does not remove messages already sent to the game.

The bridge uses a 256 UTF-16-unit readability budget. This is an adapter limit,
not a documented game limit. Long text is shortened with an ellipsis, sharing
space between the selected languages. It stays in one chat message.

Both applications run on the same computer. The bridge listens on loopback
TCP port `38647`; its [protocol and build guide](https://github.com/NowLoadY/XRT-Avatar-Plugin/tree/main/adapters/bigwalk)
belong to the game adapter.

The native command path follows the approach demonstrated by
[BigTalk ConversationStarters](https://thunderstore.io/c/big-walk/p/gogogadgetjustice/BigTalk_ConversationStarters/).
