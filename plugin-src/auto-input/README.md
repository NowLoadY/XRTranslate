# Automatic input

This Windows plugin inserts completed translations into the currently focused
editable control. Place its built directory under the application's `plugins/` folder. Its enable switch is also available in
the floating-subtitle controls while installed. Stopping translation or closing
floating subtitles disables it, as before.

The plugin retains the original focus-change checks, one-attempt delivery,
bounded queue and duplicate suppression. It never saves text for a future focus,
changes focus or writes through the clipboard. The host executable is excluded
from insertion. No Windows input APIs run on the host's UI or output thread.

Build `xrt-auto-input` on Windows and use the [SDK staging helper](../../crates/xrtranslate-plugin)
with its native `.exe`. Missing Windows support is
reported by the ordinary capability check.
