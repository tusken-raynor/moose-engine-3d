"""Generate assets/textures/default.png, the `default` material's texture: Moose v1's
default texture (`get_dflt_bitmap` in v1's src/textures.rs, commit df9c711), as it was.
16x16, gray-green with an orange top row and left column (a grid where it repeats) and
three black texels near the middle.

Usage (from repo root): python3 tools/gen_default_texture.py .
"""
import os, struct, sys, zlib

ROOT = sys.argv[1]

FILL = 4673355      # v1's 0xRRGGBB values
EDGE = 13063478
MARK = 0


def rgb(v):
    return (v >> 16 & 255, v >> 8 & 255, v & 255, 255)


def png_rgba(width, height, pixels):
    """A PNG file (8-bit RGBA) from rows of (r, g, b, a) tuples."""
    raw = b"".join(b"\x00" + bytes(c for px in row for c in px) for row in pixels)

    def chunk(kind, data):
        return (struct.pack(">I", len(data)) + kind + data
                + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF))

    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


data = [FILL] * 256
for i in range(256):
    if i < 16 or i % 16 == 0:
        data[i] = EDGE
for i in (119, 120, 135):
    data[i] = MARK
rows = [[rgb(data[y * 16 + x]) for x in range(16)] for y in range(16)]
with open(os.path.join(ROOT, "assets/textures/default.png"), "wb") as f:
    f.write(png_rgba(16, 16, rows))
print("wrote assets/textures/default.png (16x16)")
