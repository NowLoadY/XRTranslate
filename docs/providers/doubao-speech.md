# Doubao account speech connection

Status: implemented and live-checked on 2026-10-29 with a user-owned account.
Desktop login and Chinese/English recognition completed successfully on Linux.
The website protocol remains experimental, and Windows/Android interactive login
still needs device acceptance testing. No credentials are bundled or created
automatically.

## Setup and scope

On **Welcome → Configure models → Speech recognition**, select **Account login
→ Doubao → Sign in**. Complete the normal login on `https://www.doubao.com/chat`.
The login window closes after the official profile response confirms login and
the required session data is available. There is no API key, App ID, model,
resource ID, or Volcengine activation to configure. Subsequent recognition uses
the saved connection directly; the web view does not remain running.

Login completion is detected automatically from the official profile response,
without a manual confirmation button. After saving the connection, the login
view closes and the client updates to **Connected** with a success notice. Desktop
clients restore and request focus for the main window; window managers that
prevent focus stealing receive a taskbar attention request instead. Android
dismisses the in-app login dialog. Closing/cancelling the login view does not
report success, and ordinary background session checks never raise the window.

**Local model / Online API / Account login** are independent connection channels.
Existing selections and API providers retain their behavior. This initial
account connector supports speech recognition only. It does not claim Doubao
consumer chat/TTS access or a particular Seed ASR model/version. The catalogue
profile `doubao-web-speech` is explicitly `server_managed`, and is never sent as
an upstream model ID. Its language declaration is `zh,en`, verified against public speech fixtures;
other languages require evidence before broadening the card.

This uses the consumer website protocol, not an officially supported public
OAuth/API integration. Changes to the website, account eligibility, or service
limits can affect availability. Official
Volcengine API services remain a separate possible API-channel integration.

## Ownership and extensibility

| Owner | Responsibility |
| --- | --- |
| `xrtranslate-assets::remote` | Connection channels, account identity, connector, exact service profile and capabilities |
| `xrtranslate-connections` | Login registry, official origins, session validation, private storage, sign-out and generation-scoped invalidation |
| Desktop login helper / Android host | Temporary native login view, native HttpOnly cookie access, cancellation and window lifecycle |
| Welcome UI | Generic channel/provider selection and sign-in, connected, expired and sign-out states |
| `doubao_web` inference adapter | Account-specific request/authentication, PCM wire protocol, cumulative snapshots and completion |
| Shared pipeline | Capture, VAD, raw PCM delivery, bounded buffering, cancellation, translation/history commit |

Future account services add a registry entry and a connector with an
`account_provider()` identity. UI/pipeline code must not dispatch on provider
names. Multiple capabilities may share that identity and saved connection.
OAuth-based services use their own system-browser authorization mechanism while
sharing account actions and presentation. ChatGPT's official OAuth implementation
is described in [online services](../online-api-providers.md#chatgpt-account-inference).
Doubao continues to use its dedicated website-session login specification.

## Session handling

Sessions use the shared multi-account inventory in
`runtime/accounts/doubao.accounts.json`, separate from `user-config.json` and
model-runtime download directories. Each saved identity keeps its own session;
selection uses that account's credentials. When the official profile supplies a
stable user ID, signing in again updates the matching account. Display names are
never used to merge identities. Writes use a private temporary file and
atomic replacement. Unix directories/files use 0700/0600 permissions; Windows
uses the installation directory's inherited ACLs, as other runtime data does.
Session Debug output is redacted. Cookie headers are marked sensitive and are
never included in URLs, process arguments, UI error messages or logs. Desktop
helpers receive only the provider identity and project root.

The host reads cookies only from its dedicated official-site login view. It
does not inspect the user's browser profiles. Only allowlisted HTTPS domains
may navigate, and capture is limited to the exact registered origin. Remote
pages receive no native IPC bridge. Desktop web views are incognito; Android
clears the app's temporary login WebView data on completion/cancellation.

Explicit service login-rejection codes expire the saved session and display
**Sign in again**. A transient network failure, arbitrary HTTP 403, protocol
failure, or early socket close does not log the user out. Store mutations are
serialized across processes, and a failed older request cannot invalidate a
newer login. Sign-out clears only the chosen account's credentials. Signing out
the active account also stops current backend consumers before the next request;
other saved sessions remain available. It disconnects XRTranslate; it does not
revoke all sessions on the provider's account.

## Audio contract

- Endpoint is fixed to the registered `wss://ws-samantha.doubao.com/samantha/audio/asr`.
  Override hosts/paths are rejected before sending cookies; no credential forwarding.
- The server accepts 16 kHz mono PCM16 binary frames. The connector sends no
  prompt, vocabulary bias, language override or upstream model selector.
- The shared PCM framer coalesces capture packets into 100 ms wire frames and
  retains the shorter final tail. This keeps 32 ms VAD packets from becoming
  individually flushed network messages; recorded input uses the same framing.
- Every `result.Text`, including empty text, replaces the current preview.
  Identical snapshots do not emit redundant previews. Preview events never
  commit subtitles/history/translation.
- After shared input EOF, all queued audio drains first. The adapter sends
  the text frame `{"event":"finish"}` and waits up to 12 seconds for the service
  to return an explicit `finish`. It does not inject silence or guess completion.
  A quiet-period heuristic, timeout or socket close never promotes a preview to
  a final transcript. A finish before input is drained is also rejected.
- An explicit finish with empty text completes a silence/noise segment. The
  shared pipeline skips it and continues the input; it does not abort a media task.
- Dropping/cancelling the request drops both send/receive futures and its socket;
  there is no detached sender or unbounded audio queue. Recorded audio is paced.
- Total request time and response-frame size are bounded. No Qwen-specific
  session reuse, finish-task message, or replacement policy is copied here.

Live acceptance used the public [SenseVoice test fixtures](https://huggingface.co/csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17/tree/main/test_wavs).
With real-time audio pacing, the Chinese sample was 5.59 seconds and the whole
request took 6.08 seconds; English was 7.15 seconds and the request took 7.69
seconds. Both returned the complete expected sentence with punctuation after a
service `finish` event. These are individual acceptance measurements, not a
latency SLA or a general accuracy benchmark. Empty completed results produce no
subtitle or translation.

Older protocol notes omitted the client finish frame, and waiting for silence
alone failed live acceptance. The corrected finish request is also documented
in [Douvo's web provider](https://github.com/rhinoc/douvo/blob/main/docs/asr-providers.md).
The actual service response, rather than a quiet timer, confirms completion.

## Host and release requirements

Desktop releases include `bin/xrtranslate-account-login[.exe]`. The helper owns
its native event loop, avoiding GTK/WebView dependencies in the main client or
backend. Windows uses system WebView2; Linux uses WebKitGTK 4.1. Linux build
packages are listed in [Linux build](../linux-build.md). Android uses its system
WebView and requires document-start script support; older WebViews receive an
update instruction. No separate Python service or user-installed Doubao plugin
is involved.

Release staging copies binaries/defaults from an allowlist and never copies
`runtime/accounts`. Default configuration contains only an empty account preset;
release configuration has no cookies, account sessions, or private browser data.

## Research references (reviewed 2026-10-29)

- [doubao-say](https://github.com/quanru/doubao-say): separates interactive account
  login from secret/API credentials in its provider registry.
- [doubao-murmur Windows login](https://github.com/lilong7676/doubao-murmur/blob/master/windows/src-tauri/src/login.rs):
  native WebView cookies plus official profile and localStorage session evidence.
- [Web speech protocol observations](https://github.com/lilong7676/doubao-murmur/blob/master/docs/doubao-wss-asr-spec.md):
  PCM and cumulative result observations. Its older finish description omitted
  the client finish command; see the live correction above. This is third-party
  observation, not an official protocol guarantee. Implementation here is
  independent; no example credentials or upstream application code are bundled.
- [Wry](https://github.com/tauri-apps/wry) and
  [Android WebKit](https://developer.android.com/jetpack/androidx/releases/webkit)
  provide the native host webview APIs.
