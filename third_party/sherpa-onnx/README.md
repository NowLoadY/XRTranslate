# sherpa-onnx source and libraries

This directory stores [sherpa-onnx v2.0.1](https://github.com/k2-fsa/sherpa-onnx/releases/tag/v2.0.1),
revision `af2d8d81dbfa61c4cea4c568da78f3982edca005`. The Rust dependency is pinned
in the workspace `Cargo.toml` and `Cargo.lock` to the same release.

| Contents | Archive | Bytes |
|---|---|---:|
| Full upstream source, including C/C++ and Rust bindings | `sherpa-onnx-v2.0.1-source.tar.gz` | 11,932,226 |
| Android | `sherpa-onnx-v2.0.1-android.tar.bz2` | 45,300,379 |
| Linux x64 | `sherpa-onnx-v2.0.1-linux-x64-static-lib.tar.bz2` | 22,694,241 |
| Linux ARM64 | `sherpa-onnx-v2.0.1-linux-aarch64-static-lib.tar.bz2` | 21,076,492 |
| Windows x64 | `sherpa-onnx-v2.0.1-win-x64-static-MD-Release-lib.tar.xz` | 52,738,232 |

The source archive is the unchanged [snapshot of the revision above](https://codeload.github.com/k2-fsa/sherpa-onnx/tar.gz/af2d8d81dbfa61c4cea4c568da78f3982edca005).
Native archives are the official release assets. Windows is recompressed from
its 96,089,581-byte `.tar.bz2` to XZ; the decompressed tar contents are identical.
Windows uses the dynamic MSVC runtime (`/MD`), matching Rust's default Windows
MSVC target; Sherpa itself is statically linked.
Linux also links Sherpa statically. Android uses the shared C API and ONNX Runtime
libraries for `arm64-v8a` and `x86_64` from its one upstream multi-ABI archive.

## Building

Source launchers and release scripts prepare these libraries automatically.
For direct Cargo builds, initialize the platform environment first:

```powershell
. ./scripts/windows-build-env.ps1
Initialize-WindowsSpeechLibraries -ProjectRoot $PWD.Path
cargo build --locked -p xrtranslate-backend --features managed-ort
```

```sh
source scripts/linux-build-env.sh
prepare_linux_sherpa "$PWD"
cargo build --locked -p xrtranslate-backend --features managed-ort
```

## Licenses

`LICENSE` is Sherpa's Apache-2.0 license at the pinned revision.
`licenses/sherpa-ThirdPartyNotices.txt` records component licenses and source URLs,
including utf8proc added in 2.x. The former eSpeak NG/piper components were removed
upstream. `licenses/onnxruntime-LICENSE` and `licenses/onnxruntime-ThirdPartyNotices.txt`
come from [ONNX Runtime v1.28.2](https://github.com/microsoft/onnxruntime/tree/v1.28.2),
the version included in Sherpa's libraries. Desktop releases and Android assets
include these notices under `licenses/sherpa-onnx`.
