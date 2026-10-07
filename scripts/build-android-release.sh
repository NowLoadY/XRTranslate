#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  printf '%s\n' \
    'Usage: ./scripts/build-android-release.sh [--abis arm64-v8a,x86_64]' \
    'Build a signed release APK in dist/ (default: arm64-v8a).'
  exit 0
fi

SIGNING_ENV_FILE="${XRT_ANDROID_SIGNING_ENV:-${XDG_CONFIG_HOME:-${HOME}/.config}/xrtranslate/signing/android-release.env}"
if [[ -n "${XRT_ANDROID_SIGNING_ENV:-}" ]]; then
  source "${SIGNING_ENV_FILE}"
elif [[ -z "${XRT_ANDROID_KEYSTORE:-}" && -r "${SIGNING_ENV_FILE}" ]]; then
  source "${SIGNING_ENV_FILE}"
fi

for setting in XRT_ANDROID_KEYSTORE XRT_ANDROID_KEY_ALIAS XRT_ANDROID_STORE_PASSWORD XRT_ANDROID_KEY_PASSWORD; do
  if [[ -z "${!setting:-}" ]]; then
    printf 'Set %s or load a signing environment file with XRT_ANDROID_SIGNING_ENV.\n' "${setting}" >&2
    exit 2
  fi
done

exec python3 "${ROOT_DIR}/scripts/android.py" build "$@" --profile release
