"""Stage and publish immutable XRTranslate OpenVoice ONNX packages."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
from contextlib import contextmanager
from pathlib import Path
from typing import Iterator

import requests

# HF Xet uploads can indefinitely stall behind otherwise healthy HTTP proxies.
# The deterministic publisher intentionally uses the ordinary single-file LFS
# path so each completed transfer has one visible commit and verification step.
os.environ.setdefault("HF_HUB_DISABLE_XET", "1")

from huggingface_hub import HfApi, get_token, hf_hub_url
from huggingface_hub.utils import build_hf_headers


REPOSITORY = "NowLoadY/XRTranslate-OpenVoice-ONNX"


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def _validate_release_root(build_root: Path, release_root: Path) -> None:
    protected = {
        Path.cwd().resolve(),
        Path.home().resolve(),
        Path(release_root.anchor).resolve(),
    }
    if (
        release_root in protected
        or release_root == build_root
        or release_root in build_root.parents
    ):
        raise RuntimeError(f"Refusing unsafe release root: {release_root}")


@contextmanager
def publication_lock(release_root: Path) -> Iterator[None]:
    """Prevent two local publishers from changing one release or Hub branch."""
    release_root.parent.mkdir(parents=True, exist_ok=True)
    lock_path = release_root.parent / ".xrtranslate-openvoice-hf.publish.lock"
    stream = lock_path.open("a+b")
    stream.seek(0, os.SEEK_END)
    if stream.tell() == 0:
        stream.write(b"0")
        stream.flush()
    stream.seek(0)
    try:
        if os.name == "nt":
            import msvcrt

            msvcrt.locking(stream.fileno(), msvcrt.LK_NBLCK, 1)
        else:
            import fcntl

            fcntl.flock(stream.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError as error:
        stream.close()
        raise RuntimeError(
            f"Another publisher already owns {lock_path}; refusing concurrent upload"
        ) from error
    try:
        yield
    finally:
        stream.seek(0)
        if os.name == "nt":
            import msvcrt

            msvcrt.locking(stream.fileno(), msvcrt.LK_UNLCK, 1)
        else:
            import fcntl

            fcntl.flock(stream.fileno(), fcntl.LOCK_UN)
        stream.close()


def stage(build_root: Path, release_root: Path, languages: list[str]) -> None:
    """Build a clean release tree from one or more already-verified packages."""
    _validate_release_root(build_root, release_root)
    if release_root.exists():
        shutil.rmtree(release_root)
    release_root.mkdir(parents=True)

    package_manifests = []
    for language in languages:
        source = build_root / "packages" / language / "v1"
        manifest_path = source / "package-manifest.json"
        if not manifest_path.is_file():
            raise RuntimeError(f"Build output is incomplete: {source}")
        destination = release_root / "packages" / language / "v1"
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copytree(source, destination)
        package_manifests.append(json.loads(manifest_path.read_text(encoding="utf-8")))

    packages = []
    for manifest in package_manifests:
        language = manifest["language"]["key"]
        packages.append(
            {
                "id": manifest["package_id"],
                "language": language,
                "path": f"packages/{language}/v1",
                "installed_bytes": sum(
                    path.stat().st_size
                    for path in (release_root / "packages" / language / "v1").rglob("*")
                    if path.is_file()
                ),
            }
        )

    language_yaml = "\n".join(f"- {language}" for language in languages)
    base_models = "\n".join(
        f"- {manifest['language']['melo_repository']}" for manifest in package_manifests
    )
    rows = []
    for manifest, package in zip(package_manifests, packages, strict=True):
        installed_mib = package["installed_bytes"] / 1024 / 1024
        language = manifest["language"]
        rows.append(
            f"| `{manifest['package_id']}` | {language['label']} | FP16 | "
            f"{installed_mib:.1f} MiB | {manifest['graph_contract']['sample_rate_hz']:,} Hz |"
        )
    package_rows = "\n".join(rows)
    readme = f"""---
license: mit
language:
{language_yaml}
pipeline_tag: text-to-speech
library_name: onnxruntime
tags:
- openvoice
- melotts
- onnx
- xrtranslate
base_model:
{base_models}
---

# XRTranslate OpenVoice ONNX

Community-maintained, reproducible ONNX conversions of official MyShell MeloTTS
language checkpoints for XRTranslate's pure-Rust OpenVoice V2 provider. These
are not official MyShell or NVIDIA artifacts.

## Packages

| Package | Language | Precision | Installed size | Base output |
| --- | --- | --- | ---: | ---: |
{package_rows}

Each package is self-contained: its language-specific frontend and feature
graph, MeloTTS acoustic graph, OpenVoice V2 converter, reference speaker
encoder, source tone embedding, licenses, and a machine-readable per-file
SHA-256 manifest. XRTranslate returns converted audio at 22,050 Hz mono PCM16.

## Reproducibility

The production graphs are exported in the existing `torch211cu128` Conda
environment with Python 3.10, PyTorch `2.11.0+cu128`, CUDA 12.8, ONNX 1.22.0,
ONNX Runtime 1.23.2, and opset 16. PyTorch is not upgraded or downgraded.

```powershell
conda run -n torch211cu128 python third_party/openvoice_onnx_export/build.py `
  --language <language-key> `
  --output runtime/.temp/openvoice-onnx-export
```

Exact upstream commits, source hashes, graph tensor contracts, conversion
versions, smoke-test results, and produced file hashes are recorded in each
`packages/<language-key>/v1/package-manifest.json`.

## Upstream and licenses

- [MeloTTS](https://github.com/myshell-ai/MeloTTS) and
  [OpenVoice](https://github.com/myshell-ai/OpenVoice) are MIT licensed.
- Each package manifest pins its exact MeloTTS checkpoint repository and commit.
- Source tone embeddings come from
  [`myshell-ai/OpenVoiceV2`](https://huggingface.co/myshell-ai/OpenVoiceV2).
- Language-specific BERT/tokenizer, G2P, dictionary, and normalization notices
  are preserved inside that package.

See each package's `licenses/` directory before redistribution.

## Limitations

The packages require a compatible CUDA ONNX Runtime in XRTranslate. They are
not standalone Python TTS APIs. Voice cloning transfers tone color;
pronunciation, accent, and prosody remain controlled by the selected MeloTTS
language pack.
"""
    (release_root / "README.md").write_text(readme, encoding="utf-8")
    files = [
        {
            "path": path.relative_to(release_root).as_posix(),
            "bytes": path.stat().st_size,
            "sha256": sha256(path),
        }
        for path in sorted(release_root.rglob("*"))
        if path.is_file()
        and ".cache" not in path.parts
        and path.name != "release-manifest.json"
    ]
    release_manifest = {"schema_version": 1, "packages": packages, "files": files}
    (release_root / "release-manifest.json").write_text(
        json.dumps(release_manifest, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )


def _release_files(release_root: Path) -> list[Path]:
    return [
        path
        for path in sorted(release_root.rglob("*"))
        if path.is_file() and ".cache" not in path.parts
    ]


def _state_path(release_root: Path) -> Path:
    return release_root.parent / f".{release_root.name}.publish-state.json"


def _load_state(release_root: Path) -> dict:
    path = _state_path(release_root)
    if not path.is_file():
        return {"schema_version": 1, "repository": REPOSITORY, "files": {}}
    state = json.loads(path.read_text(encoding="utf-8"))
    if state.get("schema_version") != 1 or state.get("repository") != REPOSITORY:
        raise RuntimeError(f"Unexpected publication state: {path}")
    return state


def _save_state(release_root: Path, state: dict) -> None:
    path = _state_path(release_root)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(
        json.dumps(state, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    os.replace(temporary, path)


def _verify_hub_file(
    api: HfApi,
    token: str,
    relative_path: str,
    expected_bytes: int,
    expected_sha256: str,
    revision: str,
) -> bool:
    entries = api.get_paths_info(
        REPOSITORY,
        [relative_path],
        expand=True,
        revision=revision,
        repo_type="model",
    )
    if len(entries) != 1 or getattr(entries[0], "path", None) != relative_path:
        return False
    remote = entries[0]
    if remote.size != expected_bytes:
        return False

    lfs = getattr(remote, "lfs", None)
    if lfs is not None:
        return lfs.sha256.casefold() == expected_sha256.casefold()

    with requests.get(
        hf_hub_url(REPOSITORY, relative_path, revision=revision),
        headers=build_hf_headers(token=token),
        stream=True,
        timeout=(15, 120),
    ) as response:
        response.raise_for_status()
        digest = hashlib.sha256()
        received = 0
        for block in response.iter_content(chunk_size=4 * 1024 * 1024):
            if block:
                digest.update(block)
                received += len(block)
    return received == expected_bytes and digest.hexdigest() == expected_sha256


def upload(release_root: Path, *, allow_public_update: bool = False) -> str:
    token = get_token()
    if not token:
        raise RuntimeError("Run Hugging Face interactive login before publishing.")
    api = HfApi(token=token)
    identity = api.whoami()["name"]
    if identity.casefold() != "nowloady":
        raise RuntimeError(f"Refusing to publish as unexpected account {identity!r}")
    api.create_repo(REPOSITORY, repo_type="model", private=True, exist_ok=True)
    repository = api.model_info(REPOSITORY)
    initially_private = repository.private
    if not initially_private and not allow_public_update:
        raise RuntimeError(
            "Refusing to update a public repository without --allow-public-update. "
            "Published catalogue revisions remain immutable, but the moving Hub HEAD "
            "will expose the new staged release while it is uploaded."
        )

    files = _release_files(release_root)
    state = _load_state(release_root)
    head = repository.sha
    for index, path in enumerate(files, start=1):
        relative_path = path.relative_to(release_root).as_posix()
        expected_bytes = path.stat().st_size
        expected_sha256 = sha256(path)
        print(
            f"[{index}/{len(files)}] {relative_path} ({expected_bytes} bytes)",
            flush=True,
        )

        current = api.model_info(REPOSITORY).sha
        if current != head:
            raise RuntimeError(
                f"Hub HEAD changed concurrently: expected {head}, found {current}"
            )
        if _verify_hub_file(
            api, token, relative_path, expected_bytes, expected_sha256, head
        ):
            print(f"[{index}/{len(files)}] already verified at {head}", flush=True)
        else:
            commit = api.upload_file(
                path_or_fileobj=path,
                path_in_repo=relative_path,
                repo_id=REPOSITORY,
                repo_type="model",
                token=token,
                commit_message=f"Publish {relative_path}",
                parent_commit=head,
            )
            head = commit.oid
            if not _verify_hub_file(
                api, token, relative_path, expected_bytes, expected_sha256, head
            ):
                raise RuntimeError(
                    f"Hub verification failed for {relative_path} at {head}"
                )
            print(f"[{index}/{len(files)}] uploaded and verified at {head}", flush=True)

        state["files"][relative_path] = {
            "bytes": expected_bytes,
            "sha256": expected_sha256,
            "revision": head,
        }
        state["head"] = head
        _save_state(release_root, state)

    final = api.model_info(REPOSITORY, files_metadata=True)
    if final.private != initially_private or final.sha != head:
        raise RuntimeError("Repository visibility or HEAD changed after publication")
    print(f"Published {len(files)} files at immutable revision {head}", flush=True)
    return head


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--build-root", type=Path, required=True)
    parser.add_argument("--release-root", type=Path, required=True)
    parser.add_argument("--language", action="append", dest="languages")
    parser.add_argument("--workers", type=int, default=1)
    parser.add_argument("--upload", action="store_true")
    parser.add_argument(
        "--allow-public-update",
        action="store_true",
        help=(
            "Allow appending a staged release while the repository is already public. "
            "Existing catalogue revisions remain immutable."
        ),
    )
    args = parser.parse_args()
    if args.workers < 1:
        raise RuntimeError("--workers must be at least one")
    if args.workers != 1:
        raise RuntimeError("Deterministic publication requires --workers 1")
    languages = args.languages or ["zh"]
    if len(set(languages)) != len(languages):
        raise RuntimeError("Each release language may be selected only once")
    release_root = args.release_root.resolve()
    build_root = args.build_root.resolve()
    with publication_lock(release_root):
        stage(build_root, release_root, languages)
        if args.upload:
            upload(release_root, allow_public_update=args.allow_public_update)


if __name__ == "__main__":
    main()
