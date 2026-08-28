#!/usr/bin/env python3
"""Render one owner-review contact sheet from a successful seed-owner capture."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

REPO_ROOT = Path(__file__).resolve().parents[1]
MOSAICS = [
    ("HEIGHT 2048x2048", REPO_ROOT / "docs/evidence/worldgen-mosaics/seed-712816/height.png"),
    ("BIOME 2048x2048", REPO_ROOT / "docs/evidence/worldgen-mosaics/seed-712816/biome.png"),
    ("VEGETATION 2048x2048", REPO_ROOT / "docs/evidence/worldgen-mosaics/seed-712816/vegetation.png"),
]


def sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def font(size: int) -> ImageFont.ImageFont:
    return ImageFont.load_default(size=size)


def fit(image: Image.Image, width: int, height: int) -> Image.Image:
    copy = image.convert("RGB")
    copy.thumbnail((width, height), Image.Resampling.LANCZOS)
    canvas = Image.new("RGB", (width, height), "#171717")
    x = (width - copy.width) // 2
    y = (height - copy.height) // 2
    canvas.paste(copy, (x, y))
    return canvas


def load_capture_rows(run_dir: Path) -> list[dict]:
    result_path = run_dir / "result.json"
    result = json.loads(result_path.read_text())
    if result.get("passed") is not True or result.get("seed") != 712816:
        raise RuntimeError("run is not a successful seed-712816 capture")
    captures = result.get("captures")
    if not isinstance(captures, list) or len(captures) != 3:
        raise RuntimeError("expected exactly three capture positions")

    rows: list[dict] = []
    for capture in captures:
        screenshots = capture.get("screenshots")
        if not isinstance(screenshots, list) or len(screenshots) != 4:
            raise RuntimeError(f"capture {capture.get('label')} does not have four screenshots")
        for screenshot in screenshots:
            path = Path(screenshot["path"])
            if not path.is_file():
                raise RuntimeError(f"missing screenshot: {path}")
            digest = sha256_file(path)
            if digest != screenshot.get("sha256"):
                raise RuntimeError(f"screenshot hash mismatch: {path}")
            rows.append(
                {
                    "capture": capture["label"],
                    "direction": screenshot["direction"],
                    "position": screenshot["position"],
                    "yaw": screenshot["yaw"],
                    "pitch": screenshot["pitch"],
                    "path": path,
                    "sha256": digest,
                }
            )
    return rows


def render(run_dir: Path, output: Path) -> None:
    rows = load_capture_rows(run_dir)
    tile_w, image_h, label_h = 427, 240, 74
    gap, margin = 14, 28
    columns = 4
    first_person_rows = 3
    sheet_w = margin * 2 + columns * tile_w + (columns - 1) * gap
    title_h = 88
    mosaic_h = 390
    sheet_h = title_h + first_person_rows * (image_h + label_h + gap) + mosaic_h + margin

    sheet = Image.new("RGB", (sheet_w, sheet_h), "#101010")
    draw = ImageDraw.Draw(sheet)
    title_font = font(30)
    label_font = font(18)
    small_font = font(15)

    draw.text((margin, 16), "Solaris seed 712816 - owner terrain review", fill="white", font=title_font)
    draw.text(
        (margin, 54),
        f"Fresh tellus_like world - real Java 26.1.2 client - run {run_dir.name}",
        fill="#cfcfcf",
        font=label_font,
    )

    y0 = title_h
    for index, item in enumerate(rows):
        row, column = divmod(index, columns)
        x = margin + column * (tile_w + gap)
        y = y0 + row * (image_h + label_h + gap)
        with Image.open(item["path"]) as image:
            tile = fit(image, tile_w, image_h)
        sheet.paste(tile, (x, y))
        pos = item["position"]
        draw.text(
            (x, y + image_h + 6),
            f"{item['capture']} / {item['direction']}",
            fill="white",
            font=label_font,
        )
        draw.text(
            (x, y + image_h + 30),
            f"xyz {pos['x']:.3f}, {pos['y']:.3f}, {pos['z']:.3f}   yaw {item['yaw']:.1f} pitch {item['pitch']:.1f}",
            fill="#cfcfcf",
            font=small_font,
        )
        draw.text(
            (x, y + image_h + 51),
            item["sha256"][:24],
            fill="#8f8f8f",
            font=small_font,
        )

    mosaic_top = y0 + first_person_rows * (image_h + label_h + gap) + 16
    mosaic_tile_w = (sheet_w - 2 * margin - 2 * gap) // 3
    mosaic_image_h = 300
    for index, (label, path) in enumerate(MOSAICS):
        if not path.is_file():
            raise RuntimeError(f"missing mosaic: {path}")
        x = margin + index * (mosaic_tile_w + gap)
        with Image.open(path) as image:
            tile = fit(image, mosaic_tile_w, mosaic_image_h)
        sheet.paste(tile, (x, mosaic_top))
        draw.text((x, mosaic_top + mosaic_image_h + 8), label, fill="white", font=label_font)
        draw.text(
            (x, mosaic_top + mosaic_image_h + 32),
            sha256_file(path)[:32],
            fill="#8f8f8f",
            font=small_font,
        )

    output.parent.mkdir(parents=True, exist_ok=True)
    sheet.save(output, format="PNG", optimize=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run-dir", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    render(args.run_dir.resolve(), args.output.resolve())
    print(args.output)
