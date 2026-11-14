#!/usr/bin/env python3
"""Unpack repository-owned sherpa libraries; never access the network."""

import argparse
from pathlib import Path
import tarfile
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parent.parent


def bundled_version(root=ROOT):
    lock = tomllib.loads((root / 'Cargo.lock').read_text())
    versions = []
    for name in ('sherpa-onnx', 'sherpa-onnx-sys'):
        matches = [p['version'] for p in lock['package'] if p['name'] == name]
        if len(matches) != 1:
            raise ValueError(f'Expected exactly one {name} version in Cargo.lock; got {matches}')
        versions.extend(matches)
    if versions[0] != versions[1]:
        raise ValueError(f'Sherpa Rust bindings and native ABI versions differ: {versions}')
    return versions[0]


def prepare(platform, abi=None, root=ROOT):
    version = bundled_version(root)
    suffix = {
        'windows-x64': 'win-x64-static-MD-Release-lib',
        'linux-x64': 'linux-x64-static-lib',
        'linux-aarch64': 'linux-aarch64-static-lib',
        'android': 'android',
    }[platform]
    name = f'sherpa-onnx-v{version}-{suffix}'
    extension = '.tar.xz' if platform == 'windows-x64' else '.tar.bz2'
    archive = root / 'third_party/sherpa-onnx' / (name + extension)
    if not archive.is_file():
        raise FileNotFoundError(f'Missing bundled sherpa archive; restore it from the repository: {archive}')
    if platform == 'android' and abi not in ('arm64-v8a', 'x86_64'):
        raise ValueError('Android sherpa requires arm64-v8a or x86_64')
    cache = root / 'target/sherpa-onnx-prebuilt'
    # Android's upstream archive has an unversioned jniLibs root. Give every
    # archive its own cache to prevent a warm build linking old ABI definitions.
    # Keep our completed extractions separate from upstream's direct-Cargo cache.
    extracted = cache / ('bundled-' + name)
    relative = Path('jniLibs' if platform == 'android' else name + '/lib')
    if platform == 'android':
        relative /= abi
    required = ('libsherpa-onnx-c-api.so', 'libonnxruntime.so') if platform == 'android' else (
        ('sherpa-onnx-c-api.lib', 'onnxruntime.lib', 'utf8proc.lib') if platform == 'windows-x64' else
        ('libsherpa-onnx-c-api.a', 'libonnxruntime.a', 'libutf8proc.a'))

    def complete(directory):
        return (directory / '.complete').is_file() and all((directory / relative / name).is_file() for name in required)

    if extracted.exists() and not complete(extracted):
        raise ValueError(f'Incomplete sherpa cache; remove this generated directory and retry: {extracted}')
    if not complete(extracted):
        cache.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(prefix='.extract-', dir=cache) as temporary:
            staging = Path(temporary) / 'libraries'
            staging.mkdir()
            with tarfile.open(archive) as contents:
                contents.extractall(staging, filter='data')
            (staging / '.complete').touch()
            if not complete(staging):
                raise ValueError(f'Bundled sherpa archive is missing required libraries: {archive}')
            try:
                staging.rename(extracted)
            except OSError:
                # Two independent Cargo builds can prepare the same bundle.
                if not complete(extracted):
                    raise
    return extracted / relative


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--platform', required=True, choices=('windows-x64', 'linux-x64', 'linux-aarch64', 'android'))
    parser.add_argument('--abi', choices=('arm64-v8a', 'x86_64'))
    args = parser.parse_args()
    print(prepare(args.platform, args.abi))


if __name__ == '__main__':
    main()
