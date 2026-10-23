#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$(readlink -f "$0")")" && pwd)"
CLIENT="${ROOT_DIR}/bin/rust-client"
missing="$(LC_ALL=C ldd "${CLIENT}" | awk '/=> not found/ {print $1}' || true)"
if [[ -z "${missing}" ]]; then
  exec "${CLIENT}" "$@"
fi

# Only suggest packages for dependencies actually missing on this machine.
packages=()
unknown=()
while IFS= read -r library; do
  case "${library}" in
    libmpv.so.2) packages+=(libmpv2) ;;
    libpipewire-0.3.so.0) packages+=(libpipewire-0.3-0) ;;
    *) unknown+=("${library}") ;;
  esac
done <<< "${missing}"

if [[ "${LC_ALL:-${LC_MESSAGES:-${LANG:-}}}" == zh* ]]; then
  message="缺少运行依赖。安装后重新打开 XRTranslate。"
else
  message="Missing runtime dependencies. Install them, then reopen XRTranslate."
fi
if command -v apt-get >/dev/null 2>&1 && ((${#packages[@]})); then
  message+=$'\n\nsudo apt install '"${packages[*]}"
else
  unknown+=("${packages[@]}")
fi
if ((${#unknown[@]})); then
  message+=$'\n\n'"${unknown[*]}"
fi
printf '%s\n' "${message}" >&2
if command -v zenity >/dev/null 2>&1; then
  printf '%s\n' "${message}" | zenity --text-info --title=XRTranslate --width=480 --height=200 || true
elif command -v kdialog >/dev/null 2>&1; then
  kdialog --title XRTranslate --error "${message}" || true
elif command -v xmessage >/dev/null 2>&1; then
  xmessage -center -title XRTranslate "${message}" || true
fi
exit 1
