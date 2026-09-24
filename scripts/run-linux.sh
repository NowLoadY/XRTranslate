#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FEATURES="${XRTRANSLATE_FEATURES:-}"

cd "${ROOT_DIR}"

if [[ ! -f "XR-Corpus/crates/core/Cargo.toml" ]]; then
  printf '%s\n' 'XR-Corpus is not initialized. Run: git submodule update --init XR-Corpus' >&2
  exit 2
fi

if ! command -v cargo >/dev/null 2>&1; then
  printf '%s\n' 'cargo is required; install Rust 1.95 or newer.' >&2
  exit 127
fi

cargo_args=(build --locked --target-dir "${ROOT_DIR}/target" -p rust-client -p xrtranslate-backend --features xrtranslate-backend/managed-ort --release)
if [[ -n "${FEATURES}" ]]; then
  cargo_args+=(--features "${FEATURES}")
fi
cargo "${cargo_args[@]}"
cargo build --locked --manifest-path XR-Corpus/Cargo.toml \
  --target-dir "${ROOT_DIR}/target" -p xr-corpus-server --release

exec "${ROOT_DIR}/target/release/rust-client" "$@"
