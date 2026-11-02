# Online audio

Users choose the plugin before entering a link. No shared link inbox or automatic
plugin switch is involved. The plugins share discovery and decoding code, not UI
ownership or session state.

| Location | Accepted online content | Processing |
| --- | --- | --- |
| Meeting → New meeting → Audio source → Online stream / RSS | Live radio, live HLS, RSS items explicitly marked live | Realtime recognition and meeting translation; pause skips incoming audio; finish drains accepted work |
| Media → Import media → existing file/link field | Recorded podcast episodes, RSS episodes, finite online audio/video | Existing file-style recognition, subtitle translation, playback and seeking |

Load the link/RSS and select an item before starting. Mixed feeds display item
status; the selected plugin rejects incompatible items in place. Upcoming and
ended live items do not become recordings automatically. Existing local file and
meeting-recording workflows remain available.

## Source discovery

- RSS `<item>/<enclosure>` and Atom enclosure links describe recorded episodes.
- Podcasting 2.0 `<podcast:liveItem>` uses its `status`, rather than guessing from
  scheduled times. Standard and alternate enclosures are supported; an HTML
  `contentLink` is not mistaken for a playable stream.
- Public Apple Podcasts show/episode links use the public catalogue. A specific
  episode must match its exact ID; if absent from the latest 200 catalogue entries,
  lookup reports it unavailable instead of choosing another episode.
- Public Xiaoyuzhou episode/show pages use their episode data. Paid/private
  entries are excluded. Show pages expose the entries actually present in the page.
- Direct HTTP(S) media uses ICY radio metadata or finite response length;
  HLS uses its media playlist and `ENDLIST`. Ambiguous direct sources are reported
  as unsupported until the publisher supplies enough information, such as an RSS
  episode or live item. URL extensions alone do not establish live status.

RSS is a catalogue format, not a real-time transport. Its normal enclosures and
optional live items describe different content. References:
[Apple RSS requirements](https://podcasters.apple.com/support/823-podcast-requirements),
[Podcasting 2.0 live items](https://podcasting2.org/docs/podcast-namespace/tags/live-item).

## Execution and limits

HTTP audio is decoded incrementally with bounded PCM queues. Byte-range reads
allow seeking in formats such as M4A without downloading the entire episode.
Servers must support ranges when their container requires seeking. Network reads
are cancellable and time out; truncated responses report failure.

HLS and formats requiring MPV reuse the existing optional `mpv` runtime. PCM is
streamed through a loopback socket, without a growing extracted file or an
additional FFmpeg executable. Builds without MPV retain native HTTP audio
decoding; network playback and HLS require MPV. Network URLs are retained as
sources, while source audio is not downloaded permanently. Signed or expired
URLs, unavailable CDNs and access-controlled platform pages can fail normally.

Meeting network input does not record system output or require a microphone.
Pausing keeps the connection current while discarding incoming recognition audio;
resuming continues from the current broadcast. Natural EOF and explicit finish
wait for accepted recognition and translations. Source audio retention is currently
available for the existing device-capture workflow, not network live input.

## Verification

Run the deterministic import, source-selection and meeting lifecycle tests:

```sh
cargo test -p rust-client --lib --features mpv
```

The ignored online checks accept an explicitly selected public source. They do
not make network requests during ordinary tests:

```sh
XRT_AUDIO_CHECK_URL='https://streams.kqed.org/kqedradio' \
cargo test -p rust-client --lib --features mpv public_online_audio -- --ignored --nocapture
```

For an end-to-end check, start a separately configured backend and corpus service,
then set `XRT_AUDIO_CHECK_SERVER` to its WebSocket URL and run the ignored
`online_meeting_recognizes_translates_and_persists` test with an English speech
stream. It checks real ASR, translation, saved meeting segments and final status.
The test uses temporary meeting storage and does not modify the user's meetings.

Validated on Linux with MPV: KQED live radio, a rolling local HLS broadcast,
Xiaoyuzhou M4A, an Apple Podcasts Changelog episode and RSS selection. HTTP tests
cover incremental delivery, range seeking, truncated transfers and cancellation
while the server is stalled. MPV checks cover channel selection before downmixing
and completion after its output socket stops producing data. A TED Tech episode
resolved correctly but its Acast CDN failed TLS negotiation in this environment;
this is reported as a network error.

On 2026-11-02, the full client regression returned 150 passed, zero failed and
11 environment-dependent checks ignored by default. The avatar bubble interaction
test now explicitly clears texture updates because it runs without a renderer.
Audio configuration is exercised through actual decoding; redundant constant and
progress assertions and the temporary WAV export branch were removed.
