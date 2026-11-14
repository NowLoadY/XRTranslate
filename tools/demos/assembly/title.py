"""A moving title panel composed over exported stills or video frames."""

from contextlib import ExitStack
import math
import os
from pathlib import Path
import subprocess
import tempfile

import numpy as np
from PIL import Image, ImageColor, ImageDraw, ImageFont

from .media import write_json


def smooth(value):
    value = min(1.0, max(0.0, value))
    return value**3 * (value * (value * 6 - 15) + 10)


class Frames:
    def __init__(self, ffmpeg, source, size, fps):
        self.size, self.process, self.last = size, None, None
        if source.get("motion"):
            self.process = subprocess.Popen(
                [
                    str(ffmpeg),
                    "-v",
                    "error",
                    "-ss",
                    str(source.get("at", 0)),
                    "-i",
                    source["file"],
                    "-an",
                    "-vf",
                    f"fps={fps},scale={size[0]}:{size[1]}",
                    "-f",
                    "rawvideo",
                    "-pix_fmt",
                    "rgb24",
                    "-",
                ],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
            )
        else:
            self.last = np.array(
                Image.open(source["file"])
                .convert("RGB")
                .resize(tuple(size), Image.Resampling.LANCZOS)
            )

    def next(self):
        if self.process:
            raw = self.process.stdout.read(self.size[0] * self.size[1] * 3)
            if raw:
                self.last = np.frombuffer(raw, np.uint8).reshape(
                    self.size[1], self.size[0], 3
                )
            else:
                error = self.process.stderr.read().decode("utf-8", "replace")
                if self.process.wait() or self.last is None:
                    raise RuntimeError(
                        error or "The selected video range has no frames."
                    )
                self.close()
        return self.last

    def close(self):
        if self.process:
            if self.process.poll() is None:
                self.process.terminate()
            self.process.wait()
            self.process.stdout.close()
            self.process.stderr.close()
            self.process = None


def font_file(settings, bold):
    if settings.get("fontPath"):
        return settings["fontPath"]
    family = settings.get("fontFamily", "Segoe UI").split(",")[0].strip(" '\"")
    fonts = {
        "Microsoft YaHei": ("msyh.ttc", "msyhbd.ttc"),
        "Yu Gothic": ("YuGothM.ttc", "YuGothB.ttc"),
        "Malgun Gothic": ("malgun.ttf", "malgunbd.ttf"),
        "Segoe UI": ("segoeui.ttf", "seguisb.ttf"),
    }
    return str(
        Path(os.environ.get("WINDIR", "C:/Windows"))
        / "Fonts"
        / fonts[family][int(bold)]
    )


def text_layer(p, size):
    w, h = size
    safe = 1 - p["panelFraction"] + abs(p["slant"]) / 2 + 0.055
    width, scale = (0.95 - safe) * w, h / 1080
    lines = p["title"].split("\n")
    font_size = p["fontSize"] * scale
    while True:
        font = ImageFont.truetype(font_file(p, True), round(font_size * 2))
        if max(font.getlength(line) / 2 for line in lines) <= width:
            break
        font_size *= 0.96
    layer = Image.new("RGBA", (w * 2, h * 2))
    draw = ImageDraw.Draw(layer)
    subtitle_size, gap = 28 * scale, 24 * scale
    center = h / 2 - ((subtitle_size + gap) / 2 if p.get("subtitle") else 0)
    for i, line in enumerate(lines):
        y = center + (i - (len(lines) - 1) / 2) * font_size * 1.4
        draw.text(
            (safe * w * 2, y * 2), line, font=font, anchor="lm", fill=p["foreground"]
        )
    if p.get("subtitle"):
        y = (
            center
            + (len(lines) - 1) / 2 * font_size * 1.4
            + font_size / 2
            + gap
            + subtitle_size / 2
        )
        font = ImageFont.truetype(font_file(p, False), round(subtitle_size * 2))
        draw.text(
            (safe * w * 2, y * 2),
            p["subtitle"],
            font=font,
            anchor="lm",
            fill=p.get("subtitleForeground", "#647486"),
        )
    return layer.resize((w, h), Image.Resampling.LANCZOS)


def render_title(ffmpeg, p, size, fps, output):
    w, h = size
    count = round(p["duration"] * fps)
    enter, hold, leave = p["phases"]
    middle, span = 1 - p["panelFraction"], 1 + abs(p["slant"]) + 0.02
    start, end = 1 + abs(p["slant"]) / 2 + 0.01, -span - abs(p["slant"]) / 2 - 0.01
    x = np.arange(w, dtype=np.float32)[None, :] / w
    y = np.arange(h, dtype=np.float32)[:, None] / h - 0.5
    color = np.array(ImageColor.getrgb(p["background"]), dtype=np.float32)
    labels = text_layer(p, size)
    bounds = labels.getbbox()
    if bounds:
        bounds = (
            max(0, bounds[0] - 3),
            max(0, bounds[1] - 3),
            min(w, bounds[2] + 3),
            min(h, bounds[3] + 3),
        )
        labels = labels.crop(bounds)
    outgoing, incoming = Frames(ffmpeg, p["from"], size, fps), Frames(
        ffmpeg, p["to"], size, fps
    )
    file = output / "title-card.mp4"
    with ExitStack() as resources:
        resources.callback(outgoing.close)
        resources.callback(incoming.close)
        errors = resources.enter_context(tempfile.TemporaryFile())
        encoder = subprocess.Popen(
            [
                str(ffmpeg),
                "-v",
                "error",
                "-y",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgb24",
                "-s",
                f"{w}x{h}",
                "-r",
                str(fps),
                "-i",
                "-",
                "-an",
                "-c:v",
                "libx264",
                "-preset",
                "fast",
                "-crf",
                "16",
                "-threads",
                "4",
                "-pix_fmt",
                "yuv420p",
                "-movflags",
                "+faststart",
                str(file),
            ],
            stdin=subprocess.PIPE,
            stderr=errors,
            creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
        )
        try:
            for index in range(count):
                t = p["duration"] if index == count - 1 else index / fps
                entry = smooth(t / enter)
                exit_progress = smooth((t - enter - hold) / leave)
                position = (
                    start + (middle - start) * entry
                    if t < enter
                    else middle + (end - middle) * exit_progress
                )
                envelope = smooth(t / 0.7) * smooth((p["duration"] - t) / 0.65)
                phase = t * math.tau * p.get("swayFrequency", 0.48)
                drift = p.get("swayPixels", 12) * h / 1080 * math.sin(phase) * envelope
                slant = (
                    p["slant"]
                    + p.get("swaySlant", 0.018)
                    * math.sin(phase * 0.73 + 0.4)
                    * envelope
                )
                position += drift / w
                q = x + slant * y
                leading = np.clip((q - position) * w / 1.5 + 0.5, 0, 1)
                trailing = np.clip((q - position - span) * w / 1.5 + 0.5, 0, 1)
                panel = leading - trailing
                original = Image.fromarray(outgoing.next()).transform(
                    (w, h),
                    Image.Transform.AFFINE,
                    (1, 0, p.get("outgoingShift", 0.23) * w * entry, 0, 1, 0),
                    Image.Resampling.BICUBIC,
                    fillcolor=p["outgoingBackground"],
                )
                frame = (
                    np.asarray(original, np.float32) * (1 - leading[..., None])
                    + color * panel[..., None]
                    + incoming.next() * trailing[..., None]
                )
                if bounds:
                    exact_x = bounds[0] + (position - middle) * w
                    left = math.floor(exact_x)
                    x0, x1 = max(0, left), min(w, left + labels.width + 2)
                    if x1 > x0:
                        text = np.asarray(
                            labels.transform(
                                (labels.width + 2, labels.height),
                                Image.Transform.AFFINE,
                                (1, 0, left - exact_x, 0, 1, 0),
                                Image.Resampling.BICUBIC,
                            ),
                            np.float32,
                        )[:, x0 - left : x1 - left]
                        area = frame[bounds[1] : bounds[3], x0:x1]
                        alpha = (
                            text[..., 3:4]
                            / 255
                            * panel[bounds[1] : bounds[3], x0:x1, None]
                        )
                        area[:] = area * (1 - alpha) + text[..., :3] * alpha
                frame = np.rint(np.clip(frame, 0, 255)).astype(np.uint8)
                if index == min(
                    count - 1, round((enter + max(hold - 0.1, hold / 2)) * fps)
                ):
                    Image.fromarray(frame).save(output / "poster.png")
                encoder.stdin.write(frame.tobytes())
            encoder.stdin.close()
            if encoder.wait():
                errors.seek(0)
                raise RuntimeError(errors.read().decode("utf-8", "replace"))
        finally:
            if encoder.poll() is None:
                encoder.kill()
                encoder.wait()
    write_json(
        output / "render.json",
        {
            "file": file.name,
            "frames": count,
            "fps": fps,
            "width": w,
            "height": h,
            "duration": count / fps,
            "parameters": p,
        },
    )
