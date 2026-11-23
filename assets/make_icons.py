"""Рисует логотип Axiom во всех размерах: axiom.ico (Windows) и PNG для Linux.

Только стандартная библиотека: фигуры заданы как SDF (signed distance field),
сглаживание — суперсэмплингом. Запуск: python assets/make_icons.py
"""

import math
import struct
import zlib
from pathlib import Path

HERE = Path(__file__).resolve().parent

# Плоский минимализм: тёмная плитка, белая «Λ», акцентная точка.
TILE = (0x0F, 0x0F, 0x13)
MARK = (0xF4, 0xF5, 0xFA)
DOT = (0x8B, 0x9C, 0xFF)

# Геометрия в долях стороны.
APEX, LEFT, RIGHT = (0.5, 0.285), (0.305, 0.715), (0.695, 0.715)
STROKE = 0.074
DOT_C, DOT_R = (0.5, 0.605), 0.047
CORNER = 0.226


def sd_round_rect(x, y, half, radius):
    qx, qy = abs(x - 0.5) - half + radius, abs(y - 0.5) - half + radius
    outside = math.hypot(max(qx, 0.0), max(qy, 0.0))
    return outside + min(max(qx, qy), 0.0) - radius


def sd_segment(x, y, a, b):
    px, py = x - a[0], y - a[1]
    bx, by = b[0] - a[0], b[1] - a[1]
    t = max(0.0, min(1.0, (px * bx + py * by) / (bx * bx + by * by)))
    return math.hypot(px - bx * t, py - by * t)


def color_at(x, y):
    if sd_round_rect(x, y, 0.5, CORNER) > 0:
        return None
    if math.hypot(x - DOT_C[0], y - DOT_C[1]) < DOT_R:
        return DOT
    if min(sd_segment(x, y, APEX, LEFT), sd_segment(x, y, APEX, RIGHT)) < STROKE / 2:
        return MARK
    return TILE


def render(size, ss=4):
    rows = []
    for j in range(size):
        row = bytearray()
        for i in range(size):
            r = g = b = hits = 0
            for sj in range(ss):
                for si in range(ss):
                    c = color_at((i + (si + 0.5) / ss) / size, (j + (sj + 0.5) / ss) / size)
                    if c:
                        r, g, b, hits = r + c[0], g + c[1], b + c[2], hits + 1
            if hits:
                row += bytes((round(r / hits), round(g / hits), round(b / hits), round(255 * hits / (ss * ss))))
            else:
                row += b"\0\0\0\0"
        rows.append(bytes(row))
    return rows


def png(size, rows):
    raw = b"".join(b"\0" + r for r in rows)

    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    header = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")


def ico(images):
    head = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    entries, blobs = b"", b""
    for size, data in images:
        dim = 0 if size >= 256 else size
        entries += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(data), offset)
        blobs += data
        offset += len(data)
    return head + entries + blobs


def main():
    cache = {}

    def get(size):
        if size not in cache:
            cache[size] = png(size, render(size, 6 if size <= 32 else 4 if size <= 256 else 2))
        return cache[size]

    (HERE / "axiom.ico").write_bytes(ico([(s, get(s)) for s in (16, 20, 24, 32, 40, 48, 64, 128, 256)]))
    linux = HERE / "linux"
    linux.mkdir(exist_ok=True)
    for size in (16, 24, 32, 48, 64, 128, 256, 512):
        (linux / f"axiom-{size}.png").write_bytes(get(size))
    (HERE / "logo-512.png").write_bytes(get(512))
    print("axiom.ico, logo-512.png и assets/linux/*.png готовы")


if __name__ == "__main__":
    main()
