"""Рисует логотип Axiom во всех размерах и собирает axiom.ico.

Только стандартная библиотека: фигуры заданы как SDF (signed distance field),
сглаживание — суперсэмплингом. Запуск: python assets/make_icons.py
"""

import math
import struct
import zlib
from pathlib import Path

HERE = Path(__file__).resolve().parent

BG_TOP, BG_BOTTOM = (0x1D, 0x1D, 0x26), (0x0F, 0x0F, 0x13)
MARK_TOP, MARK_BOTTOM = (0xC3, 0xCB, 0xFF), (0x7D, 0x8C, 0xFF)
DOT = (0xFF, 0xFF, 0xFF)

# Геометрия в долях стороны: «Λ» без перекладины и точка вместо неё.
APEX, LEFT, RIGHT = (0.5, 0.26), (0.27, 0.74), (0.73, 0.74)
STROKE = 0.09
DOT_C, DOT_R = (0.5, 0.60), 0.055
CORNER = 0.226


def lerp(a, b, t):
    return tuple(x + (y - x) * t for x, y in zip(a, b))


def sd_round_rect(x, y, half, radius):
    qx, qy = abs(x - 0.5) - half + radius, abs(y - 0.5) - half + radius
    outside = math.hypot(max(qx, 0.0), max(qy, 0.0))
    return outside + min(max(qx, qy), 0.0) - radius


def sd_segment(x, y, a, b):
    px, py = x - a[0], y - a[1]
    bx, by = b[0] - a[0], b[1] - a[1]
    t = max(0.0, min(1.0, (px * bx + py * by) / (bx * bx + by * by)))
    return math.hypot(px - bx * t, py - by * t)


def render(size, ss=4):
    rows = []
    for j in range(size):
        row = bytearray()
        for i in range(size):
            acc = [0.0, 0.0, 0.0, 0.0]
            for sj in range(ss):
                for si in range(ss):
                    x = (i + (si + 0.5) / ss) / size
                    y = (j + (sj + 0.5) / ss) / size
                    if sd_round_rect(x, y, 0.5, CORNER) > 0:
                        continue
                    color = lerp(BG_TOP, BG_BOTTOM, y)
                    stroke = min(sd_segment(x, y, APEX, LEFT), sd_segment(x, y, APEX, RIGHT))
                    if stroke < STROKE / 2:
                        color = lerp(MARK_TOP, MARK_BOTTOM, (y - APEX[1]) / (LEFT[1] - APEX[1]))
                    if math.hypot(x - DOT_C[0], y - DOT_C[1]) < DOT_R:
                        color = DOT
                    acc[0] += color[0]
                    acc[1] += color[1]
                    acc[2] += color[2]
                    acc[3] += 1
            n = ss * ss
            alpha = acc[3] / n
            if acc[3]:
                row += bytes(round(c / acc[3]) for c in acc[:3]) + bytes([round(alpha * 255)])
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
    images = []
    for size in (16, 20, 24, 32, 40, 48, 64, 128, 256):
        data = png(size, render(size, 6 if size <= 32 else 4))
        images.append((size, data))
    (HERE / "axiom.ico").write_bytes(ico(images))
    (HERE / "logo-512.png").write_bytes(png(512, render(512, 2)))
    print("axiom.ico и logo-512.png готовы")


if __name__ == "__main__":
    main()
