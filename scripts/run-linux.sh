#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FEATURES="${XRTRANSLATE_FEATURES-mpv}"

cd "${ROOT_DIR}"

if ! command -v cargo >/dev/null 2>&1; then
  printf '%s\n' 'cargo is required; install Rust 1.95 or newer.' >&2
  exit 127
fi

source "${ROOT_DIR}/scripts/linux-build-env.sh"
prepare_linux_build_env "${ROOT_DIR}"

cargo_args=(build --locked --target-dir "${ROOT_DIR}/target" -p rust-client -p xrtranslate-account-login -p xrtranslate-backend -p xrtranslate-plugin --features xrtranslate-backend/managed-ort,xrtranslate-plugin/host --release)
if [[ -n "${FEATURES}" ]]; then
  cargo_args+=(--features "${FEATURES}")
fi
cargo "${cargo_args[@]}"
cargo build --locked --manifest-path XR-Corpus/Cargo.toml \
  --target-dir "${ROOT_DIR}/target" -p xr-corpus-server --release

bash "${ROOT_DIR}/plugin-src/build.sh" "${ROOT_DIR}" "${ROOT_DIR}/plugins" \
  --target-dir "${ROOT_DIR}/target" --locked

# Use the system EGL vendor registry, independent of the invoking shell.
unset __EGL_VENDOR_LIBRARY_DIRS __EGL_VENDOR_LIBRARY_FILENAMES
exec "${ROOT_DIR}/target/release/rust-client" "$@"
