#!/usr/bin/env python3
"""Rasterize deterministic Reborn Quad previews for host-side visual review.

Mirrors the native renderer: flat quads, coverage-mask quads (glyph pages and
the shape/icon atlas, tinted), artwork and library thumbnails. Thumbnails and
artwork use procedural covers keyed by their id, since previews carry no art.
"""

from __future__ import annotations

import argparse
import json
import random
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFilter

WIDTH, HEIGHT = 480, 360
BG = (6, 7, 10, 255)


def rgba(value: int) -> tuple[int, int, int, int]:
    return ((value >> 24) & 255, (value >> 16) & 255, (value >> 8) & 255, value & 255)


def mask_page(path: Path, width: int) -> Image.Image:
    data = path.read_bytes()
    return Image.frombytes("L", (width, len(data) // width), data)


PALETTE = [
    ((20, 24, 60), (214, 96, 58), (255, 196, 96)),
    ((10, 50, 60), (30, 120, 130), (160, 230, 220)),
    ((60, 20, 50), (160, 60, 110), (255, 150, 170)),
    ((30, 30, 34), (90, 100, 110), (230, 235, 240)),
    ((14, 52, 30), (88, 160, 70), (230, 250, 150)),
    ((70, 40, 14), (210, 150, 60), (255, 230, 150)),
    ((30, 16, 70), (110, 80, 200), (200, 190, 255)),
    ((10, 30, 70), (40, 100, 190), (150, 210, 255)),
]


def cover(seed: int, size: int = 160, rounded: float = 0.085) -> Image.Image:
    """A procedural cover; rounded corners are baked into alpha, like the app does."""
    rng = random.Random(seed)
    a, b, c = PALETTE[seed % len(PALETTE)]
    im = Image.new("RGB", (size, size))
    d = ImageDraw.Draw(im)
    for y in range(size):
        t = y / (size - 1)
        d.line([(0, y), (size, y)], fill=tuple(int(a[i] * (1 - t) + b[i] * t) for i in range(3)))
    for _ in range(5):
        x, y, s = rng.randint(0, size), rng.randint(0, size), rng.randint(size // 6, size // 2)
        layer = Image.new("RGBA", (size, size), (0, 0, 0, 0))
        ImageDraw.Draw(layer).ellipse([x - s, y - s, x + s, y + s], fill=c + (rng.randint(50, 100),))
        im.paste(layer, (0, 0), layer)
    im = im.filter(ImageFilter.GaussianBlur(size / 80)).convert("RGBA")
    scale = 4
    mask = Image.new("L", (size * scale, size * scale), 0)
    ImageDraw.Draw(mask).rounded_rectangle([0, 0, size * scale - 1, size * scale - 1], size * scale * rounded, fill=255)
    im.putalpha(mask.resize((size, size), Image.Resampling.LANCZOS))
    return im


class Rasterizer:
    def __init__(self, root: Path):
        self.regular = mask_page(root / "assets/fonts/reborn-regular.a8", 1024)
        self.semibold = mask_page(root / "assets/fonts/reborn-semibold.a8", 1024)
        self.fallback = mask_page(root / "assets/fonts/reborn-fallback.a8", 1024)
        self.masks = mask_page(root / "assets/icons/reborn-masks.a8", 512)
        self.covers: dict[int, Image.Image] = {}

    def cover_for(self, key: int, size: int) -> Image.Image:
        if key not in self.covers:
            self.covers[key] = cover(key >> 8 if key else 0)
        return self.covers[key].resize((size, size), Image.Resampling.BILINEAR)

    def draw(self, canvas: Image.Image, quad: dict) -> None:
        x, y = round(quad["x"]), round(quad["y"])
        w, h = max(1, round(quad["w"])), max(1, round(quad["h"]))
        tint = rgba(quad["color"])
        tex = quad.get("tex", "Solid")
        if tex == "Solid":
            layer = Image.new("RGBA", (w, h), tint)
        elif tex in ("Art", "CollectionArt"):
            layer = self.cover_for(0, w).resize((w, h))
            layer.putalpha(ImageChops.multiply(layer.getchannel("A"), Image.new("L", (w, h), tint[3])))
        elif isinstance(tex, dict) and "Thumb" in tex:
            layer = self.cover_for(tex["Thumb"], w).resize((w, h))
            layer.putalpha(ImageChops.multiply(layer.getchannel("A"), Image.new("L", (w, h), tint[3])))
        else:
            if isinstance(tex, dict) and "Font" in tex:
                page = (self.regular, self.semibold, self.fallback)[tex["Font"]]
            else:
                page = self.masks
            sx, sy, sw, sh = quad["src"]
            if sw == 0 or sh == 0:
                return
            source = page.crop((sx, sy, sx + sw, sy + sh))
            if (sw, sh) != (w, h):
                source = source.resize((w, h), Image.Resampling.BILINEAR)
            layer = Image.new("RGBA", (w, h), tint)
            layer.putalpha(ImageChops.multiply(source, Image.new("L", (w, h), tint[3])))
        # Clip to the canvas.
        if x < 0 or y < 0 or x + w > WIDTH or y + h > HEIGHT:
            clipped = Image.new("RGBA", (WIDTH, HEIGHT), (0, 0, 0, 0))
            clipped.paste(layer, (x, y))
            canvas.alpha_composite(clipped)
        else:
            canvas.alpha_composite(layer, (x, y))

    def render(self, quads: list[dict], background=BG) -> Image.Image:
        canvas = Image.new("RGBA", (WIDTH, HEIGHT), background if len(background) == 4 else background + (255,))
        for quad in quads:
            self.draw(canvas, quad)
        return canvas.convert("RGB")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("preview_dir", type=Path)
    parser.add_argument("--output", type=Path, default=None)
    parser.add_argument("--scale", type=int, default=1, help="integer upscale of the PNGs for review")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    output = args.output or args.preview_dir / "png"
    output.mkdir(parents=True, exist_ok=True)
    raster = Rasterizer(root)
    for spec in sorted(args.preview_dir.glob("*.json")):
        if spec.name == "manifest.json":
            continue
        document = json.loads(spec.read_text())
        if "quads" not in document:
            continue  # data for other tools, e.g. boot-layout.json
        image = raster.render(document["quads"])
        if args.scale > 1:
            image = image.resize((WIDTH * args.scale, HEIGHT * args.scale), Image.Resampling.NEAREST)
        image.save(output / f"{spec.stem}.png")
    images = sorted(p for p in output.glob("*.png") if p.stem != "contact-sheet")
    cell_w, cell_h = WIDTH * args.scale, HEIGHT * args.scale
    sheet = Image.new("RGB", (cell_w * 3, (cell_h + 26) * ((len(images) + 2) // 3)), (15, 18, 21))
    labels = ImageDraw.Draw(sheet)
    for i, path in enumerate(images):
        x, y = i % 3 * cell_w, i // 3 * (cell_h + 26)
        labels.text((x + 16, y + 7), path.stem, fill=(242, 240, 235))
        sheet.paste(Image.open(path), (x, y + 26))
    sheet.save(output / "contact-sheet.png")
    print(f"wrote {len(images)} {cell_w}x{cell_h} preview PNGs and contact sheet to {output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
