"""Real-time Windows capture; all demonstrations use the public UI Director."""

import argparse
import json
import subprocess
import sys
import threading
import time
import urllib.request
from pathlib import Path

from film import FFMPEG, load, presenter
from windows_capture import Frame, InternalCaptureControl, WindowsCapture

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from ui_director import UIDirector


def checked(result):
    if not result.get("success"):
        raise RuntimeError(result)
    return result.get("data")


def inspect(ui, target):
    if isinstance(target, dict):
        matches = [
            item
            for item in ui.list_elements()
            if item["kind"] == target["kind"] and item.get("value") in target["values"]
        ]
        if len(matches) != 1:
            raise LookupError(
                f"Expected one control matching {target}; found {len(matches)}"
            )
        return checked(ui.inspect(matches[0]["id_hex"]))
    deadline = time.monotonic() + 3
    while True:
        result = ui.inspect(target)
        if result.get("success"):
            return result["data"]
        exact = [e for e in ui.list_elements(target) if e["label"] == target]
        if len(exact) == 1:
            return checked(ui.inspect(exact[0]["id_hex"]))
        if time.monotonic() >= deadline:
            return checked(result)
        time.sleep(0.1)


def music(port, value):
    request = urllib.request.Request(
        f"http://127.0.0.1:{port}",
        data=json.dumps(value).encode(),
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(request, timeout=5) as response:
        return json.load(response)


class Take:
    def __init__(self, ui, scene, film, args):
        self.ui, self.film, self.args = ui, film, args
        self.scene, self.events, self.frames = scene, [], 0
        self.latest = None
        self.ready = threading.Event()
        self.lock = threading.Lock()
        self.started = None
        self.error = None

    def now(self):
        return time.perf_counter() - self.started if self.started else 0

    def log(self, kind, **values):
        self.events.append(dict(at=round(self.now(), 5), kind=kind, **values))

    def point(self, action):
        if "point" in action:
            return action["point"]
        if action.get("target") and action["do"] in ("click", "set", "type", "drag"):
            if action.get("row"):
                deadline = time.monotonic() + 3
                while True:
                    items = self.ui.list_elements()
                    start = next(
                        (i for i, e in enumerate(items) if e["label"] == action["row"]),
                        None,
                    )
                    if start is not None:
                        break
                    if time.monotonic() > deadline:
                        raise LookupError(action["row"])
                    time.sleep(0.1)
                item = next(
                    e for e in items[start + 1 :] if e["label"] == action["target"]
                )
                checked(self.ui.inspect(item["id_hex"]))
            else:
                item = inspect(self.ui, action["target"])
            x, y, w, h = item["rect"]
            return [x + w / 2, y + h / 2]

    def action(self, action, recording=True):
        kind, target = action["do"], action.get("target")
        if isinstance(target, dict):
            target = inspect(self.ui, target)["id_hex"]
            action = {**action, "target": target}
        if kind == "choose":
            self.action({"do": "click", "target": target}, recording)
            if recording:
                time.sleep(0.3)
            matches = [
                e
                for e in self.ui.list_elements()
                if e["label"] == action["value"] and e["kind"] == "button"
            ]
            if len(matches) != 1:
                raise RuntimeError(("Menu option", action, matches))
            self.action({"do": "click", "target": matches[0]["id_hex"]}, recording)
            return
        if kind == "ensure":
            item = inspect(self.ui, target)
            if item["value"] != action["value"]:
                if "set" in item["actions"]:
                    checked(self.ui.set_value(item["id_hex"], action["value"]))
                else:
                    checked(self.ui.click(item["id_hex"]))
            return
        if kind == "maybe_click":
            matches = [e for e in self.ui.list_elements(target) if e["label"] == target]
            if len(matches) == 1:
                self.action({"do": "click", "target": matches[0]["id_hex"]}, recording)
            return
        point = self.point(action)
        if point and recording:
            self.log("aim", point=point)
            checked(self.ui.pointer(*point))
            time.sleep(0.22)
        if kind == "page":
            checked(self.ui.page(target))
            time.sleep(0.25)
        elif kind == "click":
            if recording:
                self.log("click", point=point)
            checked(self.ui.pointer(*point, pressed=True))
            time.sleep(0.04)
            checked(self.ui.pointer(*point, pressed=False))
            time.sleep(0.04)
        elif kind == "tap":
            checked(self.ui.pointer(*point, pressed=True))
            time.sleep(0.04)
            checked(self.ui.pointer(*point, pressed=False))
            if recording:
                self.log("click", point=point)
        elif kind == "set":
            checked(self.ui.set_value(target, action["value"]))
            if recording:
                self.log("click", point=point)
        elif kind == "type":
            checked(self.ui.set_value(target, ""))
            checked(self.ui.pointer(*point, pressed=True))
            checked(self.ui.pointer(*point, pressed=False))
            value, duration = action["value"], action.get("duration", 1.2)
            start = time.perf_counter()
            shown = 0
            while shown < len(value):
                progress = min(1.0, (time.perf_counter() - start) / duration)
                count = max(shown, round(len(value) * progress))
                if count > shown:
                    checked(self.ui.set_value(target, value[:count]))
                    shown = count
                time.sleep(0.02)
        elif kind == "drag":
            end = [point[0] + action["delta"][0], point[1] + action["delta"][1]]
            checked(self.ui.pointer(*point, pressed=True))
            if recording:
                self.log("click", point=point)
            duration = action.get("duration", 0.75)
            start = time.perf_counter()
            for i in range(1, 13):
                t = i / 12
                t = t * t * (3 - 2 * t)
                p = [a + (b - a) * t for a, b in zip(point, end)]
                checked(self.ui.pointer(*p))
                if recording:
                    self.log("drag", point=p)
                time.sleep(max(0, start + duration * i / 12 - time.perf_counter()))
            checked(self.ui.pointer(*end, pressed=False))
        elif kind == "audio":
            checked(self.ui.audio_file(action.get("path")))
        elif kind == "call":
            checked(self.ui.call(action["name"], action.get("arguments", {})))
        elif kind == "music":
            music(self.args.music_port, action["value"])
        elif kind == "scroll":
            checked(self.ui.scroll(*action["point"], action["delta"]))
        elif kind == "wait":
            start = time.perf_counter()
            while time.perf_counter() - start < action.get("timeout", 12):
                values = [
                    str(e.get("value", ""))
                    for e in self.ui.list_elements(action.get("filter"))
                    if not action.get("kind") or e["kind"] == action["kind"]
                ]
                if any(action["contains"].lower() in v.lower() for v in values):
                    break
                time.sleep(0.1)
            else:
                raise TimeoutError(action)
        else:
            raise ValueError(kind)
        if recording:
            self.log("action", action=action)

    def record(self):
        performance = presenter(self.scene)
        if performance:
            checked(
                self.ui.call(
                    "companion.present",
                    {
                        "scene": {
                            **performance,
                            "duration": 120,
                            "speech": [],
                            "cues": performance["cues"][:1],
                        }
                    },
                )
            )
        for action in self.scene.get("prepare", []):
            self.action(action, False)
        time.sleep(0.35)
        path = self.args.output / self.scene["id"]
        self.capture = WindowsCapture(
            window_hwnd=self.args.hwnd, cursor_capture=False, draw_border=None
        )

        @self.capture.event
        def on_frame_arrived(frame: Frame, capture_control: InternalCaptureControl):
            with self.lock:
                self.latest = frame.frame_buffer.copy()
                self.frames += 1
            self.ready.set()

        @self.capture.event
        def on_closed():
            pass

        control = self.capture.start_free_threaded()
        vw, vh, scale = self.film["viewport"]
        checked(self.ui.pointer(vw / scale - 20, vh / scale - 20))
        if not self.ready.wait(10):
            raise TimeoutError("No capture frame")
        height, width = self.latest.shape[:2]
        # Capture contains the native frame and title bar; retain the client viewport.
        left, top = (width - vw) // 2, height - vh - (width - vw) // 2
        if left < 0 or top < 0:
            raise ValueError(("Viewport exceeds capture", width, height, vw, vh))
        fps = self.film["fps"]
        command = [
            FFMPEG,
            "-hide_banner",
            "-loglevel",
            "warning",
            "-y",
            "-f",
            "rawvideo",
            "-pixel_format",
            "bgra",
            "-video_size",
            f"{width}x{height}",
            "-framerate",
            str(fps),
            "-i",
            "pipe:0",
            "-vf",
            f"crop={vw}:{vh}:{left}:{top}",
            "-an",
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-crf",
            "16",
            "-pix_fmt",
            "yuv420p",
            str(path.with_suffix(".mp4")),
        ]
        with path.with_suffix(".log").open("w", encoding="utf8") as log:
            encoder = subprocess.Popen(
                command,
                stdin=subprocess.PIPE,
                stderr=log,
                creationflags=subprocess.CREATE_NO_WINDOW,
            )
            self.started = time.perf_counter()
            if performance:
                checked(self.ui.call("companion.present", {"scene": performance}))
                self.log("presenter_start")
            if any("media" in sound for sound in self.scene.get("sound", [])):
                self.log(
                    "media_position",
                    position=inspect(
                        self.ui,
                        self.film.get("targets", {}).get("media-position", "播放位置"),
                    )["value"],
                )

            def write():
                try:
                    for i in range(round(self.scene["duration"] * fps)):
                        time.sleep(max(0, self.started + i / fps - time.perf_counter()))
                        with self.lock:
                            pixels = self.latest
                        encoder.stdin.write(pixels.tobytes())
                except BaseException as exc:  # noqa: BLE001 -- Save failure and close the encoder pipe.
                    self.error = str(exc)
                finally:
                    encoder.stdin.close()

            writer = threading.Thread(target=write)
            writer.start()
            try:
                for action in self.scene.get("actions", []):
                    time.sleep(
                        max(0, self.started + action["at"] - time.perf_counter())
                    )
                    self.action(action)
                    if self.now() > self.scene["duration"]:
                        raise RuntimeError(f"Action overran the scene: {action}")
                writer.join()
            except BaseException as exc:  # noqa: BLE001 -- Finalize capture and preserve its failure record.
                self.error = str(exc)
                writer.join()
            finally:
                control.stop()
                if encoder.wait(timeout=30) != 0:
                    self.error = (
                        f"Video encoding failed; see {path.with_suffix('.log')}"
                    )
                self.started = None
        record = {
            "scene": self.scene,
            "events": self.events,
            "captured_frames": self.frames,
            "error": self.error,
        }
        path.with_suffix(".json").write_text(
            json.dumps(record, ensure_ascii=False, indent=2), encoding="utf8"
        )
        print(
            json.dumps(
                {"take": self.scene["id"], "frames": self.frames, "error": self.error},
                ensure_ascii=False,
            ),
            flush=True,
        )
        if self.error:
            raise RuntimeError(self.error)
        for action in self.scene.get("after", []):
            self.action(action, False)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("script", type=Path)
    parser.add_argument("ids", nargs="*")
    parser.add_argument("--hwnd", type=int, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--port", type=int, default=19821)
    parser.add_argument("--music-port", type=int, default=19828)
    args = parser.parse_args()
    script = load(args.script)
    args.output.mkdir(parents=True, exist_ok=True)
    ui = UIDirector(port=args.port, timeout=25)
    checked(ui.viewport(*script["viewport"]))
    try:
        setup = Take(ui, {}, script, args)
        for action in script.get("setup", []):
            setup.action(action, False)
        for scene in script["scenes"]:
            if not args.ids or scene["id"] in args.ids:
                Take(ui, scene, script, args).record()
    finally:
        checked(ui.call("companion.present", {"scene": None}))
