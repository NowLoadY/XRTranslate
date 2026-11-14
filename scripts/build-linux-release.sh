#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET_DIR="${XRTRANSLATE_RELEASE_DIR:-${ROOT_DIR}/dist/linux-x86_64}"
FEATURES="${XRTRANSLATE_FEATURES-mpv}"

cd "${ROOT_DIR}"

if [[ "$(uname -s)" != "Linux" || "$(uname -m)" != "x86_64" ]]; then
  printf '%s\n' 'This release script targets Linux x86_64.' >&2
  exit 2
fi

if ! command -v cargo >/dev/null 2>&1; then
  printf '%s\n' 'cargo is required; install Rust 1.95 or newer.' >&2
  exit 127
fi

if [[ -e "${TARGET_DIR}" ]]; then
  printf 'Release directory already exists: %s\n' "${TARGET_DIR}" >&2
  exit 2
fi

source "${ROOT_DIR}/scripts/linux-build-env.sh"
prepare_linux_build_env "${ROOT_DIR}"

RESOURCE_DIR="${ROOT_DIR}/target/release-resources/linux-x86_64"
mkdir -p "${RESOURCE_DIR}"
ONNX_DIR="${RESOURCE_DIR}/onnxruntime-linux-x64-1.28.0"
tar -xzf "${ROOT_DIR}/third_party/onnxruntime/onnxruntime-linux-x64-1.28.0.tgz" -C "${RESOURCE_DIR}" \
  onnxruntime-linux-x64-1.28.0/lib/libonnxruntime.so.1.28.0 \
  onnxruntime-linux-x64-1.28.0/LICENSE \
  onnxruntime-linux-x64-1.28.0/ThirdPartyNotices.txt

cargo_args=(build --locked --target-dir "${ROOT_DIR}/target" -p rust-client -p xrtranslate-account-login -p xrtranslate-backend -p xrtranslate-installer -p xrtranslate-updater -p xrtranslate-packager -p xrtranslate-plugin --features xrtranslate-backend/managed-ort,xrtranslate-plugin/host --release)
if [[ -n "${FEATURES}" ]]; then
  cargo_args+=(--features "${FEATURES}")
fi
cargo "${cargo_args[@]}"
cargo build --locked --manifest-path XR-Corpus/Cargo.toml \
  --target-dir "${ROOT_DIR}/target" -p xr-corpus-server --release

PLUGIN_DIR="$(mktemp -d "${TMPDIR:-/tmp}/xrtranslate-release-plugins.XXXXXX")"
trap 'rm -rf -- "$PLUGIN_DIR"' EXIT
bash "${ROOT_DIR}/plugin-src/build.sh" "${ROOT_DIR}" "${PLUGIN_DIR}" \
  --target-dir "${ROOT_DIR}/target" --locked

# Use distributable defaults; private configuration and installed large models stay local.
target/release/xrtranslate-installer --config config.json \
  export-defaults --output "${RESOURCE_DIR}/config.json"
target/release/xrtranslate-installer --config "${RESOURCE_DIR}/config.json" \
  prepare-resources --output "${RESOURCE_DIR}" --source "${ROOT_DIR}/third_party"

plugin_args=()
for directory in "${PLUGIN_DIR}"/*; do
  [[ -f "${directory}/plugin.json" ]] || continue
  plugin_args+=(--plugin-directory "${directory}")
done
if [[ ${#plugin_args[@]} -eq 0 ]]; then
  printf '%s\n' 'No public plugin packages were built.' >&2
  exit 2
fi

target/release/xrtranslate-packager "${plugin_args[@]}" \
  --rust-client-bin target/release/rust-client \
  --backend-bin target/release/xrtranslate-backend \
  --account-login-bin target/release/xrtranslate-account-login \
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
