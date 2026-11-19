#!/usr/bin/env python3
from __future__ import annotations

import argparse
import os
import struct
import zlib
from pathlib import Path

LOGO_SVG = """<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64" width="64" height="64" role="img" aria-label="meerai">
  <defs>
    <linearGradient id="meerai-bg" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0%" stop-color="#141923" />
      <stop offset="100%" stop-color="#0b0d10" />
    </linearGradient>
    <linearGradient id="meerai-m" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0%" stop-color="#5eead4" />
      <stop offset="100%" stop-color="#2dd4bf" />
    </linearGradient>
  </defs>
  <rect x="2" y="2" width="60" height="60" rx="14" fill="url(#meerai-bg)" stroke="#262c36" stroke-width="1.5" />
  <path
    fill="url(#meerai-m)"
    d="M14 17.6h7.2v28.8H14z M42.8 17.6H50v28.8h-7.2z M21.2 24.8h7.2V32h-7.2z M35.6 24.8h7.2V32h-7.2z M28.4 32h7.2v7.2h-7.2z"
  />
</svg>
"""

FAVICON_SVG = """<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32" width="32" height="32" role="img" aria-label="meerai">
  <rect x="1" y="1" width="30" height="30" rx="7" fill="#0b0d10" stroke="#262c36" stroke-width="1.25" />
  <path
    fill="#2dd4bf"
    d="M6 8h4v16H6z M22 8h4v16h-4z M10 12h4v4h-4z M18 12h4v4h-4z M14 16h4v4h-4z"
  />
</svg>
"""

BG_TOP = (0x14, 0x19, 0x23)
BG_BOTTOM = (0x0B, 0x0D, 0x10)
BORDER = (0x26, 0x2C, 0x36)
TEAL_TOP = (0x5E, 0xEA, 0xD4)
TEAL_BOTTOM = (0x2D, 0xD4, 0xBF)


def lerp_rgb(a: tuple[int, int, int], b: tuple[int, int, int], t: float) -> tuple[float, float, float]:
    return (
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    )


def in_rounded_rect(nx: float, ny: float, half: float, radius: float) -> bool:
    ax = abs(nx - 0.5)
    ay = abs(ny - 0.5)
    inner = half - radius
    if ax > half or ay > half:
        return False
    if ax <= inner or ay <= inner:
        return True
    dx = ax - inner
    dy = ay - inner
    return dx * dx + dy * dy <= radius * radius


def in_block_m(nx: float, ny: float) -> bool:
    gx = (nx - 6.0 / 32.0) / (2.0 / 32.0)
    gy = (ny - 8.0 / 32.0) / (2.0 / 32.0)
    if gx < 0.0 or gx > 10.0 or gy < 0.0 or gy > 8.0:
        return False
    if gx <= 2.0 or gx >= 8.0:
        return True
    if 2.0 < gx <= 4.0 or 6.0 <= gx < 8.0:
        return 2.0 <= gy <= 4.0
    if 4.0 < gx < 6.0:
        return 4.0 <= gy <= 6.0
    return False


def render_rgba(size: int, samples: int = 4) -> bytearray:
    buf = bytearray(size * size * 4)
    outer_half = 30.0 / 64.0
    outer_r = 7.0 / 32.0
    inner_half = 28.0 / 64.0
    inner_r = 6.0 / 32.0
    inv_samples = 1.0 / (samples * samples)

    for y in range(size):
        for x in range(size):
            r_sum = 0.0
            g_sum = 0.0
            b_sum = 0.0
            a_sum = 0.0
            for sy in range(samples):
                ny = (y + (sy + 0.5) / samples) / size
                for sx in range(samples):
                    nx = (x + (sx + 0.5) / samples) / size
                    if not in_rounded_rect(nx, ny, outer_half, outer_r):
                        continue
                    if in_block_m(nx, ny):
                        t = max(0.0, min(1.0, (ny - 0.25) / 0.5))
                        cr, cg, cb = lerp_rgb(TEAL_TOP, TEAL_BOTTOM, t)
                    elif not in_rounded_rect(nx, ny, inner_half, inner_r):
                        cr, cg, cb = BORDER
                    else:
                        t = max(0.0, min(1.0, (nx + ny) * 0.5))
                        cr, cg, cb = lerp_rgb(BG_TOP, BG_BOTTOM, t)
                    r_sum += cr
                    g_sum += cg
                    b_sum += cb
                    a_sum += 255.0

            idx = (y * size + x) * 4
            if a_sum > 0.0:
                alpha_norm = a_sum * inv_samples
                scale = inv_samples / (alpha_norm / 255.0)
                buf[idx] = min(255, round(r_sum * scale))
                buf[idx + 1] = min(255, round(g_sum * scale))
                buf[idx + 2] = min(255, round(b_sum * scale))
                buf[idx + 3] = min(255, round(alpha_norm))
    return buf


def png_chunk(kind: bytes, payload: bytes) -> bytes:
    crc = zlib.crc32(kind + payload) & 0xFFFFFFFF
    return struct.pack(">I", len(payload)) + kind + payload + struct.pack(">I", crc)


def encode_png(size: int, rgba: bytearray) -> bytes:
    stride = size * 4
    rows = bytearray()
    for y in range(size):
        rows.append(0)
        start = y * stride
        rows.extend(rgba[start : start + stride])
    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    idat = zlib.compress(bytes(rows), level=9)
    return b"".join(
        [
            b"\x89PNG\r\n\x1a\n",
            png_chunk(b"IHDR", ihdr),
            png_chunk(b"IDAT", idat),
            png_chunk(b"IEND", b""),
        ]
    )


def encode_ico(images: list[tuple[int, bytes]]) -> bytes:
    header = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    entries = []
    payloads = []
    for size, png_bytes in images:
        dim = 0 if size >= 256 else size
        entries.append(
            struct.pack(
                "<BBBBHHII",
                dim,
                dim,
                0,
                0,
                1,
                32,
                len(png_bytes),
                offset,
            )
        )
        payloads.append(png_bytes)
        offset += len(png_bytes)
    return b"".join([header, *entries, *payloads])


def generate_assets(out_dir: Path) -> list[Path]:
    out_dir.mkdir(parents=True, exist_ok=True)
    logo_path = out_dir / "logo.svg"
    favicon_svg_path = out_dir / "favicon.svg"
    png_16_path = out_dir / "favicon-16x16.png"
    png_32_path = out_dir / "favicon-32x32.png"
    apple_path = out_dir / "apple-touch-icon.png"
    ico_path = out_dir / "favicon.ico"

    logo_path.write_text(LOGO_SVG, encoding="utf-8")
    favicon_svg_path.write_text(FAVICON_SVG, encoding="utf-8")

    png_16 = encode_png(16, render_rgba(16))
    png_32 = encode_png(32, render_rgba(32))
    png_48 = encode_png(48, render_rgba(48))
    png_180 = encode_png(180, render_rgba(180))

    png_16_path.write_bytes(png_16)
    png_32_path.write_bytes(png_32)
    apple_path.write_bytes(png_180)
    ico_path.write_bytes(encode_ico([(16, png_16), (32, png_32), (48, png_48)]))

    return [
        logo_path,
        favicon_svg_path,
        ico_path,
        png_16_path,
        png_32_path,
        apple_path,
    ]


def parse_args(default_out: Path) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        prog="generate_favicon.py",
        description="Generate the meerai SVG logo and browser favicon bundle.",
    )
    parser.add_argument(
        "--out",
        type=Path,
        default=default_out,
        help="Destination directory for SVG and favicon assets",
    )
    return parser.parse_args()


def main() -> int:
    repo_root = Path(__file__).resolve().parent.parent
    args = parse_args(repo_root / "meerai-web-client" / "assets")
    written = generate_assets(args.out.resolve())
    for path in written:
        print(f"wrote {os.path.relpath(path, repo_root)} ({path.stat().st_size} bytes)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
