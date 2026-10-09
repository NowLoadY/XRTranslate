#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET_DIR="${XRTRANSLATE_RELEASE_DIR:-${ROOT_DIR}/dist/linux-x86_64}"
FEATURES="${XRTRANSLATE_FEATURES:-}"

cd "${ROOT_DIR}"

if [[ "$(uname -s)" != "Linux" || "$(uname -m)" != "x86_64" ]]; then
  printf '%s\n' 'This release script targets Linux x86_64.' >&2
  exit 2
fi

if ! command -v cargo >/dev/null 2>&1; then
  printf '%s\n' 'cargo is required; install Rust 1.95 or newer.' >&2
  exit 127
fi

if [[ ! -f "XR-Corpus/crates/core/Cargo.toml" ]]; then
  printf '%s\n' 'XR-Corpus is not initialized. Run: git submodule update --init XR-Corpus' >&2
  exit 2
fi

if [[ -e "${TARGET_DIR}" ]]; then
  printf 'Release directory already exists: %s\n' "${TARGET_DIR}" >&2
  exit 2
fi

source "${ROOT_DIR}/scripts/linux-build-env.sh"
prepare_linux_build_env "${ROOT_DIR}"

cargo_args=(build --locked --target-dir "${ROOT_DIR}/target" -p rust-client -p xrtranslate-backend -p xrtranslate-installer -p xrtranslate-updater -p xrtranslate-packager --features xrtranslate-backend/managed-ort --release)
if [[ -n "${FEATURES}" ]]; then
  cargo_args+=(--features "${FEATURES}")
fi
cargo "${cargo_args[@]}"
cargo build --locked --manifest-path XR-Corpus/Cargo.toml \
  --target-dir "${ROOT_DIR}/target" -p xr-corpus-server --release

RESOURCE_DIR="${ROOT_DIR}/target/release-resources/linux-x86_64"
mkdir -p "${RESOURCE_DIR}"
# Use distributable defaults; private configuration and installed large models stay local.
git show HEAD:config.json > "${RESOURCE_DIR}/config.json"
target/release/xrtranslate-installer --config "${RESOURCE_DIR}/config.json" \
  prepare-resources --output "${RESOURCE_DIR}" --cache "${RESOURCE_DIR}/downloads"

ONNX_ARCHIVE="${RESOURCE_DIR}/onnxruntime-linux-x64-1.28.0.tgz"
cargo run --locked --release -p xrtranslate-download --example fetch -- \
  https://github.com/microsoft/onnxruntime/releases/download/v1.28.0/onnxruntime-linux-x64-1.28.0.tgz \
  9125960 "${ONNX_ARCHIVE}"
tar -xzf "${ONNX_ARCHIVE}" -C "${RESOURCE_DIR}" \
  onnxruntime-linux-x64-1.28.0/lib/libonnxruntime.so.1.28.0 \
  onnxruntime-linux-x64-1.28.0/LICENSE \
  onnxruntime-linux-x64-1.28.0/ThirdPartyNotices.txt
ONNX_DIR="${RESOURCE_DIR}/onnxruntime-linux-x64-1.28.0"

target/release/xrtranslate-packager \
  --rust-client-bin target/release/rust-client \
  --backend-bin target/release/xrtranslate-backend \
  --corpus-bin target/release/xr-corpus-server \
  --installer-bin target/release/xrtranslate-installer \
  --updater-bin target/release/xrtranslate-updater \
  --config "${RESOURCE_DIR}/config.json" \
  --resources-dir rust-client/resources \
  --seed-database XR-Corpus/corpora/default.sqlite \
  --onnx-runtime-cpu "${ONNX_DIR}/lib/libonnxruntime.so.1.28.0" \
  --onnx-runtime-license "${ONNX_DIR}/LICENSE" \
  --onnx-runtime-notices "${ONNX_DIR}/ThirdPartyNotices.txt" \
  --output "${TARGET_DIR}"
ln -s "$(basename "${TARGET_DIR}"/XRTranslate-v*)" "${TARGET_DIR}/xrtranslate"

printf 'Linux release staged at %s\n' "${TARGET_DIR}"
