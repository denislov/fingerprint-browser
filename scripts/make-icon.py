#!/usr/bin/env python3
"""Draws the program's icon: `assets/icon.png`, `assets/icon.ico`, the SVG, and
the raw pixels the tray icon hands to the desktop.

The icon is generated rather than drawn by hand in an editor, because it is one
design at six sizes and the assets are binary: a PNG in a repository with nothing
that produced it is a file nobody can change later. The design is a rounded
square in the window's own dark palette with a fingerprint on it - ridges that
open downward, the innermost one in the colour the window uses for a thing that
is working.

Standard library only: `zlib` and `struct` are enough to write a PNG and an ICO,
and a build machine should not need an image library to reproduce an icon. Run it
from the repository root:

    python3 scripts/make-icon.py

and it writes the files under `assets/`. The output is deterministic: the same
numbers produce the same bytes, so re-running it is a no-op in git.

`assets/tray.rgba` is raw RGBA rows with no header, which is what both tray
libraries want: `tray-icon` takes RGBA and `ksni` takes ARGB32, and the byte
swap is four lines in the Linux backend rather than a second asset that could
disagree with this one. It is committed like the others, and produced here like
the others, for the same reason: a binary asset with nothing that made it is a
file nobody can change later.
"""

from __future__ import annotations

import math
import os
import struct
import zlib

# --- the design, as numbers -------------------------------------------------

#: The rounded square: background, ring, and the two ridge colours.
BACKGROUND = (0x1F, 0x1F, 0x23)
BORDER = (0x27, 0x27, 0x2A)
RIDGE = (0xE4, 0xE4, 0xE7)
ACCENT = (0x4A, 0xDE, 0x80)

#: Corner radius and border width, as a fraction of the icon's side.
CORNER = 0.22
BORDER_WIDTH = 0.012

#: The print: where it sits, the ridge radii, the stroke, and where each ridge
#: is broken. A real print is not concentric circles with aligned gaps, so each
#: ridge opens at its own angle - that is what makes the shape read as a print
#: rather than as a target.
CENTER = (0.5, 0.52)
RIDGES = (0.075, 0.145, 0.215, 0.285, 0.345)
STROKE = 0.045
#: (centre angle in degrees, half-width in degrees) of the break in each ridge.
#: Alternating ends with the loop closing in the middle, which is what a print's
#: core looks like; aligned breaks would make the shape read as a target.
BREAKS = (
    (90.0, 26.0),
    (270.0, 22.0),
    (90.0, 30.0),
    (270.0, 26.0),
    (90.0, 36.0),
)
#: The innermost ridge is the accent colour rather than the plain one, and so is
#: the core inside it.
ACCENT_RIDGE = 0
#: The whorl's core: a filled dot inside the innermost ridge.
CORE = 0.032

#: Supersampling per axis: 4x4 samples a pixel, which is what makes the arcs
#: smooth at 32 px without a drawing library.
SAMPLES = 4

#: Sizes in the ICO. Windows picks the nearest, and the shell asks for all of
#: these somewhere; 256 is the one the taskbar and Explorer use at large sizes.
ICO_SIZES = (16, 24, 32, 48, 64, 128, 256)
PNG_SIZE = 512
#: The tray icon's size. Panels ask for 16-32 logical pixels and the desktop
#: scales what it is given, so this is drawn large enough to be smooth when it
#: is scaled down on a high-density display.
TRAY_SIZE = 64


def coverage(x: float, y: float) -> tuple[float, float, float, float]:
    """The colour at a point, as premultiplied-free RGBA in 0..1.

    One point, sampled `SAMPLES` times per axis by the caller: everything here is
    a distance or an angle, which is why this is cheap enough to do without an
    image library.
    """
    covered = [0.0, 0.0, 0.0, 0.0]

    # The rounded square, as a mask: outside it the icon is transparent, so the
    # shape holds on a light desktop or a dark one.
    inset = BORDER_WIDTH / 2
    if not inside_rounded_square(x, y, inset, CORNER):
        return (0.0, 0.0, 0.0, 0.0)

    # The border ring, then the fill.
    if inside_rounded_square(x, y, inset + BORDER_WIDTH, CORNER):
        covered = [BACKGROUND[0] / 255, BACKGROUND[1] / 255, BACKGROUND[2] / 255, 1.0]
    else:
        covered = [BORDER[0] / 255, BORDER[1] / 255, BORDER[2] / 255, 1.0]

    dx = x - CENTER[0]
    dy = y - CENTER[1]
    distance = math.hypot(dx, dy)
    angle = math.degrees(math.atan2(dy, dx)) % 360.0

    if distance <= CORE:
        return (ACCENT[0] / 255, ACCENT[1] / 255, ACCENT[2] / 255, 1.0)

    for index, radius in enumerate(RIDGES):
        if abs(distance - radius) > STROKE / 2:
            continue
        centre, half = BREAKS[index]
        if abs(((angle - centre + 180.0) % 360.0) - 180.0) <= half:
            continue
        colour = ACCENT if index == ACCENT_RIDGE else RIDGE
        covered = [colour[0] / 255, colour[1] / 255, colour[2] / 255, 1.0]

    return tuple(covered)


def inside_rounded_square(x: float, y: float, inset: float, corner: float) -> bool:
    """Whether a point is inside the rounded square's outline.

    Written out rather than sampled from a signed distance field: the shape is
    the union of a cross and four corner circles, which is exact and short.
    """
    left, top, right, bottom = inset, inset, 1.0 - inset, 1.0 - inset
    if not (left <= x <= right and top <= y <= bottom):
        return False
    radius = corner - inset
    # The straight parts: anything between the corner circles.
    if left + radius <= x <= right - radius or top + radius <= y <= bottom - radius:
        return True
    corner_x = left + radius if x < 0.5 else right - radius
    corner_y = top + radius if y < 0.5 else bottom - radius
    return math.hypot(x - corner_x, y - corner_y) <= radius


def render(size: int) -> bytes:
    """One icon at `size`, as RGBA rows."""
    pixels = bytearray()
    step = 1.0 / (size * SAMPLES)
    for row in range(size):
        for column in range(size):
            total = [0.0, 0.0, 0.0, 0.0]
            for sub_row in range(SAMPLES):
                for sub_column in range(SAMPLES):
                    x = (column * SAMPLES + sub_column + 0.5) * step
                    y = (row * SAMPLES + sub_row + 0.5) * step
                    sample = coverage(x, y)
                    for channel in range(4):
                        total[channel] += sample[channel]
            count = SAMPLES * SAMPLES
            # Straight alpha, rounded: the sums are exact quarters here.
            alpha = total[3] / count
            if alpha == 0.0:
                pixels += bytes(4)
                continue
            # Un-premultiply the colour, which was averaged over covered and
            # uncovered samples alike.
            colour = [min(255, round(total[channel] / count / alpha * 255)) for channel in range(3)]
            pixels += bytes(colour) + bytes([round(alpha * 255)])
    return bytes(pixels)


def png(size: int) -> bytes:
    """A PNG holding one rendered icon."""
    rows = render(size)
    raw = b"".join(
        b"\x00" + rows[row * size * 4 : (row + 1) * size * 4] for row in range(size)
    )

    def chunk(tag: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    header = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


def ico(images: list[tuple[int, bytes]]) -> bytes:
    """An ICO holding the images, each stored as a PNG.

    PNG-compressed entries are what Vista and later read, and every size this
    writes is one Windows asks for.
    """
    header = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    entries = bytearray()
    data = bytearray()
    for size, image in images:
        # 0 means 256 in an ICO directory entry.
        side = 0 if size >= 256 else size
        entries += struct.pack("<BBBBHHII", side, side, 0, 0, 1, 32, len(image), offset)
        offset += len(image)
        data += image
    return bytes(header + entries + data)


def svg() -> str:
    """The same design as a vector, for anything that wants to scale it."""
    def arc(index: int, radius: float) -> str:
        centre, half = BREAKS[index]
        start = math.radians(centre + half)
        end = math.radians(centre + 360.0 - half)
        large = 1 if (end - start) > math.pi else 0
        x1 = CENTER[0] + radius * math.cos(start)
        y1 = CENTER[1] + radius * math.sin(start)
        x2 = CENTER[0] + radius * math.cos(end)
        y2 = CENTER[1] + radius * math.sin(end)
        colour = ACCENT if index == ACCENT_RIDGE else RIDGE
        return (
            f'    <path d="M {x1:.4f} {y1:.4f} A {radius:.4f} {radius:.4f} 0 {large} 1 '
            f'{x2:.4f} {y2:.4f}" stroke="#{colour[0]:02x}{colour[1]:02x}{colour[2]:02x}"/>\n'
        )

    paths = "".join(arc(index, radius) for index, radius in enumerate(RIDGES))
    core = (
        f'  <circle cx="{CENTER[0]}" cy="{CENTER[1]}" r="{CORE}" '
        f'fill="#{ACCENT[0]:02x}{ACCENT[1]:02x}{ACCENT[2]:02x}"/>\n'
    )
    return (
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1 1" '
        f'width="{PNG_SIZE}" height="{PNG_SIZE}">\n'
        '  <rect x="0.006" y="0.006" width="0.988" height="0.988" rx="0.214" '
        f'fill="#{BACKGROUND[0]:02x}{BACKGROUND[1]:02x}{BACKGROUND[2]:02x}" '
        f'stroke="#{BORDER[0]:02x}{BORDER[1]:02x}{BORDER[2]:02x}" stroke-width="0.012"/>\n'
        f'  <g fill="none" stroke-width="{STROKE}" stroke-linecap="round">\n'
        f"{paths}"
        "  </g>\n"
        f"{core}"
        "</svg>\n"
    )


def main() -> None:
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    assets = os.path.join(root, "assets")
    os.makedirs(assets, exist_ok=True)

    with open(os.path.join(assets, "icon.png"), "wb") as handle:
        handle.write(png(PNG_SIZE))
    with open(os.path.join(assets, "icon.ico"), "wb") as handle:
        handle.write(ico([(size, png(size)) for size in ICO_SIZES]))
    with open(os.path.join(assets, "icon.svg"), "w", encoding="utf-8") as handle:
        handle.write(svg())
    with open(os.path.join(assets, "tray.rgba"), "wb") as handle:
        handle.write(render(TRAY_SIZE))

    for name in ("icon.png", "icon.ico", "icon.svg", "tray.rgba"):
        path = os.path.join(assets, name)
        print(f"wrote {os.path.relpath(path, root)} ({os.path.getsize(path)} bytes)")


if __name__ == "__main__":
    main()
