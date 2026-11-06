# Big Walk output

Place the built directory under the application's `plugins/` folder, then open **Big Walk**.
Install the existing game bridge and join a game. The page reuses the host's text
composer; committed translations are sent according to the selected audio sources
and text format. See the [setup guide](../../docs/bigwalk-chat.md).

The plugin owns the local connection, bounded delivery queue and duplicate
suppression. Disabling or removing it closes the connection and discards pending
messages. `bigwalk.status` exposes status without requiring the page to be open.
Build and package instructions are in the [SDK guide](../../crates/xrtranslate-plugin).
