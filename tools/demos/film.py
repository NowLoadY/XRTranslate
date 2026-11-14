"""Shared screenplay, audio timing, and presenter data for real-time recordings."""

import itertools
import json
import subprocess
from pathlib import Path

import imageio_ffmpeg
import numpy as np

RATE = 48000
FFMPEG = imageio_ffmpeg.get_ffmpeg_exe()


def decode(path, start=0, duration=None):
    command = [FFMPEG, "-v", "error", "-ss", str(start), "-i", str(path)]
    if duration is not None:
        command += ["-t", str(duration)]
    command += ["-f", "f32le", "-ac", "2", "-ar", str(RATE), "pipe:1"]
    return (
        np.frombuffer(subprocess.check_output(command), dtype="<f4")
        .reshape(-1, 2)
        .copy()
    )


def load(path):
    path = Path(path).resolve()
    film = json.loads(path.read_text("utf8"))
    roots = {
        key: (path.parent / value).resolve()
        for key, value in film.get("paths", {}).items()
    }

    def expand(value):
        if isinstance(value, dict):
            return {key: expand(item) for key, item in value.items()}
        if isinstance(value, list):
            return [expand(item) for item in value]
        if isinstance(value, str) and value.startswith("$"):
            root, relative = value[1:].split("/", 1)
            return str(roots[root] / relative)
        if isinstance(value, str) and value.startswith("@"):
            return film["targets"][value[1:]]
        return value

    film = expand(film)
    ids = set()
    for scene in film["scenes"]:
        if scene["id"] in ids:
            raise ValueError(f"Duplicate scene: {scene['id']}")
        ids.add(scene["id"])
        scene["audio"] = [
            {**film["voices"][cue["voice"]], **cue} for cue in scene.get("audio", [])
        ]
        previous_end = 0
        for cue in scene["audio"]:
            cue["duration"] = len(decode(cue["file"])) / RATE
            start = cue.get("at", 0.3)
            if start < previous_end or start + cue["duration"] > scene["duration"]:
                raise ValueError(
                    f"Speech overlaps or exceeds scene {scene['id']}: {cue['voice']}"
                )
            previous_end = start + cue["duration"]
        actions = scene.get("actions", [])
        if any(a["at"] >= scene["duration"] for a in actions):
            raise ValueError(f"Action after scene end: {scene['id']}")
        if any(a["at"] > b["at"] for a, b in itertools.pairwise(actions)):
            raise ValueError(f"Actions out of order: {scene['id']}")
    return film


def presenter(scene):
    """Drive the existing mouth rig from the exact audio used in the final mix."""
    config = scene.get("presenter")
    if not config:
        return None
    count = int(np.ceil(scene["duration"] * 30))
    levels = np.zeros(count, dtype=np.float32)
    for cue in scene["audio"]:
        if cue.get("presenter", True) is False:
            continue
        samples = decode(cue["file"]).mean(axis=1)
        blocks = int(np.ceil(len(samples) / (RATE // 30)))
        padded = np.pad(samples, (0, blocks * (RATE // 30) - len(samples)))
        rms = np.sqrt(np.mean(padded.reshape(blocks, -1) ** 2, axis=1))
        # A small noise floor leaves breaths and pauses closed; no random flapping.
        envelope = np.clip(
            (rms - 0.008) / max(float(np.percentile(rms, 88)) - 0.008, 0.02), 0, 1
        )
        at = round(cue.get("at", 0.3) * 30)
        end = min(count, at + blocks)
        levels[at:end] = np.maximum(levels[at:end], envelope[: end - at])
    return {
        **config,
        "duration": scene["duration"] + 0.15,
        "speech": np.round(levels, 3).tolist(),
    }


def cue_time(cue, events):
    if cue.get("anchor"):
        return next(
            e["at"]
            for e in events
            if e["kind"] == "action" and e["action"].get("tag") == cue["anchor"]
        )
    return cue.get("at", 0.3)
