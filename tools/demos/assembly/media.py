"""FFmpeg and sidecar helpers shared by the screenplay assembler."""

import json
from pathlib import Path
import re
import subprocess


def run(command):
    result = subprocess.run(
        list(map(str, command)),
        capture_output=True,
        text=True,
        encoding="utf-8",
        creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
    )
    if result.returncode:
        raise RuntimeError(result.stderr[-5000:] or result.stdout[-5000:])
    return result


def scan(ffmpeg, file, decode=False):
    command = [ffmpeg, "-hide_banner", "-xerror", "-i", file, "-map", "0:v:0"]
    command += ["-map", "0:a?"] if decode else ["-c:v", "copy"]
    result = run(command + ["-progress", "pipe:1", "-nostats", "-f", "null", "-"])
    video = next(
        line
        for line in result.stderr.splitlines()
        if "Stream #0:" in line and "Video:" in line
    )
    size = re.search(r"\b(\d{2,5})x(\d{2,5})\b", video)
    rate = re.search(r"([\d.]+) fps", video)
    frames = int(re.findall(r"^frame=(\d+)", result.stdout, re.M)[-1])
    return {
        "frames": frames,
        "fps": float(rate[1]),
        "size": [int(size[1]), int(size[2])],
        "audio": any(
            "Stream #0:" in line and "Audio:" in line
            for line in result.stderr.splitlines()
        ),
        "streams": [
            line.strip() for line in result.stderr.splitlines() if "Stream #0:" in line
        ],
    }


def sample(ffmpeg, file, time, output):
    run(
        [
            ffmpeg,
            "-hide_banner",
            "-v",
            "error",
            "-y",
            "-ss",
            time,
            "-i",
            file,
            "-frames:v",
            "1",
            output,
        ]
    )


def seconds(value):
    h, m, s = value.replace(",", ".").split(":")
    return int(h) * 3600 + int(m) * 60 + float(s)


def stamp(value):
    minutes, milliseconds = divmod(round(value * 1000), 60000)
    hours, minutes = divmod(minutes, 60)
    seconds, milliseconds = divmod(milliseconds, 1000)
    return f"{hours:02}:{minutes:02}:{seconds:02},{milliseconds:03}"


def read_captions(file):
    text = Path(file).read_text("utf-8-sig").replace("\r\n", "\n")
    pattern = r"(\d\d:\d\d:\d\d,\d{3}) --> (\d\d:\d\d:\d\d,\d{3})\n(.*?)(?=\n\n|\Z)"
    return [
        (seconds(a), seconds(b), text.strip())
        for a, b, text in re.findall(pattern, text, re.S)
    ]


def read_chapters(file):
    result = []
    for block in Path(file).read_text("utf-8-sig").split("[CHAPTER]")[1:]:
        fields = dict(
            line.split("=", 1) for line in block.strip().splitlines() if "=" in line
        )
        a, b = map(int, fields["TIMEBASE"].split("/"))
        result.append(
            (int(fields["START"]) * a / b, int(fields["END"]) * a / b, fields["title"])
        )
    return result


def sidecars(shots, output, title):
    captions, chapters = [], []
    for shot in shots:
        start, end = shot["in"], shot["in"] + shot["duration"]
        offset = shot["start"] - start
        for key, reader, target in [
            ("captions", read_captions, captions),
            ("chapters", read_chapters, chapters),
        ]:
            if shot.get(key):
                for a, b, text in reader(shot[key]):
                    if b > start and a < end:
                        target.append(
                            (max(a, start) + offset, min(b, end) + offset, text)
                        )
        if shot.get("chapter"):
            chapters.append(
                (shot["start"], shot["start"] + shot["duration"], shot["chapter"])
            )
    captions.sort()
    subtitle = output / "captions.srt"
    subtitle.write_text(
        "\n\n".join(
            f"{i+1}\n{stamp(a)} --> {stamp(b)}\n{text}"
            for i, (a, b, text) in enumerate(captions)
        )
        + ("\n" if captions else ""),
        encoding="utf-8",
    )
    lines = [";FFMETADATA1", "title=" + title.replace("\n", " ")]
    markers = sorted({round(a * 1000): text for a, _, text in chapters}.items())
    total = round((shots[-1]["start"] + shots[-1]["duration"]) * 1000)
    for i, (start, text) in enumerate(markers):
        end = markers[i + 1][0] if i + 1 < len(markers) else total
        lines += [
            "[CHAPTER]",
            "TIMEBASE=1/1000",
            f"START={start}",
            f"END={end}",
            "title=" + text,
        ]
    metadata = output / "chapters.ffmeta"
    metadata.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return subtitle if captions else None, metadata


def write_json(file, value):
    Path(file).write_text(
        json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
