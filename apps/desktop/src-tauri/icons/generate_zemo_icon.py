#!/usr/bin/env python3
"""Generate the original ZEMO macOS app icon locally. No text, no third-party art."""

from __future__ import annotations

from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

SIZE = 1024
OUT = Path(__file__).resolve().parent


def rounded_rect(
    draw: ImageDraw.ImageDraw,
    box: tuple[int, int, int, int],
    radius: int,
    fill: tuple[int, int, int, int],
) -> None:
    draw.rounded_rectangle(box, radius=radius, fill=fill)


def main() -> None:
    image = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    canvas = ImageDraw.Draw(image)

    # macOS-style squircle canvas
    inset = 48
    rounded_rect(
        canvas,
        (inset, inset, SIZE - inset, SIZE - inset),
        radius=230,
        fill=(230, 238, 228, 255),
    )

    highlight = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    ImageDraw.Draw(highlight).ellipse(
        (80, 40, 620, 420),
        fill=(255, 255, 255, 48),
    )
    image = Image.alpha_composite(image, highlight.filter(ImageFilter.GaussianBlur(48)))
    canvas = ImageDraw.Draw(image)

    cx = cy = SIZE // 2
    radius = 318
    canvas.ellipse(
        (cx - radius - 10, cy - radius - 10, cx + radius + 10, cy + radius + 10),
        fill=(197, 212, 200, 255),
    )
    canvas.ellipse(
        (cx - radius, cy - radius, cx + radius, cy + radius),
        fill=(23, 88, 58, 255),
    )

    # Folder motif (behind)
    folder = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    folder_draw = ImageDraw.Draw(folder)
    tab = (330, 360, 500, 410)
    body = (300, 400, 690, 690)
    folder_draw.rounded_rectangle(tab, radius=18, fill=(232, 220, 190, 255))
    folder_draw.rounded_rectangle(body, radius=36, fill=(214, 198, 160, 255))
    image = Image.alpha_composite(image, folder)
    canvas = ImageDraw.Draw(image)

    # Document motif (front)
    doc = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    doc_draw = ImageDraw.Draw(doc)
    left, top, right, bottom = 390, 300, 720, 720
    fold = 78
    doc_draw.polygon(
        [
            (left, top),
            (right - fold, top),
            (right, top + fold),
            (right, bottom),
            (left, bottom),
        ],
        fill=(255, 255, 255, 255),
    )
    doc_draw.polygon(
        [
            (right - fold, top),
            (right, top + fold),
            (right - fold, top + fold),
        ],
        fill=(214, 224, 214, 255),
    )
    for index, y in enumerate((430, 490, 550, 610)):
        width = 210 if index < 3 else 150
        doc_draw.rounded_rectangle(
            (430, y, 430 + width, y + 18),
            radius=8,
            fill=(23, 88, 58, 46 if index < 3 else 32),
        )
    image = Image.alpha_composite(image, doc)

    source = OUT / "zemo-icon-1024.png"
    image.save(source, "PNG")

    sizes = {
        "32x32.png": 32,
        "128x128.png": 128,
        "128x128@2x.png": 256,
        "icon.png": 1024,
    }
    for name, edge in sizes.items():
        image.resize((edge, edge), Image.Resampling.LANCZOS).save(OUT / name, "PNG")

    iconset = OUT / "ZEMO.iconset"
    if iconset.exists():
        for child in iconset.iterdir():
            child.unlink()
    else:
        iconset.mkdir()
    mapping = {
        "icon_16x16.png": 16,
        "icon_16x16@2x.png": 32,
        "icon_32x32.png": 32,
        "icon_32x32@2x.png": 64,
        "icon_128x128.png": 128,
        "icon_128x128@2x.png": 256,
        "icon_256x256.png": 256,
        "icon_256x256@2x.png": 512,
        "icon_512x512.png": 512,
        "icon_512x512@2x.png": 1024,
    }
    for name, edge in mapping.items():
        image.resize((edge, edge), Image.Resampling.LANCZOS).save(iconset / name, "PNG")

    ico_sizes = [(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)]
    image.save(OUT / "icon.ico", format="ICO", sizes=ico_sizes)


if __name__ == "__main__":
    main()
