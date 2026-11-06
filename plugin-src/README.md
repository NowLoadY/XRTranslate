# Native plugin source

These independent executables use the [public SDK](../crates/xrtranslate-plugin).
They depend on the running host's UI/translation services, not its source or other
plugins' private implementation. Put built plugin directories under the app's
`plugins/` folder; removing them uninstalls them, including during a running session.

| Source | Platforms | Responsibility |
|---|---|---|
| `agent` | Linux, Windows | Ongoing conversation, tool execution, context and progress |
| `media` | Linux, Windows | Media projects, subtitle editing/optimization, dubbing and export |
| `meeting` | Linux, Windows | Meeting records, recording navigation and transcript processing |
| `vocabulary` | Linux, Windows | Wordbook pages and example work using the shared bookmark store |
| `lyrics` | Linux, Windows | Music observation, lyric cache, title and synchronized text |
| `bigwalk` | Linux, Windows | Translation/direct text delivery through the game bridge |
| `auto-input` | Windows | User-enabled insertion into an already focused text field |

Release scripts stage these native directories and the release packager copies
them under `plugins/`. Private plugins are not included. Third parties use exactly
the same layout; a Steam DLC depot can deliver that directory without a second
plugin loader. Store ownership checks, if needed, belong to the store integration.

`scripts/run-linux.sh` also builds and updates the public development plugins.
For an individual plugin, use
`xrt-plugin stage <source> <binary> <destination> --replace`;
its persistent data stays in the runtime directory.

Lyrics receives `outputs.lyrics` and publishes `lyrics.text`. OSC's integration
owns that connection; the loader has no lyrics-specific code. Big Walk and
automatic input consume committed, clear and cancellation translation events,
retaining their bounded queues and result identities. Installing automatic input
does not start typing; its input switch must be enabled explicitly.

Motion source is in an independent private Git repository under the ignored
`private-plugins/` directory. The public build does not depend on it. Built plugin
directories in `/plugins/` are also ignored; they are not source repositories.

The existing egui pages use `xrtranslate-client-core` and the SDK's optional UI
surface. Their buttons and fields keep their original handlers; UI Director
exposes those same controls to the CLI. Inference, capture, shared model settings
and downloads remain host services. A plugin is not a second standalone app.
