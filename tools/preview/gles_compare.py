#!/usr/bin/env python3
"""Render preview quad lists with the production GL code on a host GLES2
context and compare each frame with the Python rasteriser.

Builds crates/reborn-graphics/tests/gles_harness.c (Mesa, EGL surfaceless) and
checks glyph/mask textures (luminance-alpha), tinting, UV mapping, thumbnails
and artwork end to end. It cannot prove behaviour on the player's Mali-400.

Usage: gles_compare.py QUADS_DIR [--only NAME ...]
"""
from __future__ import annotations

import argparse
import json
import math
import subprocess
import sys
import tempfile
from pathlib import Path

from PIL import Image, ImageChops

sys.path.insert(0, str(Path(__file__).resolve().parent))
from render_previews import HEIGHT, WIDTH, Rasterizer  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
TEX = {"Solid": 0, "Mask": 4, "Art": 5, "CollectionArt": 6, "Glyphs": 9}


# Mirrors reborn-graphics/src/lib.rs: ART_SIZE, THUMB_SIZE, level_cell and
# level_source (a picture with its half and quarter copies, gutters replicated).
ART_SIZE, THUMB_SIZE = 180, 128
THUMB_CELL_W, THUMB_CELL_H = THUMB_SIZE + 2 + THUMB_SIZE // 2 + 2, THUMB_SIZE + 2
THUMB_COLUMNS = 2048 // THUMB_CELL_W


def level_at(n: int, i: int) -> tuple[int, int, int]:
    return [(1, 1, n), (n + 3, 1, n // 2), (n + 3, n // 2 + 3, n // 4)][i]


def level_source(n: int, edge: float) -> list[int]:
    i = next((i for i in (2, 1, 0) if level_at(n, i)[2] >= edge), 0)
    x, y, e = level_at(n, i)
    return [x, y, e, e]


def drawn_edge(q: dict) -> float:
    if q.get("warp"):
        c = q["warp"]["corners"]
        return max(math.dist(c[i], c[(i + 1) % 4]) for i in range(4))
    return max(q["w"], q["h"])


def level_block(image: Image.Image, n: int) -> Image.Image:
    """The picture and its box-filtered copies, each with a replicated gutter."""
    block = Image.new("RGBA", (n + 2 + n // 2 + 2, n + 2))
    level = image
    for i in range(3):
        x, y, e = level_at(n, i)
        if i:
            level = level.reduce(2)
        padded = Image.new("RGBA", (e + 2, e + 2))
        padded.paste(level, (1, 1))
        padded.paste(level.crop((0, 0, e, 1)), (1, 0))
        padded.paste(level.crop((0, e - 1, e, e)), (1, e + 1))
        padded.paste(padded.crop((1, 0, 2, e + 2)), (0, 0))
        padded.paste(padded.crop((e, 0, e + 1, e + 2)), (e + 1, 0))
        block.paste(padded, (x - 1, y - 1))
    return block


def render_native(harness: Path, raster: Rasterizer, quads: list[dict], work: Path) -> Image.Image:
    lines, cells = [], {}
    art = collection = glyphs = False
    for q in quads:
        tex = q.get("tex", "Solid")
        if isinstance(tex, dict) and "Thumb" in tex:
            key = tex["Thumb"]
            if key not in cells:
                index = len(cells)
                cells[key] = index
                path = work / f"thumb-{index}.rgba"
                path.write_bytes(level_block(raster.cover_for(key, THUMB_SIZE), THUMB_SIZE).tobytes())
                lines.append(f"T {index % THUMB_COLUMNS} {index // THUMB_COLUMNS} {path}")
            index = cells[key]
            x, y, w, h = level_source(THUMB_SIZE, drawn_edge(q))
            kind = 7
            src = [index % THUMB_COLUMNS * THUMB_CELL_W + x, index // THUMB_COLUMNS * THUMB_CELL_H + y, w, h]
        elif isinstance(tex, dict) and "Font" in tex:
            kind, src = 1 + tex["Font"], q["src"]
        else:
            kind, src = TEX[tex], q.get("src", [0, 0, 0, 0])
            if kind in (5, 6):
                src = level_source(ART_SIZE, drawn_edge(q))
            if kind == 5 and not art:
                path = work / "art.rgba"
                block = level_block(raster.cover_for(0, ART_SIZE), ART_SIZE)
                path.write_bytes(block.tobytes())
                lines.insert(0, f"A {block.width} {block.height} {path}")
                art = True
            if kind == 9 and not glyphs:
                lines.insert(0, f"G {raster.glyphs_path}")
                glyphs = True
            if kind == 6 and not collection:
                path = work / "collection.rgba"
                block = level_block(raster.cover_for(0, ART_SIZE), ART_SIZE)
                path.write_bytes(block.tobytes())
                lines.insert(0, f"C {block.width} {block.height} {path}")
                collection = True
        if q.get("warp"):
            corners = " ".join(f"{v}" for point in q["warp"]["corners"] for v in point)
            colors = " ".join(str(c) for c in q["warp"]["colors"])
            lines.append(f"W {corners} {colors} {kind} {src[0]} {src[1]} {src[2]} {src[3]}")
            continue
        lines.append("Q {x} {y} {w} {h} {color} {kind} {s0} {s1} {s2} {s3}".format(
            x=q["x"], y=q["y"], w=q["w"], h=q["h"], color=q["color"], kind=kind,
            s0=src[0], s1=src[1], s2=src[2], s3=src[3]))
    (work / "quads.txt").write_text("\n".join(lines) + "\n")
    out = work / "frame.ppm"
    subprocess.run([str(harness), str(ROOT / "assets"), str(work / "quads.txt"), str(out)], check=True)
    return Image.open(out).convert("RGB")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("quads", type=Path)
    parser.add_argument("--only", nargs="*")
    parser.add_argument("--save", type=Path, help="write side-by-side PNGs here")
    args = parser.parse_args()
    raster = Rasterizer(ROOT)
    raster.load_glyphs(args.quads)
    raster.glyphs_path = args.quads / "glyph-page.a8"
    failures = 0
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        harness = work / "harness"
        flags = subprocess.check_output(["pkg-config", "--cflags", "--libs", "libdrm", "gbm", "egl", "glesv2"], text=True).split()
        subprocess.run(["cc", "-std=c11", "-Wall", "-Wextra", str(ROOT / "crates/reborn-graphics/tests/gles_harness.c"),
                        *flags, "-o", str(harness)], check=True)
        for spec in sorted(args.quads.glob("*.json")):
            if spec.name in ("manifest.json", "boot-layout.json") or (args.only and spec.stem not in args.only):
                continue
            doc = json.loads(spec.read_text())
            if "quads" not in doc:
                continue
            native = render_native(harness, raster, doc["quads"], work)
            python = raster.render(doc["quads"])
            diff = ImageChops.difference(native, python).convert("L")
            data = list(diff.getdata())
            mean = sum(data) / len(data)
            large = sum(1 for v in data if v > 40) / len(data)
            ok = mean < 1.5 and large < 0.01
            failures += not ok
            print(f"{'ok  ' if ok else 'FAIL'} {spec.stem:34s} mean {mean:5.2f}  >40: {large * 100:5.2f} %")
            if args.save:
                args.save.mkdir(parents=True, exist_ok=True)
                sheet = Image.new("RGB", (WIDTH * 3, HEIGHT))
                sheet.paste(python, (0, 0))
                sheet.paste(native, (WIDTH, 0))
                sheet.paste(diff.point(lambda v: min(255, v * 4)).convert("RGB"), (WIDTH * 2, 0))
                sheet.save(args.save / f"{spec.stem}.png")
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
