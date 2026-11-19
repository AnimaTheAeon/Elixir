"""Recolor the suite's glowing orb to Elixir's rose-magenta.
Usage: elixir_orb.py SRC_256.png OUTDIR"""
import sys, os
import numpy as np
from PIL import Image

src = Image.open(sys.argv[1]).convert("RGBA")
a = np.asarray(src).astype(np.float32) / 255
rgb, alpha = a[..., :3], a[..., 3:]
# Map by luminance onto a rose ramp so shading and highlights carry over.
lum = (0.299 * rgb[..., 0] + 0.587 * rgb[..., 1] + 0.114 * rgb[..., 2])[..., None]
stops = np.array([0.0, 0.35, 0.65, 0.85, 1.0])
cols = np.array([[26, 0, 20], [110, 16, 92], [214, 70, 172], [248, 170, 226], [255, 246, 252]]) / 255
out = np.stack([np.interp(lum[..., 0], stops, cols[:, c]) for c in range(3)], -1)
img = Image.fromarray((np.concatenate([out, alpha], -1) * 255).round().astype(np.uint8), "RGBA")
d = sys.argv[2]; os.makedirs(d, exist_ok=True)
for p in [16, 24, 32, 48, 64, 72, 256]:
    (img if p == 256 else img.resize((p, p), Image.LANCZOS)).save(f"{d}/elixir_{p}.png")
