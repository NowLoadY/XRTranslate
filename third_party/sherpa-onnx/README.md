# sherpa-onnx libraries

This directory contains **precompiled libraries** for Windows, Linux and Android
from [sherpa-onnx v1.13.8](https://github.com/k2-fsa/sherpa-onnx/releases/tag/v1.13.8),
not a source checkout. The upstream revision is
`11afbd009a7f8c08f4bcf2fc1b265d0df4670fbf`.

| Platform | Archive | Bytes | SHA-256 |
|---|---|---:|---|
| Android | `sherpa-onnx-v1.13.8-android.tar.bz2` | 46,093,321 | `2ff63469a71cb6009aa2e3ed5f4a670f8abdcbe4bb9ffd23776afc792a6b4f44` |
| Linux x64 | `sherpa-onnx-v1.13.8-linux-x64-static-lib.tar.bz2` | 22,952,328 | `e1fdc5b67530e15741ef897fa5ffff297056f3bf0c6d829a27af9225a4c4b5a6` |
| Linux ARM64 | `sherpa-onnx-v1.13.8-linux-aarch64-static-lib.tar.bz2` | 21,325,380 | `77983e3cf29aa60f2e531d249dbd01d15596530550c8db2e9e02fc6a655da6bb` |
| Windows x64 | `sherpa-onnx-v1.13.8-win-x64-static-MD-Release-lib.tar.xz` | 48,336,832 | `a89dff64a408eb9c07eda86487f26ff2c043e35607aaa58e58e358a440517eda` |

The official archives are available on the upstream release linked above. Linux
and Android archives are unchanged. The Windows archive is recompressed with XZ
without changing its complete decompressed contents.
Its original `.tar.bz2` is 96,599,351 bytes, with SHA-256
`a0f44cd91486e448c2be1f1d3662edb4f473ca3cb38a803cee158760cc588428`.
The Windows libraries use the dynamic MSVC runtime (`/MD`), matching Rust's default
Windows MSVC target; Sherpa itself remains statically linked.

The Android archive contains four ABIs. XRTranslate packages only the C API and
ONNX Runtime libraries for its supported `arm64-v8a` and `x86_64` targets. The
existing Android packaging step adjusts runtime search paths on extracted copies;
the archive remains unchanged. Rust bindings remain pinned by `Cargo.lock`.

## Licenses

`LICENSE` is sherpa-onnx's Apache-2.0 license at the revision above. The binaries
also include third-party components with their own terms:

- `licenses/sherpa-ThirdPartyNotices.txt` retains component licenses and source URLs.
- `licenses/onnxruntime-LICENSE` and `licenses/onnxruntime-ThirdPartyNotices.txt`
  come from [ONNX Runtime v1.28.2](https://github.com/microsoft/onnxruntime/tree/v1.28.2),
  the version included in this archive.
