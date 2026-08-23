#!/usr/bin/env python3
"""Generate the minimal ZEMO app icon for macOS and Windows.

Brand direction: simple, non-futuristic, black background, blue ``z`` and
white ``emo``. The output is deterministic and contains no third-party art.
"""

from __future__ import annotations

from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

SIZE = 1024
OUT = Path(__file__).resolve().parent
BLUE = (20, 120, 255, 255)
WHITE = (248, 249, 251, 255)
BLACK = (5, 5, 6, 255)


def load_font(size: int) -> ImageFont.FreeTypeFont:
    candidates = (
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/Library/Fonts/Arial.ttf",
        "C:/Windows/Fonts/arial.ttf",
    )
    for path in candidates:
        try:
            return ImageFont.truetype(path, size=size)
        except OSError:
            continue
    raise RuntimeError("No supported sans-serif font found for ZEMO icon generation")


def build_master() -> Image.Image:
    image = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)

    # Native-looking squircle while preserving the exact black brand ground.
    inset = 48
    draw.rounded_rectangle(
        (inset, inset, SIZE - inset, SIZE - inset),
        radius=220,
        fill=BLACK,
    )

    font = load_font(258)
    z_text = "z"
    rest_text = "emo"

    z_box = draw.textbbox((0, 0), z_text, font=font)
    rest_box = draw.textbbox((0, 0), rest_text, font=font)
    z_width = z_box[2] - z_box[0]
    rest_width = rest_box[2] - rest_box[0]
    total_width = z_width + rest_width - 8
    text_height = max(z_box[3] - z_box[1], rest_box[3] - rest_box[1])

    x = (SIZE - total_width) // 2
    y = (SIZE - text_height) // 2 - 28

    draw.text((x, y), z_text, font=font, fill=BLUE)
    draw.text((x + z_width - 8, y), rest_text, font=font, fill=WHITE)
    return image


def save_png(image: Image.Image, name: str, edge: int) -> None:
    image.resize((edge, edge), Image.Resampling.LANCZOS).save(
        OUT / name,
        "PNG",
        optimize=True,
    )


def main() -> None:
    image = build_master()

    # Canonical Tauri/macOS/Windows assets.
    image.save(OUT / "zemo-icon-1024.png", "PNG", optimize=True)
    image.save(OUT / "icon.png", "PNG", optimize=True)
    save_png(image, "32x32.png", 32)
    save_png(image, "128x128.png", 128)
    save_png(image, "128x128@2x.png", 256)

    # Windows/MSIX-compatible auxiliary tiles already tracked by the project.
    windows_tiles = {
        "Square30x30Logo.png": 30,
        "Square44x44Logo.png": 44,
        "Square71x71Logo.png": 71,
        "Square89x89Logo.png": 89,
        "Square107x107Logo.png": 107,
        "Square142x142Logo.png": 142,
        "Square150x150Logo.png": 150,
        "Square284x284Logo.png": 284,
        "Square310x310Logo.png": 310,
        "StoreLogo.png": 50,
    }
    for name, edge in windows_tiles.items():
        save_png(image, name, edge)

    image.save(
        OUT / "icon.ico",
        format="ICO",
        sizes=[
            (16, 16),
            (24, 24),
            (32, 32),
            (48, 48),
            (64, 64),
            (128, 128),
            (256, 256),
        ],
    )

    # Pillow writes a multi-resolution Apple ICNS container from the master.
    image.save(OUT / "icon.icns", format="ICNS")


if __name__ == "__main__":
    main()
