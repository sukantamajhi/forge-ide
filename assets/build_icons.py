"""Draws the Forge logo: the SVG and every raster size, from one set of coordinates.

The mark is a few lines of code on a dark tile: bars of different lengths, indented like source,
with one line and the text caret in orange. It says "code editor" without leaning on the name.
Small sizes (32 px and under) switch to three heavier bars and drop the caret so they stay legible.

    python assets/build_icons.py          (needs Pillow)

Writes assets/forge-logo.svg, assets/icons/icon-<size>.png and assets/forge.ico.
"""

from pathlib import Path

from PIL import Image, ImageDraw

HERE = Path(__file__).parent
GRID = 256  # all coordinates below are on a 256 x 256 grid

WHITE, GREY, ORANGE = (0xF2, 0xF2, 0xF2), (0x8A, 0x90, 0xA0), (0xFF, 0x8A, 0x1F)
TILE_TOP, TILE_BOTTOM = (0x24, 0x27, 0x30), (0x12, 0x14, 0x19)
TILE_RADIUS = 58

# (x, y, width, height, colour): rounded bars, the corner radius is half the height.
LINES = [
    (56, 57, 84, 22, WHITE),
    (86, 97, 116, 22, GREY),
    (86, 137, 62, 22, ORANGE),
    (56, 177, 100, 22, WHITE),
    (162, 131, 12, 34, ORANGE),  # the caret, after the orange line
]
LINES_SMALL = [
    (52, 59, 100, 34, WHITE),
    (84, 111, 92, 34, ORANGE),
    (52, 163, 124, 34, GREY),
]

SIZES = [16, 24, 32, 48, 64, 128, 256, 512]
ICO_SIZES = [16, 24, 32, 48, 64, 128, 256]
SUPERSAMPLE = 4


def hex_color(rgb):
    return "#%02X%02X%02X" % rgb


def svg():
    bars = "\n  ".join(
        f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{min(h / 2, 6) if w < 20 else h / 2:g}" fill="{hex_color(c)}"/>'
        for x, y, w, h, c in LINES
    )
    return f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {GRID} {GRID}" width="256" height="256" role="img" aria-label="Forge">
  <title>Forge</title>
  <defs>
    <linearGradient id="tile" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="{hex_color(TILE_TOP)}"/>
      <stop offset="1" stop-color="{hex_color(TILE_BOTTOM)}"/>
    </linearGradient>
  </defs>
  <rect width="{GRID}" height="{GRID}" rx="{TILE_RADIUS}" fill="url(#tile)"/>
  {bars}
</svg>
"""


def vertical_gradient(size, top, bottom):
    image = Image.new("RGB", (size, size))
    draw = ImageDraw.Draw(image)
    for y in range(size):
        t = y / max(size - 1, 1)
        draw.line([(0, y), (size, y)], fill=tuple(round(a + (b - a) * t) for a, b in zip(top, bottom)))
    return image


def render(size):
    big = size * SUPERSAMPLE
    scale = big / GRID
    mask = Image.new("L", (big, big), 0)
    ImageDraw.Draw(mask).rounded_rectangle([0, 0, big - 1, big - 1], radius=TILE_RADIUS * scale, fill=255)
    canvas = vertical_gradient(big, TILE_TOP, TILE_BOTTOM)
    draw = ImageDraw.Draw(canvas)
    for x, y, w, h, colour in LINES_SMALL if size <= 32 else LINES:
        radius = (h / 2 if w > 20 else 6) * scale
        draw.rounded_rectangle([x * scale, y * scale, (x + w) * scale - 1, (y + h) * scale - 1], radius=radius, fill=colour)
    out = canvas.convert("RGBA")
    out.putalpha(mask)
    return out.resize((size, size), Image.LANCZOS)


def main():
    (HERE / "forge-logo.svg").write_text(svg(), encoding="utf-8")
    icons = HERE / "icons"
    icons.mkdir(exist_ok=True)
    images = {size: render(size) for size in SIZES}
    for size, image in images.items():
        image.save(icons / f"icon-{size}.png")
    images[256].save(HERE / "forge.ico", sizes=[(s, s) for s in ICO_SIZES], append_images=[])
    print("wrote forge-logo.svg, icons/icon-*.png and forge.ico")


if __name__ == "__main__":
    main()
