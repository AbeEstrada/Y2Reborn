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
import subprocess
import sys
import tempfile
from pathlib import Path

from PIL import Image, ImageChops

sys.path.insert(0, str(Path(__file__).resolve().parent))
from render_previews import HEIGHT, WIDTH, Rasterizer  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
TEX = {"Solid": 0, "Mask": 4, "Art": 5, "CollectionArt": 6}


def cell_bytes(image: Image.Image) -> bytes:
    """A 98x98 thumbnail cell: the 96x96 image with its edge replicated."""
    cell = Image.new("RGBA", (98, 98))
    cell.paste(image, (1, 1))
    cell.paste(image.crop((0, 0, 96, 1)), (1, 0))
    cell.paste(image.crop((0, 95, 96, 96)), (1, 97))
    cell.paste(cell.crop((1, 0, 2, 98)), (0, 0))
    cell.paste(cell.crop((96, 0, 97, 98)), (97, 0))
    return cell.tobytes()


def render_native(harness: Path, raster: Rasterizer, quads: list[dict], work: Path) -> Image.Image:
    lines, cells = [], {}
    art = collection = False
    for q in quads:
        tex = q.get("tex", "Solid")
        if isinstance(tex, dict) and "Thumb" in tex:
            key = tex["Thumb"]
            if key not in cells:
                index = len(cells)
                cells[key] = index
                path = work / f"thumb-{index}.rgba"
                path.write_bytes(cell_bytes(raster.cover_for(key, 96)))
                lines.append(f"T {index % 10} {index // 10} {path}")
            index = cells[key]
            kind, src = 7, [index % 10 * 98 + 1, index // 10 * 98 + 1, 96, 96]
        elif isinstance(tex, dict) and "Font" in tex:
            kind, src = 1 + tex["Font"], q["src"]
        else:
            kind, src = TEX[tex], q.get("src", [0, 0, 0, 0])
            if kind == 5 and not art:
                path = work / "art.rgba"
                path.write_bytes(raster.cover_for(0, 160).tobytes())
                lines.insert(0, f"A {path}")
                art = True
            if kind == 6 and not collection:
                path = work / "collection.rgba"
                path.write_bytes(raster.cover_for(0, 160).tobytes())
                lines.insert(0, f"C {path}")
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
