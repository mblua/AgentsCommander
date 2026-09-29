"""Regenerate the AgentsCommander desktop wallpapers (2560x1440 PNG).

Manual tool, not part of any build or CI. Requirements the repo does not provide:
  - Python 3 with Pillow (pip install pillow)
  - The Bahnschrift font, shipped with Windows (C:/Windows/Fonts/bahnschrift.ttf)
Run from the repository root on Windows:
  python docs/assets/wallpapers/generate-wallpapers.py
"""
import random
from pathlib import Path
from PIL import Image, ImageChops, ImageDraw, ImageFilter, ImageFont

W, H = 2560, 1440
OUT = Path(__file__).resolve().parent
LOGO = Image.open(OUT.parents[2] / "src-tauri" / "icons" / "icon.png").convert("RGB")
FONT = "C:/Windows/Fonts/bahnschrift.ttf"


def font(size, variation="Bold"):
    f = ImageFont.truetype(FONT, size)
    f.set_variation_by_name(variation)
    return f


def starfield(clear_shapes):
    """Faint stars on black, kept away from the areas in clear_shapes."""
    bg = Image.new("RGB", (W, H), (0, 0, 0))
    random.seed(7)
    d = ImageDraw.Draw(bg)
    for _ in range(900):
        x, y = random.randrange(W), random.randrange(H)
        v = random.choice((30, 45, 60, 80, 110, 160))
        r = 1 if v < 150 else 2
        d.ellipse((x - r / 2, y - r / 2, x + r / 2, y + r / 2), fill=(v, v + 10, min(255, v + 40)))
    mask = Image.new("L", (W, H), 0)
    md = ImageDraw.Draw(mask)
    for kind, box in clear_shapes:
        (md.ellipse if kind == "ellipse" else lambda b, fill: md.rounded_rectangle(b, 60, fill=fill))(box, fill=255)
    return Image.composite(Image.new("RGB", (W, H), (0, 0, 0)), bg, mask.filter(ImageFilter.GaussianBlur(60)))


def put_logo(bg, cx, cy, size, fade=70):
    lg = LOGO.resize((size, size), Image.LANCZOS)
    m = Image.new("L", (size, size), 0)
    ImageDraw.Draw(m).ellipse((size * 0.05, size * 0.05, size * 0.95, size * 0.95), fill=255)
    m = m.filter(ImageFilter.GaussianBlur(fade))
    x, y = cx - size // 2, cy - size // 2
    region = bg.crop((x, y, x + size, y + size))
    bg.paste(Image.composite(ImageChops.screen(region, lg), region, m), (x, y))


def put_text(bg, text, x, y, f, spacing, color=(225, 236, 255), glow=(40, 120, 255), glow_alpha=0.7, center=False):
    d = ImageDraw.Draw(bg)
    widths = [d.textlength(c, font=f) for c in text]
    total = sum(widths) + spacing * (len(text) - 1)
    if center:
        x = (W - total) / 2
    m = Image.new("L", (W, H), 0)
    md = ImageDraw.Draw(m)
    xx = x
    for c, w in zip(text, widths):
        md.text((xx, y), c, font=f, fill=255)
        xx += w + spacing
    halo = m.filter(ImageFilter.GaussianBlur(14)).point(lambda v: int(v * glow_alpha))
    bg = Image.composite(Image.new("RGB", (W, H), glow), bg, halo)
    return Image.composite(Image.new("RGB", (W, H), color), bg, m), total


def v1_horizontal():
    cy = H // 2
    bg = starfield([("ellipse", (510, cy - 320, 1150, cy + 320)), ("rect", (1150, cy - 200, 2150, cy + 170))])
    put_logo(bg, 830, cy, 700)
    bg, _ = put_text(bg, "AGENTS", 1190, cy - 150, font(150), 22)
    bg, _ = put_text(bg, "COMMANDER", 1194, cy + 20, font(110, "Regular"), 30,
                     color=(120, 170, 255), glow=(20, 80, 220), glow_alpha=0.5)
    ImageDraw.Draw(bg).rectangle((1194, cy - 6, 2094, cy - 2), fill=(40, 110, 255))
    return bg


def v2_minimal():
    bg = starfield([])
    put_logo(bg, W - 420, H - 420, 440, fade=45)
    f = font(44)
    tw = ImageDraw.Draw(bg).textlength("AGENTS COMMANDER", font=f)
    bg, _ = put_text(bg, "AGENTS COMMANDER", W - 420 - (tw + 8 * 15) / 2, H - 175, f, 8, glow_alpha=0.5)
    return bg


def v3_centered():
    cx, cy, size = W // 2, int(H * 0.44), 820
    bg = starfield([("ellipse", (cx - 330, cy - 330, cx + 330, cy + 330))])
    put_logo(bg, cx, cy, size)
    bg, _ = put_text(bg, "AGENTS COMMANDER", 0, cy + size // 2 + 10, font(96), 18, center=True)
    return bg


if __name__ == "__main__":
    for name, build in (("v1-horizontal", v1_horizontal), ("v2-minimal", v2_minimal), ("v3-centered", v3_centered)):
        path = OUT / f"agentscommander-wallpaper-{name}-2560x1440.png"
        build().save(path, optimize=True)
        print(path)
