#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BINARY="${XRTRANSLATE_BINARY:-${ROOT_DIR}/dist/linux-x86_64/xrtranslate}"
FEATURES="${XRTRANSLATE_FEATURES:-}"

if [[ -x "${BINARY}" ]]; then
  cd "$(dirname "${BINARY}")"
  exec "./$(basename "${BINARY}")" "$@"
fi

cd "${ROOT_DIR}"

if [[ ! -f "XR-Corpus/crates/core/Cargo.toml" ]]; then
  printf '%s\n' 'XR-Corpus is not initialized. Run: git submodule update --init XR-Corpus' >&2
  exit 2
fi

if ! command -v cargo >/dev/null 2>&1; then
  printf '%s\n' 'cargo is required; install Rust 1.95 or newer.' >&2
  exit 127
fi

cargo build --locked --target-dir "${ROOT_DIR}/target" -p xrtranslate-backend --features managed-ort --release
cargo build --locked --manifest-path XR-Corpus/Cargo.toml \
  --target-dir "${ROOT_DIR}/target" -p xr-corpus-server --release

cargo_args=(run --locked --target-dir "${ROOT_DIR}/target" -p rust-client --release)
if [[ -n "${FEATURES}" ]]; then
  cargo_args+=(--features "${FEATURES}")
fi
exec cargo "${cargo_args[@]}" -- "$@"
