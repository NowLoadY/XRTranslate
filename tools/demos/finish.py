"""Finish real-time takes with restrained reframing and the recorded voice cues."""

import argparse
import json
import subprocess
import wave
from collections import deque
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import cv2
import numpy as np
from film import FFMPEG, RATE, cue_time, decode, load
from PIL import Image
from pointer import CursorArt, Pointer

ART = CursorArt()


def stamp(t):
    ms = round(t * 1000)
    h, ms = divmod(ms, 3600000)
    m, ms = divmod(ms, 60000)
    s, ms = divmod(ms, 1000)
    return f"{h:02}:{m:02}:{s:02},{ms:03}"


def audio(script, takes, destination, session):
    scenes, fps = script["scenes"], script["fps"]
    total = sum(round(s["duration"] * fps) for s in scenes) / fps
    mix = np.zeros((round(total * RATE), 2), np.float32)
    captions, chapters, cues = [], [], []
    offset = 0
    for scene in scenes:
        events = json.loads((takes / f"{scene['id']}.json").read_text("utf8"))["events"]
        chapters.append((offset, scene["chapter"]))
        spoken = []
        for item in scene.get("audio", []):
            at = cue_time(item, events)
            samples = decode(item["file"])
            duration = len(samples) / RATE
            assert at + duration <= scene["duration"] + 0.02, (
                scene["id"],
                at,
                duration,
            )
            rms = np.sqrt(np.mean(samples * samples))
            gain = min(
                0.105 / max(rms, 1e-6), 0.91 / max(float(abs(samples).max()), 1e-6)
            )
            samples *= gain
            start = round((offset + at) * RATE)
            mix[start : start + len(samples)] += samples
            captions.append((offset + at, offset + at + duration, item["text"]))
            spoken.append((at, at + duration))
            cues.append(
                {
                    "scene": scene["id"],
                    "start": offset + at,
                    "duration": duration,
                    "file": item["file"],
                }
            )
        for sound in scene.get("sound", []):
            at = cue_time({**sound, "at": sound.get("at", 0)}, events)
            position = sound.get("source_start", 0)
            if "media" in sound:
                store = session / "runtime/plugins/data/media/video_tasks.json"
                task = next(
                    t
                    for t in json.loads(store.read_text("utf8"))["tasks"]
                    if t["title"] == script["media_task"]
                )
                path = (
                    task["speech_track"]["path"]
                    if sound["media"] == "speech_track"
                    else task["separated_audio"][sound["media"]]
                )
                position = next(
                    e["position"] for e in events if e["kind"] == "media_position"
                )
            else:
                path = sound["file"]
            samples = decode(path, position, scene["duration"] - at)
            clock = np.arange(len(samples)) / RATE
            fade = sound.get("fade", 0.25)
            envelope = np.minimum(
                np.clip(clock / fade, 0, 1),
                np.clip((len(samples) / RATE - clock) / fade, 0, 1),
            )
            if "duck" in sound:
                for begin, end in spoken:
                    envelope *= np.interp(
                        clock + at,
                        [max(0, begin - 0.15), begin, end, end + 0.25],
                        [1, sound["duck"], sound["duck"], 1],
                    )
            start = round((offset + at) * RATE)
            mix[start : start + len(samples)] += (
                samples * (envelope * sound.get("gain", 1))[:, None]
            )
        offset += scene["duration"]
    peak = float(abs(mix).max())
    if peak > 0.95:
        mix *= 0.95 / peak
    with wave.open(str(destination / "narration.wav"), "wb") as out:
        out.setnchannels(2)
        out.setsampwidth(2)
        out.setframerate(RATE)
        out.writeframes((mix * 32767).astype("<i2").tobytes())
    (destination / "captions.srt").write_text(
        "\n\n".join(
            f"{i + 1}\n{stamp(a)} --> {stamp(b)}\n{text}"
            for i, (a, b, text) in enumerate(captions)
        )
        + "\n",
        encoding="utf8",
    )
    metadata = [";FFMETADATA1", "title=" + script.get("title", "XRTranslate 使用演示")]
    for i, (start, title) in enumerate(chapters):
        end = chapters[i + 1][0] if i + 1 < len(chapters) else total
        metadata += [
            "[CHAPTER]",
            "TIMEBASE=1/1000",
            f"START={round(start * 1000)}",
            f"END={round(end * 1000)}",
            f"title={title}",
        ]
    (destination / "chapters.ffmeta").write_text(
        "\n".join(metadata) + "\n", encoding="utf8"
    )
    (destination / "voice-cues.json").write_text(
        json.dumps(cues, ensure_ascii=False, indent=2), encoding="utf8"
    )


def crop(scene, t, viewport):
    keys = scene.get("camera", [[0, 1, 0.5, 0.5]])
    pose = keys[0][1:]
    for key in keys[1:]:
        if t < key[0]:
            break
        weight = min(1, (t - key[0]) / 0.36)
        weight = weight * weight * (3 - 2 * weight)
        pose = [a + (b - a) * weight for a, b in zip(pose, key[1:])]
        if weight < 1:
            break
    zoom, cx, cy = pose
    vw, vh, _ = viewport
    width, height = vw / zoom, vh / zoom
    left = max(0, min(vw - width, cx * vw - width / 2))
    top = max(0, min(vh - height, cy * vh - height / 2))
    return left, top, width, height


def render(frame, scene, t, pose, viewport):
    left, top, width, height = crop(scene, t, viewport)
    matrix = np.array(
        [
            [1920 / width, 0, -left * 1920 / width],
            [0, 1080 / height, -top * 1080 / height],
        ],
        np.float32,
    )
    frame = cv2.warpAffine(frame, matrix, (1920, 1080), flags=cv2.INTER_LINEAR)
    canvas = Image.fromarray(cv2.cvtColor(frame, cv2.COLOR_BGR2RGB))
    point, scale, opacity = pose
    ART.draw(
        canvas,
        (point, 0.78 + (scale - 1) * 0.35, opacity),
        (left, top, width, height),
        (0, 0, 1920, 1080),
        viewport[2],
    )
    return canvas, t


def finish(script, takes, destination, session):
    scenes, fps = script["scenes"], script["fps"]
    if session is None and any(
        "media" in sound for scene in scenes for sound in scene.get("sound", [])
    ):
        raise ValueError(
            "Media audio requires --session pointing to the demonstration app"
        )
    for scene in scenes:
        record = json.loads((takes / f"{scene['id']}.json").read_text("utf8"))
        if record["error"] or any(
            record["scene"].get(key) != scene.get(key)
            for key in ("duration", "audio", "actions", "presenter")
        ):
            raise ValueError(
                f"Re-record {scene['id']}: its performance no longer matches the screenplay"
            )
    audio(script, takes, destination, session)
    output = destination / script.get("filename", "XRTranslate-使用演示.mp4")
    duration = sum(s["duration"] for s in scenes)
    command = [
        FFMPEG,
        "-hide_banner",
        "-loglevel",
        "warning",
        "-y",
        "-f",
        "rawvideo",
        "-pixel_format",
        "rgb24",
        "-video_size",
        "1920x1080",
        "-framerate",
        str(fps),
        "-i",
        "pipe:0",
        "-i",
        str(destination / "narration.wav"),
        "-f",
        "ffmetadata",
        "-i",
        str(destination / "chapters.ffmeta"),
        "-i",
        str(destination / "captions.srt"),
        "-map",
        "0:v",
        "-map",
        "1:a",
        "-map",
        "3:s",
        "-map_metadata",
        "2",
        "-map_chapters",
        "2",
        "-vf",
        f"fade=t=in:st=0:d=0.12,fade=t=out:st={duration - 0.24:.3f}:d=0.24",
        "-c:v",
        "libx264",
        "-preset",
        "fast",
        "-crf",
        "17",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        "-b:a",
        "192k",
        "-af",
        "loudnorm=I=-16:TP=-1.5:LRA=7",
        "-ar",
        "48000",
        "-metadata:s:a:0",
        "language=" + script.get("subtitle_language", "zho"),
        "-c:s",
        "mov_text",
        "-metadata:s:s:0",
        "language=" + script.get("subtitle_language", "zho"),
        "-metadata:s:s:0",
        "title=" + script.get("subtitle_title", "讲解字幕"),
        "-disposition:s:0",
        "default",
        "-movflags",
        "+faststart",
        str(output),
    ]
    frames = 0
    with (destination / "encode.log").open("w", encoding="utf8") as log:
        encoder = subprocess.Popen(
            command,
            stdin=subprocess.PIPE,
            stderr=log,
            creationflags=subprocess.CREATE_NO_WINDOW,
        )
        try:
            with ThreadPoolExecutor(max_workers=4) as pool:
                for scene in scenes:
                    record = json.loads(
                        (takes / f"{scene['id']}.json").read_text("utf8")
                    )
                    assert record["error"] is None, record["error"]
                    events = iter(record["events"])
                    event = next(events, None)
                    pointer = Pointer()
                    pointer.duration = 0.22
                    reader = cv2.VideoCapture(str(takes / f"{scene['id']}.mp4"))
                    pending = deque()
                    saved = False

                    def write(result, scene=scene):
                        nonlocal saved
                        canvas, t = result
                        if not saved and t >= scene.get(
                            "snapshot", scene["duration"] - 0.8
                        ):
                            canvas.save(destination / f"{scene['id']}.jpg", quality=92)
                            saved = True
                        encoder.stdin.write(canvas.tobytes())

                    for i in range(round(scene["duration"] * fps)):
                        ok, frame = reader.read()
                        assert ok, (scene["id"], i)
                        t = i / fps
                        while event and event["at"] <= t:
                            if event["kind"] == "aim":
                                pointer.aim(event["point"], event["at"])
                            elif event["kind"] == "click":
                                pointer.click(event["at"])
                            elif event["kind"] == "drag":
                                pointer.origin = pointer.target = event["point"]
                                pointer.started = event["at"] - 0.22
                            event = next(events, None)
                        pending.append(
                            pool.submit(
                                render,
                                frame,
                                scene,
                                t,
                                pointer.pose(t),
                                script["viewport"],
                            )
                        )
                        if len(pending) >= 8:
                            write(pending.popleft().result())
                        frames += 1
                    while pending:
                        write(pending.popleft().result())
                    reader.release()
                    print(f"Finished {scene['id']}", flush=True)
            encoder.stdin.close()
            assert encoder.wait(timeout=60) == 0
        finally:
            if encoder.poll() is None:
                encoder.kill()
                encoder.wait()
    (destination / "verification.json").write_text(
        json.dumps(
            {
                "frames": frames,
                "fps": fps,
                "duration": frames / fps,
                "resolution": [1920, 1080],
                "takes": len(scenes),
            },
            indent=2,
        ),
        encoding="utf8",
    )
    print(output, flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("script", type=Path)
    parser.add_argument("--takes", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--session", type=Path)
    args = parser.parse_args()
    script = load(args.script)
    takes, destination = args.takes, args.output
    destination.mkdir(parents=True, exist_ok=True)
    finish(script, takes, destination, args.session)
