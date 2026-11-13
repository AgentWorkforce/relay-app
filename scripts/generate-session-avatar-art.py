#!/usr/bin/env python3
"""Generate the monochrome session avatar atlases used by macOS and GTK.

Each shape is a 512x256 RGBA PNG holding 32 frames of 48x48 art on a 64px
grid (8px inset), white with an alpha mask; the apps tint it. The art is plain
geometry drawn here, with a slow breathing loop. Standard library only.

    python3 scripts/generate-session-avatar-art.py
"""
import math
import struct
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DESTINATIONS = [ROOT / "macos/Sources/RelayDesktop/Resources", ROOT / "gtk/resources/linux/agents"]
SHAPES = ["blob", "pebble", "bean", "egg", "squircle", "tablet", "capsule", "cylinder", "hex", "gem",
          "crystal", "wedge", "shield", "dome", "arch", "cloud", "teardrop", "leaf", "square", "sparkle",
          "clover", "flower", "heart", "house", "star"]
FRAMES, CELL, INSET, SIZE, SAMPLES = 32, 64, 8, 48, 4


def polygon(points):
    def inside(x, y):
        hit = False
        for (x1, y1), (x2, y2) in zip(points, points[1:] + points[:1]):
            if (y1 > y) != (y2 > y) and x < (x2 - x1) * (y - y1) / (y2 - y1) + x1:
                hit = not hit
        return hit
    return inside


def regular(sides, radius, rotation=0.0):
    return [(radius * math.cos(rotation + 2 * math.pi * i / sides), radius * math.sin(rotation + 2 * math.pi * i / sides))
            for i in range(sides)]


def starry(points, outer, inner):
    return [((outer if i % 2 == 0 else inner) * math.cos(-math.pi / 2 + math.pi * i / points),
             (outer if i % 2 == 0 else inner) * math.sin(-math.pi / 2 + math.pi * i / points)) for i in range(points * 2)]


def ellipse(cx, cy, rx, ry):
    return lambda x, y: ((x - cx) / rx) ** 2 + ((y - cy) / ry) ** 2 <= 1


def rounded(w, h, r):
    def inside(x, y):
        dx, dy = max(abs(x) - (w - r), 0), max(abs(y) - (h - r), 0)
        return abs(x) <= w and abs(y) <= h and dx * dx + dy * dy <= r * r
    return inside


def union(*parts):
    return lambda x, y: any(part(x, y) for part in parts)


def polar(radius_at):
    return lambda x, y: math.hypot(x, y) <= radius_at(math.atan2(y, x))


# Shapes live in [-1, 1] with y pointing down.
SHAPE_FNS = {
    "blob": polar(lambda a: 0.82 + 0.08 * math.sin(3 * a) + 0.05 * math.cos(5 * a)),
    "pebble": ellipse(0, 0.05, 0.9, 0.68),
    "bean": union(ellipse(-0.35, 0.05, 0.5, 0.62), ellipse(0.35, -0.05, 0.5, 0.62)),
    "egg": lambda x, y: (x / (0.68 * (1 + 0.18 * y))) ** 2 + (y / 0.9) ** 2 <= 1,
    "squircle": lambda x, y: (x / 0.85) ** 4 + (y / 0.85) ** 4 <= 1,
    "tablet": rounded(0.6, 0.88, 0.2),
    "capsule": rounded(0.48, 0.9, 0.48),
    "cylinder": union(rounded(0.66, 0.62, 0.05), ellipse(0, -0.62, 0.66, 0.24), ellipse(0, 0.62, 0.66, 0.24)),
    "hex": polygon(regular(6, 0.9, math.pi / 6)),
    "gem": polygon([(-0.85, -0.3), (-0.45, -0.75), (0.45, -0.75), (0.85, -0.3), (0, 0.88)]),
    "crystal": polygon([(0, -0.95), (0.5, -0.4), (0.5, 0.45), (0, 0.95), (-0.5, 0.45), (-0.5, -0.4)]),
    "wedge": polygon([(-0.85, 0.8), (0.85, 0.8), (0.85, -0.8)]),
    "shield": polygon([(-0.8, -0.85), (0.8, -0.85), (0.8, 0.05), (0, 0.92), (-0.8, 0.05)]),
    "dome": lambda x, y: (y <= 0.6 and x * x + (y - 0.6) ** 2 <= 0.85 ** 2 and y >= -0.6) or (abs(x) <= 0.85 and 0.6 <= y <= 0.78),
    "arch": lambda x, y: (abs(x) <= 0.8 and -0.05 <= y <= 0.85 or x * x + (y + 0.05) ** 2 <= 0.8 ** 2) and not (
        abs(x) <= 0.35 and y >= 0.15 or x * x + (y - 0.15) ** 2 <= 0.35 ** 2),
    "cloud": union(ellipse(-0.45, 0.2, 0.42, 0.4), ellipse(0.1, -0.15, 0.5, 0.5), ellipse(0.5, 0.2, 0.4, 0.38),
                   ellipse(0, 0.35, 0.85, 0.3)),
    "teardrop": lambda x, y: math.hypot(x, y - 0.3) <= 0.6 or (y < 0.3 and abs(x) <= 0.6 * (y + 0.95) / 1.25),
    "leaf": lambda x, y: ellipse(0, 0, 0.55, 0.92)(x * 0.8 + y * 0.6, -x * 0.6 + y * 0.8),
    "square": rounded(0.78, 0.78, 0.08),
    "sparkle": polygon(starry(4, 0.95, 0.28)),
    "clover": union(*(ellipse(0.42 * math.cos(a), 0.42 * math.sin(a), 0.45, 0.45) for a in (0, math.pi / 2, math.pi, 3 * math.pi / 2))),
    "flower": polar(lambda a: 0.6 + 0.3 * abs(math.cos(2.5 * a))),
    "heart": lambda x, y: (x * x + (-y * 1.1 + 0.15) ** 2 - 0.55) ** 3 - x * x * (-y * 1.1 + 0.15) ** 3 <= 0,
    "house": polygon([(-0.75, -0.05), (0, -0.85), (0.75, -0.05), (0.6, -0.05), (0.6, 0.85), (-0.6, 0.85), (-0.6, -0.05)]),
    "star": polygon(starry(5, 0.95, 0.42)),
}


def frame_alpha(inside, phase):
    scale = 0.9 + 0.05 * math.sin(2 * math.pi * phase)
    bob = 0.03 * math.sin(2 * math.pi * phase + math.pi / 2)
    rows = []
    for py in range(SIZE):
        row = []
        for px in range(SIZE):
            hits = 0
            for sy in range(SAMPLES):
                for sx in range(SAMPLES):
                    x = ((px + (sx + 0.5) / SAMPLES) / SIZE * 2 - 1) / scale
                    y = ((py + (sy + 0.5) / SAMPLES) / SIZE * 2 - 1 - bob) / scale
                    hits += inside(x, y)
            row.append(round(255 * hits / SAMPLES ** 2))
        rows.append(row)
    return rows


def png(width, height, alpha):
    raw = b"".join(b"\x00" + b"".join(bytes((255, 255, 255, a)) for a in row) for row in alpha)
    def chunk(tag, data):
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def main():
    for shape in SHAPES:
        atlas = [[0] * (CELL * 8) for _ in range(CELL * 4)]
        for index in range(FRAMES):
            ox, oy = index % 8 * CELL + INSET, index // 8 * CELL + INSET
            for y, row in enumerate(frame_alpha(SHAPE_FNS[shape], index / FRAMES)):
                atlas[oy + y][ox:ox + SIZE] = row
        data = png(CELL * 8, CELL * 4, atlas)
        for destination in DESTINATIONS:
            (destination / f"session-avatar-{shape}.png").write_bytes(data)
    print(f"Generated {len(SHAPES)} avatar atlases with {FRAMES} frames each.")


if __name__ == "__main__":
    main()
