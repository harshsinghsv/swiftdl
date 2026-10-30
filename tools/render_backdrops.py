"""Renders soft, photographic 'silk' backdrops for the SwiftDL site.

Smooth noise, warped twice, is turned into long folds with a sine; the folds
are shaded as a surface lit from the top left, so the result reads like a
photo of draped satin or smoke. Everything stays in floating point, so there
is no banding.

    python tools/render_backdrops.py site/assets/bg
"""
import sys
import numpy as np
from scipy.ndimage import gaussian_filter, map_coordinates, zoom

W, H = 2400, 1400


def smooth_noise(rng, cells_y, cells_x, sigma=1.0):
    g = rng.standard_normal((cells_y + 4, cells_x + 4))
    g = gaussian_filter(g, sigma)
    up = zoom(g, (H / cells_y, W / cells_x), order=3)
    return up[:H, :W]


def fbm(rng, cells, octaves=3, gain=0.5):
    out, amp, total = 0, 1.0, 0.0
    for i in range(octaves):
        c = cells * 2 ** i
        out = out + amp * smooth_noise(rng, max(2, int(c * H / W)), c)
        total += amp
        amp *= gain
    return out / total


def render(seed, tint, lift, freq, light):
    rng = np.random.default_rng(seed)
    yy, xx = np.mgrid[0:H, 0:W].astype(np.float32)
    # Two rounds of warping bend straight bands into flowing folds.
    f = fbm(rng, 3, 3)
    wx, wy = fbm(rng, 2, 2), fbm(rng, 2, 2)
    f = map_coordinates(f, [np.clip(yy + wy * 520, 0, H - 1), np.clip(xx + wx * 520, 0, W - 1)], order=1)
    diag = (xx * 0.6 + yy * 0.8) / W
    field = f * 1.0 + diag * 1.4
    folds = np.sin(field * freq)
    folds = gaussian_filter(folds, 3)
    # Shade the folds as a lit surface.
    gy, gx = np.gradient(folds)
    shade = -gx * light[0] - gy * light[1]
    shade = shade / (np.abs(shade).max() + 1e-6)
    v = 0.5 + 0.38 * shade + 0.12 * folds
    # A soft key light from the upper middle, falling off to the edges.
    key = np.exp(-(((xx / W - 0.45) / 0.55) ** 2 + ((yy / H - 0.15) / 0.75) ** 2))
    v = np.clip(v * (0.35 + 0.75 * key) * lift, 0, 1)
    rgb = np.stack([v * tint[0], v * tint[1], v * tint[2]], -1)
    rgb += (rng.random((H, W, 1)) - 0.5) * 0.03
    from PIL import Image
    return Image.fromarray((np.clip(rgb, 0, 1) * 255).astype(np.uint8)).crop((0, 0, W, H - 110))


if __name__ == "__main__":
    out = sys.argv[1]
    render(5, (0.92, 0.93, 0.96), 1.1, 9, (1.0, 0.8)).save(f"{out}/backdrop-silver.webp", quality=84, method=6)
    render(23, (0.55, 0.6, 0.7), 0.75, 7, (0.9, 1.0)).save(f"{out}/backdrop-graphite.webp", quality=82, method=6)
    render(41, (0.32, 0.42, 0.62), 0.8, 8, (1.0, 0.6)).save(f"{out}/backdrop-blue.webp", quality=82, method=6)
    print("ok")
