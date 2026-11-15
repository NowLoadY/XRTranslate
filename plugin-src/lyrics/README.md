# Lyrics

Place the built directory under the application's `plugins/` folder, then choose lyrics or
media title in an output's display settings. The plugin uses Linux MPRIS or Windows
media sessions and a bounded local lyric cache. Its status and **Clear lyrics
cache** button appear in Settings; `lyrics.status` is available to the CLI.

On Windows, NetEase Cloud Music's local playback events supply progress when
SMTC omits its timeline, including pause, resume and seek updates. Missing or
unmatched events fall back to the media title. Linux continues to use MPRIS.

Media titles loop within the output's character budget; timed lyric windows follow
playback progress. Both use `xrtranslate_plugin::text::TextViewport`, also used by
OSC message composition, so character budgets and grapheme boundaries are shared.
Outputs can also request surrounding timed lines for a multi-line display; the
output determines how many lines fit and keeps regular messages ahead of lyrics.

Build and package instructions are in the [SDK guide](../../crates/xrtranslate-plugin).
See `licenses/desktop-lyric/` for retained upstream attribution and GPL terms.
The Windows NetEase fallback's MIT attribution is in `licenses/netease-cloudmusic-detector/`.
