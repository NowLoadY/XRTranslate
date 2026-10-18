# sherpa-onnx Android libraries

This directory contains the unmodified Android **precompiled library archive**
from [sherpa-onnx v1.13.8](https://github.com/k2-fsa/sherpa-onnx/releases/tag/v1.13.8),
not a source checkout. The upstream revision is
`11afbd009a7f8c08f4bcf2fc1b265d0df4670fbf`.

- File: `sherpa-onnx-v1.13.8-android.tar.bz2`
- Source: https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.8/sherpa-onnx-v1.13.8-android.tar.bz2
- Size: 46,093,321 bytes
- SHA-256: `2ff63469a71cb6009aa2e3ed5f4a670f8abdcbe4bb9ffd23776afc792a6b4f44`
- Verified against the upstream release asset digest on 2026-10-18.

`scripts/android.py` supplies this directory through the upstream Rust binding's
`SHERPA_ONNX_ARCHIVE_DIR` option and reads the same archive when packaging the APK.
No GitHub download is needed for this Android dependency, including after
`cargo clean`. A missing archive is an error. Other dependencies retain their own
build procedures.

The original archive contains four ABIs. XRTranslate packages only the C API and
ONNX Runtime libraries for its supported `arm64-v8a` and `x86_64` targets. The
existing Android packaging step adjusts runtime search paths on extracted copies;
the archive remains unchanged. Rust bindings remain pinned by `Cargo.lock`.

To upgrade, replace the archive with the matching official release, verify its
release digest, update `SHERPA_ARCHIVE` and the Rust dependency lock together, and
refresh this note and the license notices. Test with a clean Cargo target directory
because the upstream binding reuses extracted libraries in its target cache.

## Licenses

`LICENSE` is sherpa-onnx's Apache-2.0 license at the revision above. The binaries
also include third-party components with their own terms:

- `licenses/sherpa-ThirdPartyNotices.txt` retains component licenses and source URLs.
- `licenses/onnxruntime-LICENSE` and `licenses/onnxruntime-ThirdPartyNotices.txt`
  come from [ONNX Runtime v1.28.2](https://github.com/microsoft/onnxruntime/tree/v1.28.2),
  the version included in this archive.
