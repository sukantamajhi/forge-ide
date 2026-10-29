"""Draws the Forge logo: the SVG and every raster size, from one set of coordinates.

The mark is an anvil in profile that is also the letter F: the face and horn are the top
bar, the waist is the stem, the step is the middle bar, the foot is the base. A single
spark flies off the horn. Molten orange on a dark warm tile.

    python assets/build_icons.py          (needs Pillow)

Writes assets/forge-logo.svg, assets/icons/icon-<size>.png and assets/forge.ico.
"""

from pathlib import Path

from PIL import Image, ImageDraw

HERE = Path(__file__).parent
GRID = 256  # all coordinates below are on a 256 x 256 grid

# Mark polygons, already centred in the tile.
ANVIL = [
    # face and horn (top bar of the F)
    [(50, 76), (156, 76), (186, 80), (216, 90), (200, 97), (186, 105), (172, 113), (50, 113)],
    # waist and foot (stem of the F, widening into the base)
    [(66, 113), (104, 113), (104, 188), (130, 188), (130, 214), (40, 214), (40, 188), (66, 188)],
    # step (middle bar of the F)
    [(104, 136), (152, 136), (152, 160), (104, 160)],
]
SPARK = [  # four-point star with concave sides
    (200, 28), (206, 46), (224, 52), (206, 58), (200, 76), (194, 58), (176, 52), (194, 46),
]
SPARK_CENTRE = (200, 52)

TILE_RADIUS = 58
TILE_TOP, TILE_BOTTOM = (0x2E, 0x27, 0x23), (0x14, 0x11, 0x10)
EMBER_TOP, EMBER_BOTTOM = (0xFF, 0xB8, 0x4A), (0xE9, 0x4F, 0x1B)
SPARK_COLOR = (0xFF, 0xDC, 0x8A)

SIZES = [16, 24, 32, 48, 64, 128, 256, 512]
ICO_SIZES = [16, 24, 32, 48, 64, 128, 256]
SUPERSAMPLE = 4


def hex_color(rgb):
    return "#%02X%02X%02X" % rgb


def svg():
    def points(poly):
        return " ".join(f"{x},{y}" for x, y in poly)

    anvil = "\n    ".join(f'<polygon points="{points(p)}"/>' for p in ANVIL)
    return f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {GRID} {GRID}" width="256" height="256" role="img" aria-label="Forge">
  <title>Forge</title>
  <defs>
    <linearGradient id="tile" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="{hex_color(TILE_TOP)}"/>
      <stop offset="1" stop-color="{hex_color(TILE_BOTTOM)}"/>
    </linearGradient>
    <linearGradient id="ember" gradientUnits="userSpaceOnUse" x1="0" y1="76" x2="0" y2="214">
      <stop offset="0" stop-color="{hex_color(EMBER_TOP)}"/>
      <stop offset="1" stop-color="{hex_color(EMBER_BOTTOM)}"/>
    </linearGradient>
  </defs>
  <rect width="{GRID}" height="{GRID}" rx="{TILE_RADIUS}" fill="url(#tile)"/>
  <g fill="url(#ember)">
    {anvil}
  </g>
  <polygon points="{points(SPARK)}" fill="{hex_color(SPARK_COLOR)}"/>
</svg>
"""


def vertical_gradient(size, top, bottom, y0=0, y1=None):
    """An RGB image fading from `top` at row y0 to `bottom` at row y1 (in pixels)."""
    y1 = size if y1 is None else y1
    image = Image.new("RGB", (size, size))
    draw = ImageDraw.Draw(image)
    for y in range(size):
        t = min(max((y - y0) / max(y1 - y0, 1), 0.0), 1.0)
        draw.line([(0, y), (size, y)], fill=tuple(round(a + (b - a) * t) for a, b in zip(top, bottom)))
    return image


def render(size):
    big = size * SUPERSAMPLE
    scale = big / GRID

    def scaled(poly):
        return [(x * scale, y * scale) for x, y in poly]

    tile_mask = Image.new("L", (big, big), 0)
    ImageDraw.Draw(tile_mask).rounded_rectangle([0, 0, big - 1, big - 1], radius=TILE_RADIUS * scale, fill=255)

    mark_mask = Image.new("L", (big, big), 0)
    mark_draw = ImageDraw.Draw(mark_mask)
    for poly in ANVIL:
        mark_draw.polygon(scaled(poly), fill=255)

    spark_mask = Image.new("L", (big, big), 0)
    # The spark is dropped at small sizes where it would only muddy the horn.
    if size >= 48:
        ImageDraw.Draw(spark_mask).polygon(scaled(SPARK), fill=255)

    canvas = vertical_gradient(big, TILE_TOP, TILE_BOTTOM)
    canvas.paste(vertical_gradient(big, EMBER_TOP, EMBER_BOTTOM, 76 * scale, 214 * scale), mask=mark_mask)
    canvas.paste(Image.new("RGB", (big, big), SPARK_COLOR), mask=spark_mask)
    out = canvas.convert("RGBA")
    out.putalpha(tile_mask)
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
