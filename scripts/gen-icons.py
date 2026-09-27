#!/usr/bin/env python3
"""Generate AgentCowork app icons (Tauri bundle requirement).

Produces src-tauri/icons/{32x32,128x128,128x128@2x,icon}.png and icon.ico.
Run from desktop_app/ root: python3 scripts/gen-icons.py
"""
from PIL import Image, ImageDraw, ImageFilter
import os

OUT = os.path.join(os.path.dirname(__file__), "..", "src-tauri", "icons")
os.makedirs(OUT, exist_ok=True)

BASE = 512
S = 40  # margin -> rounded-square occupies [S, BASE-S]

img = Image.new("RGBA", (BASE, BASE), (0, 0, 0, 0))
d = ImageDraw.Draw(img)

# Vertical purple->blue gradient inside a rounded square
grad = Image.new("RGBA", (BASE, BASE), (0, 0, 0, 0))
gd = ImageDraw.Draw(grad)
top = (124, 58, 237)      # violet-600
bot = (37, 99, 235)       # blue-600
for y in range(BASE):
    t = y / BASE
    c = tuple(int(top[i] + (bot[i] - top[i]) * t) for i in range(3)) + (255,)
    gd.line([(0, y), (BASE, y)], fill=c)

mask = Image.new("L", (BASE, BASE), 0)
md = ImageDraw.Draw(mask)
md.rounded_rectangle([S, S, BASE - S, BASE - S], radius=110, fill=255)
img.paste(grad, (0, 0), mask)

# Soft highlight (top edge gloss)
glow = Image.new("RGBA", (BASE, BASE), (0, 0, 0, 0))
gd2 = ImageDraw.Draw(glow)
gd2.rounded_rectangle([S, S, BASE - S, BASE - S], radius=110, fill=(255, 255, 255, 0))
gd2.rounded_rectangle([S, S, BASE - S, int(BASE * 0.45)], radius=110, fill=(255, 255, 255, 26))
img = Image.alpha_composite(img, glow)

d = ImageDraw.Draw(img)

# White monogram: stylized "E" (three bars + vertical) as bold rounded strokes
stroke = 64
bar = 88
x0, x1 = int(BASE * 0.30), int(BASE * 0.70)
y_top = int(BASE * 0.30)
# vertical stem
d.rounded_rectangle([x0, y_top, x0 + stroke, int(BASE * 0.70)], radius=stroke // 2, fill=(255, 255, 255, 255))
# top / mid / bottom bars
for y in (y_top, int(BASE * 0.47), int(BASE * 0.70) - bar):
    d.rounded_rectangle([x0, y, x1, y + bar], radius=bar // 2, fill=(255, 255, 255, 255))

# Slight rounding soften
img = img.filter(ImageFilter.GaussianBlur(0.6))

for name, size in [("32x32.png", 32), ("128x128.png", 128), ("128x128@2x.png", 256), ("icon.png", 512)]:
    img.resize((size, size), Image.LANCZOS).save(os.path.join(OUT, name))
    print("wrote", name)

img.resize((256, 256), Image.LANCZOS).save(
    os.path.join(OUT, "icon.ico"), sizes=[(16, 16), (32, 32), (48, 48), (256, 256)]
)
print("wrote icon.ico")

# Also a simple PNG for the frontend favicon
img.resize((128, 128), Image.LANCZOS).save(os.path.join(os.path.dirname(__file__), "..", "ui", "icon.png"))
print("wrote ui/icon.png")
