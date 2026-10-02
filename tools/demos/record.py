"""Record an external screenplay through UI Director; no desktop/audio capture.

Requires Pillow and imageio-ffmpeg. All product copy, navigation, interactions,
and camera keyframes live in the supplied JSON, not in the application.
"""
from __future__ import annotations

import argparse
import json
import os
import queue
import re
from pathlib import Path
import subprocess
import sys
import threading
import time

from PIL import Image, ImageDraw, ImageFilter, ImageFont
import imageio_ffmpeg

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from ui_director import UIDirector
from pointer import CursorArt, Pointer


def checked(response):
    if not response.get("success"):
        raise RuntimeError(response.get("message", "Director action failed"))
    return response


class Capture:
    """Bounded capture at exact video times, with explicit holds for reading real results."""
    def __init__(self, director, script):
        self.director, self.script = director, script
        self.error = None
        self.stopped = threading.Event()
        self.pending = queue.Queue(maxsize=2)
        self.frames, self.actions = 0, []
        self.thread = threading.Thread(target=self.run, daemon=True)
        self.thread.start()

    def run(self):
        try:
            fps, offset = self.script["fps"], 0
            with self.director.capture_frames() as frame_at:
                for index, scene in enumerate(self.script["scenes"]):
                    for action in scene.get("prepare", []):
                        result = act(self.director, action)
                        self.actions.append({"chapter": scene["chapter"], "time": offset / fps, "prepare": True, "action": action, "result": result})
                    checked(self.director.page(scene["page"]))
                    print(f"{index + 1}/{len(self.script['scenes'])}  {scene['chapter']}", flush=True)
                    action_index = 0
                    actions = scene.get("actions", [])
                    pointer, prepared, typed = Pointer(), False, None
                    hold, held_frame = False, None
                    last_pointer = None
                    for frame in range(round(scene["duration"] * fps)):
                        if self.stopped.is_set():
                            return
                        elapsed, absolute = frame / fps, (offset + frame) / fps
                        if action_index < len(actions) and not prepared and actions[action_index]["at"] - elapsed <= .45:
                            point = action_point(self.director, actions[action_index])
                            if point:
                                pointer.aim(point, elapsed)
                            prepared = True
                        pose = pointer.pose(elapsed)
                        if pose[0] and pose[0] != last_pointer:
                            checked(self.director.pointer(*pose[0]))
                            last_pointer = pose[0]
                        while action_index < len(actions) and actions[action_index]["at"] <= elapsed:
                            action = actions[action_index]
                            if action["do"] not in ("wait", "hover"):
                                hold, held_frame = False, None
                            result = act(self.director, action)
                            if action.get("hold"):
                                hold = True
                            if action["do"] == "type":
                                typed = [action["value"], elapsed, action.get("duration", 1.5), 0]
                            if action["do"] in ("click", "tap", "set", "type"):
                                pointer.click(elapsed)
                            self.actions.append({"chapter": scene["chapter"], "time": absolute, "action": action, "result": result})
                            action_index += 1
                            prepared = False
                        if typed:
                            text, since, duration, count = typed
                            length = min(len(text), int(len(text) * (elapsed - since) / duration) + 1)
                            if length > count:
                                checked(self.director.text(text[count:length]))
                                typed[3] = length
                            if length == len(text):
                                typed = None
                        if held_frame is None:
                            captured = frame_at(absolute)
                            self.frames += 1
                            if hold:
                                held_frame = captured
                        else:
                            captured = held_frame
                        while not self.stopped.is_set():
                            try:
                                self.pending.put((index, elapsed, absolute, captured, pointer.pose(elapsed)), timeout=0.2)
                                break
                            except queue.Full:
                                pass
                    offset += round(scene["duration"] * fps)
        except Exception as error:
            self.error = error

    def next(self):
        while True:
            try:
                return self.pending.get(timeout=0.2)
            except queue.Empty:
                if self.error:
                    raise RuntimeError(f"Capture stopped: {self.error}") from self.error
                if not self.thread.is_alive():
                    raise RuntimeError("Capture ended before the last video frame")

    def close(self):
        self.stopped.set()
        self.thread.join(timeout=12)


def wrap(text, font, width):
    lines = []
    for paragraph in text.split("\n"):
        line = ""
        for char in paragraph:
            if line and font.getlength(line + char) > width:
                lines.append(line)
                line = ""
            line += char
        lines.append(line)
    return "\n".join(lines)


def ease(t):
    t = max(0.0, min(1.0, t))
    return t * t * (3.0 - 2.0 * t)


class Composition:
    size = (1920, 1080)
    screen = (540, 138, 1320, 825)

    def __init__(self, script):
        self.script = script
        self.fonts = {size: ImageFont.truetype(script["font"], size) for size in (18, 22, 26, 28, 48)}
        self.base = Image.new("RGB", self.size, "white")
        draw = ImageDraw.Draw(self.base)
        draw.rounded_rectangle((70, 63, 110, 103), radius=15, fill="#eef2ff")
        # A little conversation mark keeps the frame quiet and friendly.
        draw.rounded_rectangle((80, 74, 100, 89), radius=6, outline="#687cba", width=2)
        draw.line((84, 89, 84, 94, 90, 89), fill="#687cba", width=2)
        draw.text((125, 64), script["brand"], font=self.fonts[28], fill="#30343d")
        draw.text((72, 1007), script["footer"], font=self.fonts[18], fill="#89909c")
        x, y, w, h = self.screen
        shadow = Image.new("RGBA", self.size)
        ImageDraw.Draw(shadow).rounded_rectangle((x, y + 12, x + w, y + h + 12), radius=28, fill=(54, 61, 81, 27))
        shadow = shadow.filter(ImageFilter.GaussianBlur(22))
        self.base.paste(shadow, (0, 0), shadow)
        self.mask = Image.new("L", (w, h))
        ImageDraw.Draw(self.mask).rounded_rectangle((0, 0, w - 1, h - 1), radius=24, fill=255)
        self.panels = [self.panel(scene) for scene in script["scenes"]]
        self.cursor = CursorArt()

    def panel(self, scene):
        panel = Image.new("RGBA", (445, 800))
        d = ImageDraw.Draw(panel)
        tag = scene["chapter"]
        pill = self.fonts[22].getlength(tag) + 36
        d.rounded_rectangle((0, 8, pill, 50), radius=21, fill="#f0f3fb")
        d.text((18, 13), tag, font=self.fonts[22], fill="#5e739e")
        title = wrap(scene["title"], self.fonts[48], 432)
        d.multiline_text((0, 88), title, font=self.fonts[48], fill="#292f3a", spacing=15)
        bottom = d.multiline_textbbox((0, 88), title, font=self.fonts[48], spacing=15)[3]
        body = wrap(scene["body"], self.fonts[26], 420)
        d.multiline_text((2, bottom + 37), body, font=self.fonts[26], fill="#727b88", spacing=17)
        if scene.get("example"):
            y = bottom + 175
            for i, text in enumerate(scene["example"]):
                x = 0 if i == 0 else 30
                label = wrap(text, self.fonts[22], 343)
                height = d.multiline_textbbox((0, 0), label, font=self.fonts[22], spacing=8)[3] + 38
                d.rounded_rectangle((x, y, x + 390, y + height), radius=22, fill="#f5f6f9" if i == 0 else "#edf2ff")
                d.multiline_text((x + 22, y + 16), label, font=self.fonts[22], fill="#58687c", spacing=8)
                y += height + 16
            d.text((4, y + 5), scene["example_label"], font=self.fonts[18], fill="#a0a5af")
        if scene.get("note"):
            d.multiline_text((2, 626), wrap(scene["note"], self.fonts[22], 410), font=self.fonts[22], fill="#8c95a3", spacing=12)
        return panel

    def frame(self, source, index, elapsed, absolute, duration, pointer):
        scene = self.script["scenes"][index]
        canvas = self.base.copy()
        x, y, w, h = self.screen
        keys = scene.get("camera", [{"at": 0, "zoom": 1, "center": [0.5, 0.5]}])
        before = keys[0]
        after = keys[0]
        for key in keys[1:]:
            if key["at"] > elapsed:
                break
            before, after = after, key
        move = after.get("move", self.script.get("camera_move_seconds", 0.65))
        t = max(0.0, min(1.0, (elapsed - after["at"]) / max(0.001, move)))
        t = 1.0 - (1.0 - t) ** 3
        zoom = before["zoom"] + (after["zoom"] - before["zoom"]) * t
        cx, cy = [a + (b - a) * t for a, b in zip(before["center"], after["center"])]
        sw, sh = source.size
        cw, ch = sw / zoom, sh / zoom
        left = max(0, min(sw - cw, cx * sw - cw / 2))
        top = max(0, min(sh - ch, cy * sh - ch / 2))
        view = source.transform((w, h), Image.Transform.EXTENT, (left, top, left + cw, top + ch), Image.Resampling.BICUBIC)
        canvas.paste(view, (x, y), self.mask)
        self.cursor.draw(canvas, pointer, (left, top, cw, ch), self.screen, self.script["viewport"][2])
        draw = ImageDraw.Draw(canvas)
        draw.rounded_rectangle((x, y, x + w, y + h), radius=24, outline="#e6e8ee", width=1)
        panel = self.panels[index]
        opacity = min(ease(elapsed / 0.3), ease((scene["duration"] - elapsed) / 0.2))
        if opacity < 1:
            panel = panel.copy()
            panel.putalpha(panel.getchannel("A").point(lambda a: int(a * opacity)))
        canvas.paste(panel, (72, 228 + round(12 * (1 - opacity))), panel)
        draw.rounded_rectangle((72, 1050, 1848, 1054), radius=2, fill="#eef0f5")
        draw.rounded_rectangle((72, 1050, 72 + max(4, int(1776 * absolute / duration)), 1054), radius=2, fill="#9eadce")
        return canvas


def action_point(director, action):
    if action["do"] not in ("click", "set", "tap", "hover", "type"):
        return None
    if "point" in action:
        return action["point"]
    if "target" in action:
        x, y, w, h = checked(director.inspect(action["target"]))["data"]["rect"]
        return x + w / 2, y + h / 2


def act(director, action):
    kind = action["do"]
    if kind == "page":
        return checked(director.page(action["target"]))
    if kind == "click":
        element = checked(director.inspect(action["target"]))["data"]
        if not element["enabled"]:
            raise RuntimeError(f"Disabled control: {action['target']}")
        return checked(director.click(element["id_hex"]))
    if kind == "set":
        element = checked(director.inspect(action["target"]))["data"]
        return checked(director.set_value(element["id_hex"], action["value"]))
    if kind in ("tap", "hover", "type"):
        x, y = action_point(director, action)
        checked(director.pointer(x, y))
        if kind in ("tap", "type"):
            checked(director.pointer(x, y, True))
            time.sleep(0.06)
            checked(director.pointer(x, y, False))
        return
    if kind == "text":
        return checked(director.text(action["value"]))
    if kind == "scroll":
        return checked(director.scroll(*action["point"], action["value"]))
    if kind == "audio_file":
        return checked(director.audio_file(action["path"]))
    if kind == "wait":
        deadline, since, previous = time.monotonic() + action.get("timeout", 120), None, None
        while time.monotonic() < deadline:
            response = director.inspect(action["target"])
            value = response.get("data", {}).get("value")
            ready = response.get("success") and ("match" not in action or re.search(action["match"], str(value)))
            if ready:
                if value != previous or since is None:
                    since, previous = time.monotonic(), value
                if time.monotonic() - since >= action.get("stable", .5):
                    return response["data"]
            else:
                since = None
            time.sleep(.1)
        raise TimeoutError(f"Timed out waiting for {action['target']}")
    raise ValueError(f"Unknown screenplay action: {kind}")


def timestamp(seconds, subtitle=False):
    milliseconds = round(seconds * 1000)
    hours, milliseconds = divmod(milliseconds, 3600000)
    minutes, milliseconds = divmod(milliseconds, 60000)
    seconds, milliseconds = divmod(milliseconds, 1000)
    return f"{hours:02}:{minutes:02}:{seconds:02},{milliseconds:03}" if subtitle else f"{minutes:02}:{seconds:02}"


def record(args):
    script = json.loads(args.script.read_text(encoding="utf-8"))
    scenes = script["scenes"]
    fps = script["fps"]
    for scene in scenes:
        scene["duration"] = round(scene["duration"] * fps) / fps
    for action in script.get("prepare", []) + [a for scene in scenes for phase in ("prepare", "actions") for a in scene.get(phase, [])]:
        if action["do"] == "audio_file":
            action["path"] = str((args.script.parent / action["path"]).resolve())
    if args.preview:
        scenes = scenes[:args.preview]
        script["scenes"] = scenes
    args.output.mkdir(parents=True, exist_ok=True)
    output = args.output / (script["slug"] + ".mp4")
    if output.exists():
        raise FileExistsError(f"Use a new output directory: {output}")
    metadata, subtitles, chapters = [";FFMETADATA1", f"title={script['brand']}"], [], []
    cursor = 0.0
    for i, scene in enumerate(scenes):
        end = cursor + scene["duration"]
        metadata.extend(["[CHAPTER]", "TIMEBASE=1/1000", f"START={round(cursor * 1000)}", f"END={round(end * 1000)}", f"title={scene['chapter']}"])
        subtitles.append(f"{i + 1}\n{timestamp(cursor, True)} --> {timestamp(end, True)}\n{scene['title'].replace(chr(10), '')}\n{scene['body'].replace(chr(10), '')}\n")
        chapters.append(f"{timestamp(cursor)}  {scene['chapter']}")
        cursor = end
    (args.output / "chapters.ffmeta").write_text("\n".join(metadata), encoding="utf-8")
    (args.output / "chapters.txt").write_text("\n".join(chapters), encoding="utf-8")
    (args.output / "captions.srt").write_text("\n".join(subtitles), encoding="utf-8")
    director = UIDirector(port=args.port, timeout=12)
    if not director.wait_ready():
        raise RuntimeError("Start the isolated demo app with --director-port first")
    checked(director.viewport(*script["viewport"]))
    for action in script.get("prepare", []):
        act(director, action)
        time.sleep(0.35)
    checked(director.page(scenes[0]["page"]))
    time.sleep(0.8)
    compositor = Composition(script)
    log = (args.output / "encoder.log").open("w", encoding="utf-8")
    command = [imageio_ffmpeg.get_ffmpeg_exe(), "-hide_banner", "-loglevel", "warning", "-f", "rawvideo", "-pixel_format", "rgb24", "-video_size", "1920x1080", "-framerate", str(fps), "-i", "pipe:0", "-f", "ffmetadata", "-i", str(args.output / "chapters.ffmeta"), "-map", "0:v", "-map_metadata", "1", "-an", "-c:v", "libx264", "-preset", "fast", "-crf", "18", "-pix_fmt", "yuv420p", "-movflags", "+faststart", str(output)]
    encoder = subprocess.Popen(command, stdin=subprocess.PIPE, stderr=log, creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
    capture = Capture(UIDirector(port=args.port, timeout=12), script)
    saved = set()
    started = time.monotonic()
    try:
        for _ in range(round(cursor * fps)):
            index, elapsed, absolute, (width, height, pixels), pointer = capture.next()
            if (width, height) != tuple(script["viewport"][:2]):
                raise RuntimeError(f"Viewport size differs from screenplay: {(width, height)}")
            source = Image.frombytes("RGBA", (width, height), pixels).convert("RGB")
            composed = compositor.frame(source, index, elapsed, absolute, cursor, pointer)
            scene = scenes[index]
            if elapsed > scene.get("snapshot_at", min(scene["duration"] / 2, 5)) and index not in saved:
                composed.save(args.output / f"chapter-{index + 1:02}.png")
                saved.add(index)
            encoder.stdin.write(composed.tobytes())
        encoder.stdin.close()
        if encoder.wait(timeout=60):
            raise RuntimeError("Video encoder failed; see encoder.log")
        (args.output / "recording.json").write_text(json.dumps({"duration": cursor, "fps": fps, "frames": round(cursor * fps), "captured_frames": capture.frames, "held_frames": round(cursor * fps) - capture.frames, "wall_seconds": round(time.monotonic() - started, 2), "capture_mode": "frame_locked", "audio": False, "actions": capture.actions}, ensure_ascii=False, indent=2), encoding="utf-8")
        print(output, flush=True)
    finally:
        capture.close()
        if encoder.poll() is None:
            encoder.kill()
            encoder.wait()
        log.close()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("script", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--port", type=int, default=18920)
    parser.add_argument("--preview", type=int, help="Record just the first N scenes")
    record(parser.parse_args())
