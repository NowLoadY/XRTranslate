#!/usr/bin/env bash
set -euo pipefail
SOURCE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SDK="$(cd "${1:?Pass the SDK directory}" && pwd)"
BUILDER="$SDK/xrt-plugin"
[[ -x "$BUILDER" ]] || BUILDER="$SDK/target/release/xrt-plugin"
OUTPUT="${2:?Pass the output directory}"
mkdir -p "$OUTPUT"
OUTPUT="$(cd "$OUTPUT" && pwd)"
shift 2
for source in "$SOURCE"/*; do
  [[ -f "$source/plugin.json" ]] || continue
  [[ "$(basename "$source")" == auto-input ]] && continue
  "$BUILDER" build "$source" --sdk "$SDK" --output "$OUTPUT/$(basename "$source")" "$@"
done
