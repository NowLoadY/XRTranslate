"""Render a screenplay: reusable visual blocks, ordered clips, and placed sound tracks."""

import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
import os
from pathlib import Path
import shutil

from .compose import assemble
from .media import sample, scan, write_json
from .title import render_title


def expand(value, version):
    if isinstance(value, str):
        return value.format_map(version)
    if isinstance(value, list):
        return [expand(item, version) for item in value]
    if isinstance(value, dict):
        return {key: expand(item, version) for key, item in value.items()}
    return value


class Story:
    def __init__(self, file, ffmpeg):
        self.root = file.parent
        self.script = json.loads(file.read_text("utf-8-sig"))
        self.ffmpeg = ffmpeg
        self.work = (self.root / self.script.get("work", "out/story")).resolve()
        self.work.mkdir(parents=True, exist_ok=True)
        self.rendered, self.probes = {}, {}

    def probe(self, file):
        if file not in self.probes:
            self.probes[file] = scan(self.ffmpeg, file)
        return self.probes[file]

    def picture(self, reference, assets, shots, target, overlap):
        if "shot" in reference:
            source = next(s for s in shots if s["id"] == reference["shot"])
            at = (
                overlap
                if reference.get("at") == "overlap"
                else (
                    source["duration"] - 1 / self.script["fps"]
                    if reference.get("at") == "end"
                    else reference.get("at", 0)
                )
            )
            at += source["in"]
            file = source["file"]
        else:
            file = assets[reference["asset"]]["file"]
            at = reference.get("at", 0)
        if reference.get("motion", False):
            return {"file": file, "at": at, "motion": True}
        sample(self.ffmpeg, file, at, target)
        return {"file": str(target)}

    def render(self, parameters):
        key = hashlib.sha256(
            json.dumps(parameters, sort_keys=True).encode()
        ).hexdigest()[:12]
        if key not in self.rendered:
            directory = self.work / ("title-" + key)
            directory.mkdir(parents=True, exist_ok=True)
            config = directory / "parameters.json"
            write_json(config, parameters)
            render_title(
                self.ffmpeg,
                parameters,
                self.script["size"],
                self.script["fps"],
                directory,
            )
            info = json.loads((directory / "render.json").read_text("utf-8"))
            self.rendered[key] = dict(
                info, file=str(directory / info["file"]), audio=False
            )
        return self.rendered[key]

    def prepare(self, name):
        version = dict(self.script["versions"][name], id=name)
        project = expand(
            {k: v for k, v in self.script.items() if k != "versions"}, version
        )
        project["work"] = str(self.work)
        assets = project["assets"]
        for asset in assets.values():
            for field in ("file", "captions", "chapters"):
                if field in asset:
                    asset[field] = str((self.root / asset[field]).resolve())
        shots, cursor = [], 0
        for definition in project["timeline"]:
            if any(s["id"] == definition["id"] for s in shots):
                raise ValueError(
                    f"Each timeline block needs a unique id: {definition['id']}"
                )
            kind = definition["kind"]
            if kind == "video":
                asset = assets[definition["asset"]]
                info = self.probe(asset["file"])
                start = round(definition.get("in", 0) * project["fps"]) / project["fps"]
                end = definition.get("out", info["frames"] / info["fps"])
                frames = round((end - start) * project["fps"])
                if (
                    start < 0
                    or frames <= 0
                    or end > info["frames"] / info["fps"] + 0.001
                ):
                    raise ValueError(f"Invalid source range: {definition['id']}")
                shot = dict(
                    asset,
                    frames=frames,
                    audio=info["audio"] and not definition.get("muted", False),
                    **{"in": start},
                )
            elif kind == "title":
                print(f"{name}: rendering {definition['id']}", flush=True)
                overlap = round(definition.get("overlap", 0) * project["fps"])
                if overlap and (
                    not shots or overlap >= shots[-1]["frames"] or shots[-1]["audio"]
                ):
                    raise ValueError(
                        "A title can overlap a preceding silent visual clip; keep speech on a sound track."
                    )
                folder = self.work / name / definition["id"]
                folder.mkdir(parents=True, exist_ok=True)
                parameters = dict(
                    definition["params"],
                    title=definition["text"],
                    subtitle=definition.get("subtitle", ""),
                )
                for side in ("from", "to"):
                    at = (
                        shots[-1]["duration"] - overlap / project["fps"]
                        if overlap
                        else 0
                    )
                    parameters[side] = self.picture(
                        definition[side], assets, shots, folder / f"{side}.png", at
                    )
                parameters["duration"] = sum(parameters["phases"])
                rendered = self.render(parameters)
                if overlap:
                    shots[-1]["frames"] -= overlap
                    shots[-1]["duration"] = shots[-1]["frames"] / project["fps"]
                    cursor -= overlap
                shot = {
                    "file": rendered["file"],
                    "frames": rendered["frames"],
                    "audio": False,
                    "in": 0,
                }
            else:
                raise ValueError(f"Unknown visual block: {kind}")
            shot.update(
                id=definition["id"],
                start=cursor / project["fps"],
                duration=shot["frames"] / project["fps"],
            )
            for field in ("chapter", "opening_hold", "gain_db"):
                if field in definition:
                    shot[field] = definition[field]
            shots.append(shot)
            cursor += shot["frames"]
        sounds = [
            dict(sound, file=assets[sound["asset"]]["file"])
            for sound in project.get("audio", [])
        ]
        output = (self.root / project["output"] / name).resolve()
        write_json(
            self.work / f"{name}-timeline.json", {"shots": shots, "audio": sounds}
        )
        return shots, sounds, project, version, output


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("script", type=Path, help="Local screenplay JSON")
    parser.add_argument(
        "--only", nargs="+", help="Render selected versions, for example --only zh en"
    )
    parser.add_argument(
        "--preview",
        action="store_true",
        help="Render visual blocks and resolve timing without encoding full videos",
    )
    parser.add_argument("--ffmpeg", default=os.environ.get("FFMPEG"))
    args = parser.parse_args()
    ffmpeg = args.ffmpeg or shutil.which("ffmpeg")
    if not ffmpeg:
        try:
            from imageio_ffmpeg import get_ffmpeg_exe

            ffmpeg = get_ffmpeg_exe()
        except ImportError:
            parser.error("Set FFMPEG or pass --ffmpeg with the executable path.")
    story = Story(args.script.resolve(), ffmpeg)
    versions = args.only or list(story.script["versions"])
    with ThreadPoolExecutor(max_workers=2) as pool:
        jobs = []
        for name in versions:
            prepared = story.prepare(name)
            if args.preview:
                print(f"{name}: visual blocks and timing in {story.work}", flush=True)
            else:
                jobs.append(pool.submit(assemble, ffmpeg, *prepared))
        for job in jobs:
            job.result()


if __name__ == "__main__":
    main()
