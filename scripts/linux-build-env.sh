#!/usr/bin/env bash

# Sourced by Linux entry points. System dependencies are managed by APT.
prepare_linux_build_env() {
  check_linux_corpus "$1" || return
  prepare_linux_system_deps "${XRTRANSLATE_FEATURES-mpv}" || return
  mkdir -p "$(realpath -m "${1}/target")" || return
  prepare_linux_sherpa "$1"
}

check_linux_corpus() {
  local root="$1"
  if [[ ! -f "${root}/XR-Corpus/crates/core/Cargo.toml" ]]; then
    printf '%s\n' 'XR-Corpus is not initialized. Run: git submodule update --init XR-Corpus' >&2
    return 1
  fi

  # Builds use the working tree, including intentional submodule development.
  if command -v git >/dev/null 2>&1; then
    case "$(git -C "${root}" submodule status -- XR-Corpus 2>/dev/null)" in
      +*)
        printf '%s\n' 'Warning: XR-Corpus differs from the recorded revision; building current local sources.' \
          'If this is unintentional, preserve local work before running: git submodule update --init XR-Corpus' >&2
        ;;
    esac
  fi
}

prepare_linux_system_deps() {
  local features="${1//,/ }" feature package architecture
  local -a missing=() apt=(apt-get)
  local -a packages=(
    build-essential pkg-config curl bzip2
    libx11-dev libxcursor-dev libxrandr-dev libxi-dev
    libwayland-dev libxkbcommon-dev libasound2-dev libpipewire-0.3-dev libclang-dev
    libwebkit2gtk-4.1-dev libgtk-3-dev libdbus-1-dev
    libegl-mesa0 libglx-mesa0 libgl1-mesa-dri
    fontconfig fonts-noto-core fonts-noto-cjk fonts-dejavu-core
  )
  local -a modules=(
    x11 xcursor xrandr xi wayland-client xkbcommon alsa
    'libpipewire-0.3 >= 0.3' libspa-0.2 webkit2gtk-4.1 gtk+-3.0 dbus-1
  )
  for feature in ${features}; do
    case "${feature}" in
      mpv|rust-client/mpv) packages+=(libmpv-dev); modules+=(mpv); break ;;
    esac
  done

  if command -v apt-get >/dev/null 2>&1 && command -v dpkg-query >/dev/null 2>&1; then
    architecture="$(dpkg --print-architecture)" || return
    for package in "${packages[@]}"; do
      # A foreign-architecture runtime alone does not satisfy a native build.
      if ! dpkg-query -W -f='${Architecture} ${db:Status-Status}\n' "${package}" 2>/dev/null |
        awk -v native="${architecture}" '($1 == native || $1 == "all") && $2 == "installed" { found=1 } END { exit !found }'; then
        missing+=("${package}")
      fi
    done
    if ((${#missing[@]})); then
      if ((EUID != 0)); then
        if ! command -v sudo >/dev/null 2>&1; then
          printf '%s\n' 'Install sudo or run scripts/install-linux-deps.sh as root.' >&2
          return 1
        fi
        apt=(sudo apt-get)
      fi
      printf 'Installing system dependencies through APT: %s\n' "${missing[*]}" >&2
      "${apt[@]}" update || return
      "${apt[@]}" install -y --no-install-recommends --no-remove "${missing[@]}" || return
    fi
  fi

  if ! command -v pkg-config >/dev/null 2>&1 || ! pkg-config --exists "${modules[@]}"; then
    printf '%s\n' 'Native build dependencies are unavailable. Install the system development packages listed in docs/linux-build.md.' >&2
    return 1
  fi
}

prepare_linux_sherpa() {
  if [[ -n "${SHERPA_ONNX_LIB_DIR:-}" || -n "${SHERPA_ONNX_ARCHIVE_DIR:-}" ]]; then
    return 0
  fi
  local root="$1" version arch name archive
  version="$(awk '$0 == "name = \"sherpa-onnx-sys\"" {found=1; next}
    found && $1 == "version" {gsub(/"/, "", $3); print $3; exit}' "${root}/Cargo.lock")"
  [[ -n "${version}" ]] || { printf '%s\n' 'sherpa-onnx-sys is missing from Cargo.lock.' >&2; return 1; }
  case "$(uname -m)" in
    x86_64) arch=x64 ;;
    aarch64) arch=aarch64 ;;
    *) return 0 ;; # Let the crate report unsupported targets.
  esac
  name="sherpa-onnx-v${version}-linux-${arch}-static-lib"
  archive="${root}/third_party/sherpa-onnx/${name}.tar.bz2"
  if [[ ! -f "${archive}" ]]; then
    printf 'Missing bundled speech libraries. Restore this file from the repository: %s\n' "${archive}" >&2
    return 1
  fi
  export SHERPA_ONNX_ARCHIVE_DIR="${root}/third_party/sherpa-onnx"
}
