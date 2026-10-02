#!/usr/bin/env bash

# Sourced by the Linux entry points; development files never enter the release.
prepare_linux_build_env() {
  local root="$1" pipewire_version='' clang_package='' clang_version='' clang_major=''
  local cache cache_version multiarch pipewire_library header pc
  local -a packages=() headers=()

  if ! command -v pkg-config >/dev/null 2>&1; then
    printf '%s\n' 'pkg-config is required; install the native dependencies in docs/linux-build.md.' >&2
    return 1
  fi
  if ! command -v dpkg-query >/dev/null 2>&1; then
    if ! pkg-config --exists 'libpipewire-0.3 >= 0.3' libspa-0.2; then
      printf '%s\n' 'PipeWire development files are required; see docs/linux-build.md.' >&2
      return 1
    fi
    return 0
  fi

  if ! pkg-config --exists 'libpipewire-0.3 >= 0.3' libspa-0.2; then
    pipewire_version="$(dpkg-query -W -f='${Version} ${db:Status-Abbrev}\n' \
      'libpipewire-0.3-0*' 2>/dev/null | awk '$2 == "ii" && !found {print $1; found=1}' || true)"
    if [[ -z "${pipewire_version}" ]]; then
      printf '%s\n' 'Install libpipewire-0.3-dev and libclang-dev first; see docs/linux-build.md.' >&2
      return 1
    fi
    packages+=("libpipewire-0.3-dev=${pipewire_version}" "libspa-0.2-dev=${pipewire_version}")
  fi

  # libclang can be installed without its builtin C headers on desktop systems.
  if [[ -z "${LIBCLANG_PATH:-}" && -z "${BINDGEN_EXTRA_CLANG_ARGS:-}" ]]; then
    read -r clang_package clang_version <<< "$(dpkg-query -W \
      -f='${Package} ${db:Status-Abbrev} ${Version}\n' 'libclang1-*' 2>/dev/null \
      | awk '$2 == "ii" {print $1, $3}' | sort -V | tail -n 1 || true)"
    clang_major="${clang_package#libclang1-}"
    if [[ -n "${clang_major}" ]]; then
      headers=(/usr/lib/llvm-"${clang_major}"/lib/clang/*/include/stddef.h)
      if [[ ! -f "${headers[0]}" ]]; then
        packages+=("libclang-common-${clang_major}-dev=${clang_version}")
      fi
    fi
  fi
  if [[ ${#packages[@]} -eq 0 ]]; then
    return 0
  fi

  multiarch="$(dpkg-architecture -qDEB_HOST_MULTIARCH)"
  cache_version="${pipewire_version:-system}-${clang_version:-system}"
  cache="${root}/target/linux-build-deps/${multiarch}/${cache_version//:/_}"
  if [[ ! -f "${cache}/ready" ]]; then
    mkdir -p "${cache}/archives"
    printf '%s\n' 'Preparing missing Linux development files in target/linux-build-deps (no sudo required).' >&2
    if ! (cd "${cache}/archives" && apt-get download "${packages[@]}"); then
      printf '%s\n' 'Download failed. Install libpipewire-0.3-dev and libclang-dev, then retry.' >&2
      return 1
    fi
    for header in "${cache}"/archives/*.deb; do
      dpkg-deb -x "${header}" "${cache}"
    done
    if [[ -n "${pipewire_version}" ]]; then
      for pc in "${cache}/usr/lib/${multiarch}/pkgconfig/"*.pc; do
        sed -i 's|^prefix=/usr$|prefix=${pcfiledir}/../../..|' "${pc}"
      done
      pipewire_library="$(ldconfig -p | awk '/libpipewire-0[.]3[.]so[.]0 / && !found {print $NF; found=1}')"
      if [[ ! -f "${pipewire_library}" ]]; then
        printf '%s\n' 'The system PipeWire runtime library is missing.' >&2
        return 1
      fi
      ln -sf "${pipewire_library}" "${cache}/usr/lib/${multiarch}/libpipewire-0.3.so"
    fi
    touch "${cache}/ready"
    rm -r -- "${cache}/archives"
  fi
  if [[ -n "${pipewire_version}" ]]; then
    export PKG_CONFIG_PATH="${cache}/usr/lib/${multiarch}/pkgconfig${PKG_CONFIG_PATH:+:${PKG_CONFIG_PATH}}"
  fi
  headers=("${cache}"/usr/lib/llvm-"${clang_major}"/lib/clang/*/include/stddef.h)
  if [[ -n "${clang_major}" && -f "${headers[0]}" ]]; then
    export BINDGEN_EXTRA_CLANG_ARGS="-I'${headers[0]%/stddef.h}'"
  fi
}
