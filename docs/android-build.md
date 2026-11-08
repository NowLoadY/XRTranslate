# Android APK

Build a signed ARM64 APK with the existing entry point:

```sh
XRT_ANDROID_SIGNING_ENV=/path/to/android-release.env ./scripts/build-android-release.sh
```

The signing file supplies `XRT_ANDROID_KEYSTORE`, `XRT_ANDROID_KEY_ALIAS`,
`XRT_ANDROID_STORE_PASSWORD` and `XRT_ANDROID_KEY_PASSWORD`. Keep it outside Git.
The APK is written to `dist/XRTranslate-v<version>-android-arm64.apk`.
`python3 scripts/android.py doctor` checks the build tools; `setup` installs
SDK/NDK components. Required: SDK 36, NDK 29.0.14206865, JDK 17+, Rust Android
ARM64 target, CMake and build-time Python. There is no Python runtime in the APK.

## Plugins

The build discovers Android-enabled packages through
`[package.metadata.xrtranslate] android = true`. It uses the same SDK builder,
process protocol, UI drawing, CLI registration and host model services as desktop.
Agent, Media, Meeting, Vocabulary, VRChat OSC and ChilloutVR OSC are included.

Android 10+ disallows executing files from writable application storage.
Consequently executables are installed by the APK package manager as JNI `.so`
files; manifests/icons are copied into `files/plugins`, and the loader resolves
code within `nativeLibraryDir`. Settings and documents stay in `runtime/plugins`.
The host has no list of plugin functions. Native code updates require an updated
APK; Android does not support the desktop drop-in executable installation method.
Enabling/disabling plugins continues to use the existing plugin UI.

File picking and sharing run in the host Activity. Plugin workers use the shared
file-dialog adapter through the same request/reply channel. Audio decoding and
playback use the shared host implementation; desktop MPV video display/export
and desktop-only capture/VR integrations are not included.

## Verification

HTTP clients share `xrtranslate-http`. Android publishes its current system proxy
and exclusions to an atomic, app-private file on proxy changes; the UI and native
workers inherit its path at startup. This keeps VPN HTTP proxy support consistent
without a Java VM in workers or changing environment variables after startup.
Loopback services stay direct; explicit download proxy settings take precedence.

The build validates 16 KiB ELF alignment. Check the resulting signature with
Android SDK `apksigner verify` and inspect APK native libraries/manifests. Building
and inspecting an APK does not establish device compatibility: microphone
permission/service lifecycle, documents, rendering and playback still need a
physical-device check. An emulator is not required for compilation.

Platform references:
[Android 10 execution restrictions](https://developer.android.com/about/versions/10/behavior-changes-10#execute-permission),
[16 KiB page support](https://developer.android.com/guide/practices/page-sizes).
