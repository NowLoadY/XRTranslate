#!/usr/bin/env python3
"""Build and run XRTranslate for Android. Run with --help for prerequisites."""
import argparse
import tarfile
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import struct
import shlex
import re
import tomllib

ROOT = Path(__file__).resolve().parent.parent
NDK_VERSION = '29.0.14206865'
TARGETS = {'arm64-v8a': ('aarch64-linux-android', 'aarch64-linux-android'), 'x86_64': ('x86_64-linux-android', 'x86_64-linux-android')}

def release_metadata(abis):
    version = tomllib.loads((ROOT / 'Cargo.toml').read_text())['workspace']['package']['version']
    match = re.fullmatch(r'(\d+)\.(\d+)\.(\d+)(?:-beta\.(\d+))?', version)
    if not match: raise SystemExit('Android releases require major.minor.patch or major.minor.patch-beta.N.')
    major, minor, patch = map(int, match.group(1, 2, 3))
    beta = int(match[4]) if match[4] is not None else 999
    if major > 209 or minor > 99 or patch > 99 or (beta > 998 and match[4] is not None):
        raise SystemExit('Android version components exceed the supported range (209.99.99, beta.998).')
    code = ((major * 100 + minor) * 100 + patch) * 1000 + beta
    if code <= 0: raise SystemExit('Android versionCode must be positive.')
    architecture = {'arm64-v8a': 'arm64', 'x86_64': 'x64'}.get(abis, 'universal')
    return {'version': version, 'code': code, 'name': f'XRTranslate-v{version}-android-{architecture}'}

def run(*command, env=None, cwd=ROOT):
    subprocess.run([str(arg) for arg in command], cwd=cwd, env=env, check=True)

def sdk_path():
    configured = os.environ.get('ANDROID_HOME') or os.environ.get('ANDROID_SDK_ROOT')
    if configured:
        return Path(configured).expanduser().resolve()
    local = ROOT / 'android/local.properties'
    if local.exists():
        for line in local.read_text().splitlines():
            if line.startswith('sdk.dir='):
                return Path(line.split('=', 1)[1].replace('\\:', ':').replace('\\\\', '\\')).expanduser().resolve()
    candidates = [Path.home() / 'Android/Sdk', Path.home() / 'Library/Android/sdk', Path(os.environ.get('LOCALAPPDATA', str(Path.home()))) / 'Android/Sdk']
    for candidate in candidates:
        if candidate.is_dir(): return candidate
    raise SystemExit('Set ANDROID_HOME to your Android SDK directory, or configure android/local.properties in Android Studio.')

def toolchain(sdk, abi):
    target, clang_target = TARGETS[abi]
    ndk = sdk / 'ndk' / NDK_VERSION
    host = {'Linux': 'linux-x86_64', 'Darwin': 'darwin-x86_64', 'Windows': 'windows-x86_64'}[platform.system()]
    binaries = ndk / 'toolchains/llvm/prebuilt' / host / 'bin'
    suffix = '.cmd' if os.name == 'nt' else ''
    compiler = binaries / (clang_target + '29-clang' + suffix)
    if not compiler.exists(): raise SystemExit(f'Install NDK {NDK_VERSION} using Android Studio or sdkmanager.')
    environment = dict(os.environ, ANDROID_HOME=str(sdk), ANDROID_NDK_HOME=str(ndk), CARGO_TARGET_DIR=str(ROOT / 'target'))
    environment.setdefault('CARGO_BUILD_JOBS', '2')
    key = target.replace('-', '_')
    environment[f'CC_{key}'] = str(compiler)
    environment[f'CXX_{key}'] = str(binaries / (clang_target + '29-clang++' + suffix))
    environment[f'AR_{key}'] = str(binaries / ('llvm-ar.exe' if os.name == 'nt' else 'llvm-ar'))
    environment[f'CARGO_TARGET_{key.upper()}_LINKER'] = str(compiler)
    flags = environment.get('CARGO_ENCODED_RUSTFLAGS')
    flags = flags.split('\x1f') if flags else shlex.split(environment.get('RUSTFLAGS', ''))
    flags += ['-C', 'link-arg=-Wl,-z,max-page-size=16384', '--remap-path-prefix=' + str(ROOT) + '=/xrtranslate', '--remap-path-prefix=' + str(Path(environment.get('CARGO_HOME', Path.home() / '.cargo')).resolve()) + '=/cargo']
    environment['CARGO_ENCODED_RUSTFLAGS'] = '\x1f'.join(flags)
    prefix_flags = [f'-ffile-prefix-map={ROOT}=/xrtranslate', f'-ffile-prefix-map={Path(environment.get("CARGO_HOME", Path.home() / ".cargo")).resolve()}=/cargo']
    for variable in ['CFLAGS', 'CXXFLAGS']:
        variable += '_' + key
        environment[variable] = environment.get(variable, '') + ' ' + ' '.join(shlex.quote(flag) for flag in prefix_flags)
    archive_directory = ROOT / 'target/android/sherpa'
    archive = archive_directory / 'sherpa-onnx-v1.13.8-android.tar.bz2'
    fetch('https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.8/' + archive.name, 46093321, archive)
    environment['SHERPA_ONNX_ARCHIVE_DIR'] = str(archive_directory)
    return target, environment

def fetch(url, size, destination):
    run('cargo', 'run', '--locked', '-p', 'xrtranslate-download', '--example', 'fetch', '--', url, str(size), destination)

def llama_server(sdk, abi, destination):
    revision = '08659901c43b51de735740f1cf61bb82fbe0c4e4'
    cache = ROOT / 'target/android/llama'
    source = cache / ('llama.cpp-' + revision)
    if not source.exists():
        cache.mkdir(parents=True, exist_ok=True)
        archive = cache / 'source.tar.gz'
        fetch('https://codeload.github.com/ggml-org/llama.cpp/tar.gz/' + revision, 36736254, archive)
        with tarfile.open(archive) as contents: contents.extractall(cache, filter='data')
    # This pinned source archive has explicit build metadata and intentionally no Web UI.
    for relative, original, replacement in [
        ('common/CMakeLists.txt', 'else()\n    message(WARNING "Git repository not found;', 'elseif(NOT DEFINED LLAMA_BUILD_NUMBER OR NOT DEFINED LLAMA_BUILD_COMMIT)\n    message(WARNING "Git repository not found;'),
        ('scripts/ui-assets.cmake', 'if(NOT provisioned)\n    if(EXISTS "${DIST_DIR}/index.html")', 'if(NOT provisioned AND (BUILD_UI OR HF_ENABLED))\n    if(EXISTS "${DIST_DIR}/index.html")'),
    ]:
        path = source / relative
        content = path.read_text()
        if replacement not in content:
            if content.count(original) != 1: raise SystemExit(f'Unexpected llama.cpp source: {relative}')
            path.write_text(content.replace(original, replacement))
    build = cache / abi
    generator = ['-G', 'Ninja'] if os.name == 'nt' else []
    if os.name == 'nt' and not shutil.which('ninja'): raise SystemExit('Install Ninja and add it to PATH for Android builds on Windows.')
    prefix_map = '"-ffile-prefix-map=' + ROOT.as_posix() + '=/xrtranslate"'
    run('cmake', *generator, '-S', source, '-B', build, '-DCMAKE_BUILD_TYPE=Release', '-DCMAKE_TOOLCHAIN_FILE=' + str(sdk / 'ndk' / NDK_VERSION / 'build/cmake/android.toolchain.cmake'), '-DANDROID_ABI=' + abi, '-DANDROID_PLATFORM=android-29', '-DBUILD_SHARED_LIBS=OFF', '-DGGML_NATIVE=OFF', '-DGGML_OPENMP=OFF', '-DGGML_LLAMAFILE=OFF', '-DLLAMA_OPENSSL=OFF', '-DLLAMA_BUILD_TESTS=OFF', '-DLLAMA_BUILD_EXAMPLES=OFF', '-DLLAMA_BUILD_UI=OFF', '-DLLAMA_USE_PREBUILT_UI=OFF', '-DLLAMA_BUILD_APP=OFF', '-DLLAMA_BUILD_NUMBER=10333', '-DLLAMA_BUILD_COMMIT=08659901', '-DCMAKE_EXE_LINKER_FLAGS=-Wl,-z,max-page-size=16384', '-DCMAKE_C_FLAGS=' + prefix_map, '-DCMAKE_CXX_FLAGS=' + prefix_map)
    run('cmake', '--build', build, '--target', 'llama-server', '--parallel', str(min(os.cpu_count() or 2, 4)))
    shutil.copy2(build / 'bin/llama-server', destination / 'libllama-server.so')

def prepare_elf(path):
    """Validate 16 KiB support and keep native lookup relative to the APK."""
    data = bytearray(path.read_bytes())
    if data[:6] != b'\x7fELF\x02\x01': raise SystemExit(f'Expected a 64-bit Android library: {path.name}')
    offset, = struct.unpack_from('<Q', data, 32)
    size, count = struct.unpack_from('<HH', data, 54)
    if size < 56 or offset + count * size > len(data): raise SystemExit('Invalid ELF program headers')
    segments = [struct.unpack_from('<IIQQQQQQ', data, offset + index * size) for index in range(count)]
    for kind, _, file_offset, _, _, _, _, alignment in segments:
        if kind == 1 and alignment < 16384: raise SystemExit(f'{path.name} does not support 16 KiB pages')
    dynamic = next((segment for segment in segments if segment[0] == 2), None)
    if dynamic:
        entries = []
        for index in range(dynamic[2], dynamic[2] + dynamic[5], 16):
            tag, value = struct.unpack_from('<QQ', data, index)
            if tag == 0: break
            entries.append((tag, value))
        table = next((value for tag, value in entries if tag == 5), None)
        if table is not None:
            segment = next(item for item in segments if item[0] == 1 and item[3] <= table < item[3] + item[5])
            strings = segment[2] + table - segment[3]
            for tag, value in entries:
                if tag not in (15, 29): continue
                start = strings + value
                end = data.index(0, start)
                if end - start < 7: raise SystemExit('Native search path is too short to normalize')
                if any(other in (1,14) and value <= index < value + end - start for other, index in entries): raise SystemExit('Native search path overlaps a dependency name')
                data[start:end] = b'$ORIGIN' + b'\0' * (end - start - 7)
    path.write_bytes(data)

def native(sdk, args):
    generated = ROOT / 'android/app/build/generated/rust'
    metadata = json.loads(subprocess.check_output(['cargo', 'metadata', '--locked', '--format-version', '1'], cwd=ROOT))
    verifier = next(package for package in metadata['packages'] if package['name'] == 'rustls-platform-verifier-android')
    aar = next(Path(verifier['manifest_path']).parent.glob('maven/**/*.aar'))
    generated.mkdir(parents=True, exist_ok=True)
    shutil.copy2(aar, generated / 'rustls-platform-verifier.aar')
    cargo_profile = ['--release'] if args.profile == 'release' else []
    output_profile = 'release' if cargo_profile else 'debug'
    # Generated package contents must not retain old ABIs, runtime files, or local data.
    jni_libraries = generated / args.profile / 'jniLibs'
    if jni_libraries.exists(): shutil.rmtree(jni_libraries)
    for abi in args.abis.split(','):
        if abi not in TARGETS: raise SystemExit(f'Unsupported ABI: {abi}')
        target, environment = toolchain(sdk, abi)
        destination = generated / args.profile / 'jniLibs' / abi
        destination.mkdir(parents=True, exist_ok=True)
        run('cargo', 'rustc', '--locked', '-p', 'rust-client', '--lib', '--target', target, '--crate-type', 'cdylib', *cargo_profile, env=environment)
        shutil.copy2(ROOT / 'target' / target / output_profile / 'librust_client.so', destination / 'librust_client.so')
        run('cargo', 'build', '--locked', '-p', 'xrtranslate-backend', '--features', 'managed-ort', '--target', target, *cargo_profile, env=environment)
        shutil.copy2(ROOT / 'target' / target / output_profile / 'xrtranslate-backend', destination / 'libxrtranslate-backend.so')
        run('cargo', 'build', '--locked', '--manifest-path', 'XR-Corpus/Cargo.toml', '--target-dir', ROOT / 'target', '-p', 'xr-corpus-server', '--target', target, *cargo_profile, env=environment)
        shutil.copy2(ROOT / 'target' / target / output_profile / 'xr-corpus-server', destination / 'libxr-corpus-server.so')
        llama_server(sdk, abi, destination)
        sherpa_cache = ROOT / 'target/android/sherpa'
        with tarfile.open(sherpa_cache / 'sherpa-onnx-v1.13.8-android.tar.bz2') as archive:
            for name in ['libsherpa-onnx-c-api.so', 'libonnxruntime.so']:
                library = next(member for member in archive.getmembers() if member.isfile() and abi in Path(member.name).parts and Path(member.name).name == name)
                with archive.extractfile(library) as source, (destination / name).open('wb') as output:
                    shutil.copyfileobj(source, output)
        for unused in ['libsherpa-onnx-cxx-api.so', 'libsherpa-onnx-jni.so']:
            (destination / unused).unlink(missing_ok=True)
        for library in destination.glob('*.so'): prepare_elf(library)
    assets = generated / args.profile / 'assets/application'
    assets.mkdir(parents=True, exist_ok=True)
    packaged_assets = {
        'application/config.json',
        'application/XR-Corpus/corpora/default.sqlite',
        'application/models/silero-vad/src/silero_vad/data/silero_vad.onnx',
        'application/models/3D-Speaker-ERes2NetV2/speaker_embedding.onnx',
        'application/models/gtcrn/gtcrn_simple.onnx',
    }
    for path in assets.parent.rglob('*'):
        if path.is_symlink() or (path.is_file() and path.relative_to(assets.parent).as_posix() not in packaged_assets):
            path.unlink()
    # Only the tracked defaults and public seed are packaged.
    default_config = subprocess.check_output(['git', 'show', 'HEAD:config.json'], cwd=ROOT)
    (assets / 'config.json').write_bytes(default_config)
    run('cargo', 'run', '--locked', '-p', 'xrtranslate-installer', '--', '--config', assets / 'config.json', 'prepare-resources', '--output', assets, '--cache', ROOT / 'target/android/resources-cache')
    seed = assets / 'XR-Corpus/corpora/default.sqlite'
    seed.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(ROOT / 'XR-Corpus/corpora/default.sqlite', seed)

def java_environment(sdk):
    environment = dict(os.environ, ANDROID_HOME=str(sdk))
    configured = environment.get('JAVA_HOME')
    if not configured:
        compiler = shutil.which('javac')
        if not compiler: raise SystemExit('Install a full JDK 17 or newer and set JAVA_HOME.')
        configured = str(Path(compiler).resolve().parent.parent)
    if not (Path(configured) / 'bin' / ('javac.exe' if os.name == 'nt' else 'javac')).exists():
        raise SystemExit('JAVA_HOME must point to a full JDK, including javac.')
    environment['JAVA_HOME'] = configured
    return environment

def main():
    parser = argparse.ArgumentParser(description='Android build entry. Requires Rust, Python 3.12+, CMake (plus Ninja on Windows), JDK 17+, Android SDK 36, build-tools 36.1.0, NDK ' + NDK_VERSION + ', and initialized XR-Corpus submodule. SDK location: ANDROID_HOME or android/local.properties.')
    parser.add_argument('command', choices=['doctor', 'setup', 'native', 'build', 'run', 'metadata'])
    parser.add_argument('--abis', default='arm64-v8a')
    parser.add_argument('--profile', choices=['debug', 'release'], default='debug')
    parser.add_argument('--serial', help='ADB device serial, if several devices are connected')
    args = parser.parse_args()
    args.abis = ','.join(dict.fromkeys(abi.strip() for abi in args.abis.split(',')))
    if any(abi not in TARGETS for abi in args.abis.split(',')): parser.error('Supported ABIs: arm64-v8a, x86_64')
    if args.command == 'metadata':
        print(json.dumps(release_metadata(args.abis)))
        return
    sdk = sdk_path()
    if args.command == 'doctor':
        for tool in ['cargo', 'rustup', 'java', 'python' if os.name == 'nt' else 'python3', 'cmake', 'git'] + (['ninja'] if os.name == 'nt' else []): print(f'{tool}: {shutil.which(tool) or "missing"}')
        for resource in ['platforms/android-36', 'build-tools/36.1.0', f'ndk/{NDK_VERSION}', 'platform-tools']: print(f'{resource}: {"ready" if (sdk / resource).exists() else "missing"}')
        run('rustup', 'target', 'list', '--installed')
    elif args.command == 'setup':
        manager = sdk / 'cmdline-tools/latest/bin' / ('sdkmanager.bat' if os.name == 'nt' else 'sdkmanager')
        if not manager.exists(): raise SystemExit('Install Android SDK command-line tools in Android Studio first.')
        run(manager, '--sdk_root=' + str(sdk), 'platforms;android-36', 'build-tools;36.1.0', 'platform-tools', 'ndk;' + NDK_VERSION)
        run('rustup', 'target', 'add', *[TARGETS[abi][0] for abi in args.abis.split(',')])
        run('git', 'submodule', 'update', '--init', 'XR-Corpus')
    elif args.command == 'native': native(sdk, args)
    else:
        wrapper = ROOT / 'android' / ('gradlew.bat' if os.name == 'nt' else 'gradlew')
        run(wrapper, ':app:assemble' + args.profile.capitalize(), '-PandroidAbis=' + args.abis, '-PpythonCommand=' + sys.executable, env=java_environment(sdk), cwd=ROOT / 'android')
        output = ROOT / 'android/app/build/outputs/apk' / args.profile
        metadata = json.loads((output / 'output-metadata.json').read_text())
        artifact = output / metadata['elements'][0]['outputFile']
        signed = args.profile == 'debug' or bool(os.environ.get('XRT_ANDROID_KEYSTORE'))
        suffix = '-debug' if args.profile == 'debug' else '' if signed else '-unsigned'
        destination = ROOT / 'dist' / (release_metadata(args.abis)['name'] + suffix + '.apk')
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(artifact, destination)
        print(f'APK: {destination}')
        if not signed: print('Unsigned APK: configure XRT_ANDROID_KEYSTORE and signing credentials before publishing.')
        if args.command == 'run':
            if args.profile != 'debug': raise SystemExit('Use a debug build for device installation, or configure release signing locally.')
            adb = sdk / 'platform-tools' / ('adb.exe' if os.name == 'nt' else 'adb')
            options = ['-s', args.serial] if args.serial else []
            run(adb, *options, 'install', '-r', artifact)
            run(adb, *options, 'shell', 'am', 'start', '-n', 'org.xrtranslate.app/.MainActivity')

if __name__ == '__main__': main()
