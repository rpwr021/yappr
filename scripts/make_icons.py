#!/usr/bin/env python3
"""Generate menu-bar icons for Yappr.

- icons/idle.png       : monochrome waveform, TEMPLATE image (macOS tints it to
                         match the light/dark menu bar, like Docker's icon).
- icons/rec_XX.png     : colored waveform frames, cycled to animate while recording.
- icons/think.png      : monochrome dots (processing).

Run:  uv run python make_icons.py
"""

import math
import os

from PIL import Image, ImageDraw

ICON_DIR = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
                        "src", "yappr", "assets", "icons")
os.makedirs(ICON_DIR, exist_ok=True)

# Retina: menu bar is ~22pt tall -> 2x = 44px. Keep a compact waveform.
H = 44
W = 56
BARS = 7
BAR_W = 4
GAP = (W - BARS * BAR_W) // (BARS + 1)
MARGIN_Y = 7  # vertical padding


def _bar_x(i):
    return GAP + i * (BAR_W + GAP)


def draw_waveform(heights, color_fn):
    """heights: list[0..1] per bar. color_fn(i)->RGBA. Returns RGBA image."""
    img = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    max_h = H - 2 * MARGIN_Y
    for i, frac in enumerate(heights):
        bh = max(BAR_W, int(frac * max_h))
        x0 = _bar_x(i)
        y0 = (H - bh) // 2
        # rounded bars
        d.rounded_rectangle([x0, y0, x0 + BAR_W, y0 + bh],
                            radius=BAR_W // 2, fill=color_fn(i))
    return img


def grad_color(i):
    """Blue -> cyan -> violet gradient across bars (for the recording frames)."""
    t = i / max(1, BARS - 1)
    # interpolate through three stops
    stops = [(64, 156, 255), (40, 220, 235), (150, 110, 255)]
    if t < 0.5:
        a, b, u = stops[0], stops[1], t / 0.5
    else:
        a, b, u = stops[1], stops[2], (t - 0.5) / 0.5
    r = int(a[0] + (b[0] - a[0]) * u)
    g = int(a[1] + (b[1] - a[1]) * u)
    bl = int(a[2] + (b[2] - a[2]) * u)
    return (r, g, bl, 255)


def mono(_i):
    # Solid white; as a TEMPLATE image macOS recolors by luminance/alpha.
    return (255, 255, 255, 255)


# --- idle: gentle static waveform, monochrome template ---
idle_heights = [0.35, 0.6, 0.85, 1.0, 0.85, 0.6, 0.35]
draw_waveform(idle_heights, mono).save(os.path.join(ICON_DIR, "idle.png"))

# --- thinking: three centered dots, monochrome template ---
think = Image.new("RGBA", (W, H), (0, 0, 0, 0))
dd = ImageDraw.Draw(think)
r = 4
for k, cx in enumerate((W // 2 - 14, W // 2, W // 2 + 14)):
    dd.ellipse([cx - r, H // 2 - r, cx + r, H // 2 + r], fill=(255, 255, 255, 255))
think.save(os.path.join(ICON_DIR, "think.png"))

# --- recording: animated colored waveform frames ---
FRAMES = 8
for f in range(FRAMES):
    phase = f / FRAMES * 2 * math.pi
    heights = []
    for i in range(BARS):
        # each bar oscillates with a per-bar phase offset -> traveling wave
        v = 0.5 + 0.5 * math.sin(phase + i * 0.9)
        heights.append(0.2 + 0.8 * v)
    draw_waveform(heights, grad_color).save(
        os.path.join(ICON_DIR, f"rec_{f:02d}.png"))

print(f"wrote idle.png, think.png, and {FRAMES} rec_*.png to {ICON_DIR}")
