"""Soft rounded cursor, driven by the same positions and times as Director input."""
import math
from PIL import Image, ImageDraw, ImageFilter


class Pointer:
    def __init__(self):
        self.origin = self.target = None
        self.started = self.clicked = -100.0
        self.duration = 0.45

    def aim(self, point, now):
        self.origin = self.pose(now)[0] or (point[0] + 70, point[1] + 50)
        self.target = point
        self.started = now

    def click(self, now):
        self.clicked = now

    def pose(self, now):
        if self.target is None:
            return None, 1.0, 0.0
        t = max(0, min(1, (now - self.started) / self.duration))
        smooth = 1 - (1 - t) ** 3
        point = tuple(a + (b - a) * smooth for a, b in zip(self.origin, self.target))
        click = max(0, min(1, (now - self.clicked) / .38))
        scale = 1 + .28 * math.sin(math.pi * click)
        age = now - max(self.started, self.clicked)
        opacity = min(1, max(0, (now - self.started) / .12), max(0, (4.5 - age) / .5))
        return point, scale, opacity


class CursorArt:
    def __init__(self):
        factor = 4
        size = (68 * factor, 76 * factor)
        # Quadratic curves round the arrow's tip, shoulders and tail.
        points = []
        current = (13, 10)
        for control, end in [((10, 7), (10, 13)), ((9, 31), (10, 49)),
                             ((10, 54), (14, 50)), ((17, 46), (21, 42)),
                             ((23, 41), (25, 45)), ((28, 50), (31, 56)),
                             ((33, 59), (36, 57)), ((39, 56), (42, 54)),
                             ((45, 52), (43, 49)), ((40, 44), (37, 38)),
                             ((35, 34), (39, 34)), ((46, 34), (51, 34)),
                             ((57, 34), (52, 30)), ((33, 19), (13, 10))]:
            for step in range(1, 13):
                t = step / 12
                points.append(tuple(factor * ((1-t)**2*a + 2*(1-t)*t*b + t*t*c)
                                    for a, b, c in zip(current, control, end)))
            current = end
        shape = Image.new('RGBA', size)
        draw = ImageDraw.Draw(shape)
        draw.polygon(points, fill=(244, 245, 245, 255))
        draw.line(points + points[:1], fill=(158, 163, 169, 255), width=5, joint='curve')
        shadow = Image.new('RGBA', size)
        shadow.putalpha(shape.getchannel('A').point(lambda a: int(a * .20)))
        shadow = shadow.filter(ImageFilter.GaussianBlur(3 * factor))
        shadow.alpha_composite(shape, (0, -3 * factor))
        self.image = shadow.resize((68, 76), Image.Resampling.LANCZOS)
        self.hotspot = (11, 7)

    def draw(self, canvas, pose, crop, screen, ui_scale):
        point, scale, opacity = pose
        if point is None or opacity <= 0:
            return
        left, top, width, height = crop
        x, y, w, h = screen
        px = x + (point[0] * ui_scale - left) * w / width
        py = y + (point[1] * ui_scale - top) * h / height
        if not (x <= px <= x + w and y <= py <= y + h):
            return
        art = self.image.resize((round(68 * scale), round(76 * scale)), Image.Resampling.LANCZOS)
        if opacity < 1:
            art.putalpha(art.getchannel('A').point(lambda a: int(a * opacity)))
        canvas.paste(art, (round(px - self.hotspot[0] * scale), round(py - self.hotspot[1] * scale)), art)
