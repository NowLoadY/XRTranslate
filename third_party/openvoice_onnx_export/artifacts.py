"""Download, extract, and describe model build artifacts."""

from __future__ import annotations

import shutil
import subprocess
import zipfile
import zlib
from pathlib import Path


def download_file(repo_root: Path, url: str, expected_bytes: int, target: Path) -> Path:
    """Use the application's resumable downloader for fixed-source build inputs."""
    subprocess.run(
        [
            "cargo",
            "run",
            "--quiet",
            "--locked",
            "--manifest-path",
            str(repo_root / "Cargo.toml"),
            "-p",
            "xrtranslate-download",
            "--example",
            "fetch",
            "--",
            url,
            str(expected_bytes),
            str(target.resolve()),
        ],
        check=True,
        cwd=repo_root,
    )
    return target


def extract_ngc_member(archive: Path, suffix: str, output: Path) -> None:
    try:
        with zipfile.ZipFile(archive) as package:
            matches = [name for name in package.namelist() if name.endswith(suffix)]
            if len(matches) != 1:
                raise zipfile.BadZipFile(
                    f"Expected one NGC member ending in {suffix!r}, got {matches}"
                )
            output.parent.mkdir(parents=True, exist_ok=True)
            with package.open(matches[0]) as source, output.open("wb") as target:
                shutil.copyfileobj(source, target)
    except (zipfile.BadZipFile, zlib.error, EOFError):
        archive.unlink(missing_ok=True)
        raise


def file_record(path: Path, root: Path) -> dict[str, object]:
    return {"path": path.relative_to(root).as_posix(), "bytes": path.stat().st_size}
