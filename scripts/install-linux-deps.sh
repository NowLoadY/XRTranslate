#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "${ROOT_DIR}/scripts/linux-build-env.sh"
prepare_linux_system_deps "${XRTRANSLATE_FEATURES-mpv}"
