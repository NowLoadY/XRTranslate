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

bundled_resources=(
  models/silero-vad/src/silero_vad/data/silero_vad.onnx
  models/3D-Speaker-ERes2NetV2/speaker_embedding.onnx
  models/gtcrn/gtcrn_simple.onnx
  runtime/onnxruntime/cpu/libonnxruntime.so.1.28.0
)
for resource in XR-Corpus/corpora/default.sqlite "${bundled_resources[@]}"; do
  if [[ ! -f "${resource}" ]]; then
    printf 'Required release resource is missing: %s\n' "${resource}" >&2
    exit 2
  fi
done

cargo_args=(build --locked --target-dir "${ROOT_DIR}/target" -p rust-client -p xrtranslate-backend --features xrtranslate-backend/managed-ort --release)
if [[ -n "${FEATURES}" ]]; then
  cargo_args+=(--features "${FEATURES}")
fi
cargo "${cargo_args[@]}"
cargo build --locked --manifest-path XR-Corpus/Cargo.toml \
  --target-dir "${ROOT_DIR}/target" -p xr-corpus-server --release

mkdir -p "$(dirname "${TARGET_DIR}")"
STAGE_DIR="$(mktemp -d "${TARGET_DIR}.tmp.XXXXXX")"
trap 'rm -r -- "${STAGE_DIR}"' EXIT
mkdir -p "${STAGE_DIR}/bin" "${STAGE_DIR}/resources" "${STAGE_DIR}/XR-Corpus" "${STAGE_DIR}/corpora" "${STAGE_DIR}/runtime"
install -m 0755 target/release/rust-client "${STAGE_DIR}/xrtranslate"
install -m 0755 target/release/xrtranslate-backend target/release/xr-corpus-server "${STAGE_DIR}/bin/"
sed 's|XR-Corpus/corpora/default.sqlite|corpora/default.sqlite|' config.json > "${STAGE_DIR}/config.json"
install -m 0644 LICENSE LICENSE-MIT "${STAGE_DIR}/"
install -m 0644 XR-Corpus/LICENSE "${STAGE_DIR}/XR-Corpus/"
cp -a rust-client/resources/{branding,icons,plugins} "${STAGE_DIR}/resources/"
install -m 0644 XR-Corpus/corpora/default.sqlite "${STAGE_DIR}/corpora/default.sqlite"
for resource in "${bundled_resources[@]}"; do
  install -D -m 0644 "${resource}" "${STAGE_DIR}/${resource}"
done
mv -- "${STAGE_DIR}" "${TARGET_DIR}"
trap - EXIT

printf 'Linux release staged at %s\n' "${TARGET_DIR}"
