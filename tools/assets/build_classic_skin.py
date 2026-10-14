#!/usr/bin/env python3
"""Draw the pictures of the shipped Classic skin (skins/classic/images).

Small gradient strips and frames, nine-sliced or stretched by skin.json, so
the skin stays a few kilobytes. Deterministic: rerun after editing a colour.
"""

from pathlib import Path

from PIL import Image

OUT = Path(__file__).resolve().parents[2] / "skins/classic/images"


def hexrgb(h: str) -> tuple[int, int, int, int]:
    h = h.lstrip("#")
    return tuple(int(h[i:i + 2], 16) for i in (0, 2, 4)) + (255,)


def mix(a, b, t):
    return tuple(round(x + (y - x) * t) for x, y in zip(a, b))


def vertical(w: int, h: int, top: str, bottom: str) -> Image.Image:
    a, b = hexrgb(top), hexrgb(bottom)
    im = Image.new("RGBA", (w, h))
    for y in range(h):
        for x in range(w):
            im.putpixel((x, y), mix(a, b, y / max(1, h - 1)))
    return im


def frame(im: Image.Image, color: str) -> Image.Image:
    c = hexrgb(color)
    w, h = im.size
    for x in range(w):
        im.putpixel((x, 0), c)
        im.putpixel((x, h - 1), c)
    for y in range(h):
        im.putpixel((0, y), c)
        im.putpixel((w - 1, y), c)
    return im


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    # Title bar: brushed light grey with a hairline below.
    bar = vertical(4, 30, "#FCFCFD", "#D3D6DB")
    for x in range(4):
        bar.putpixel((x, 29), hexrgb("#9EA2A9"))
    bar.save(OUT / "titlebar.png")
    # Selected row: the classic blue with a lighter top edge.
    sel = vertical(4, 33, "#7DB7F7", "#2A69D2")
    for x in range(4):
        sel.putpixel((x, 0), hexrgb("#A9CFFA"))
        sel.putpixel((x, 32), hexrgb("#1F5BBE"))
    sel.save(OUT / "highlight.png")
    # Progress and volume: recessed grey track, glossy blue fill.
    frame(vertical(8, 12, "#C9CBCF", "#F1F2F4"), "#8F939A").save(OUT / "track.png")
    frame(vertical(8, 12, "#8CC0F8", "#2D6CD4"), "#2257B4").save(OUT / "fill.png")
    # Battery: outline with a nub; the level is a green bar drawn beneath.
    battery = Image.new("RGBA", (28, 14), (0, 0, 0, 0))
    edge = hexrgb("#4A4E55")
    for x in range(25):
        battery.putpixel((x, 0), edge)
        battery.putpixel((x, 13), edge)
    for y in range(14):
        battery.putpixel((0, y), edge)
        battery.putpixel((24, y), edge)
    for y in range(4, 10):
        for x in range(25, 28):
            battery.putpixel((x, y), edge)
    battery.save(OUT / "battery.png")
    vertical(4, 10, "#9BE38B", "#3AA32C").save(OUT / "battery_fill.png")
    vertical(4, 10, "#FF9C8A", "#D93025").save(OUT / "battery_low.png")


if __name__ == "__main__":
    main()
