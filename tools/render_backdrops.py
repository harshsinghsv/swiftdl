"""Renders the dark 'smoke' backdrops behind the site's app mockups.

Mostly black, with soft grey ribbons of smoke curling through it. Smooth
noise is warped and swirled, then light is drawn where a sine of it peaks,
at two widths (a broad soft glow and a brighter edge), and masked so only
some areas catch the light. The film grain is added by the site's CSS, which
keeps these files small.

    python tools/render_backdrops.py site/assets/bg
"""
import sys
import numpy as np
from PIL import Image
from scipy.ndimage import gaussian_filter, map_coordinates, zoom

W, H = 2400, 1360


def smooth_noise(rng, cy, cx, sigma=1.0):
    g = gaussian_filter(rng.standard_normal((cy + 4, cx + 4)), sigma)
    return zoom(g, (H / cy, W / cx), order=3)[:H, :W]


def fbm(rng, cells, octaves=3, gain=0.5):
    out, amp, total = 0, 1.0, 0.0
    for i in range(octaves):
        c = cells * 2 ** i
        out = out + amp * smooth_noise(rng, max(2, int(c * H / W)), c)
        total += amp
        amp *= gain
    return out / total


def smoke(rng, curls, freq):
    """One layer of smoke ribbons, 0..1."""
    yy, xx = np.mgrid[0:H, 0:W].astype(np.float32)
    # Swirl around a few centres so the bands curl like smoke.
    ax, ay = xx.copy(), yy.copy()
    for cx, cy, k, r in curls:
        dx, dy = ax - cx * W, ay - cy * H
        d2 = (dx * dx + dy * dy) / (r * W) ** 2
        a = k * np.exp(-d2)
        ca, sa = np.cos(a), np.sin(a)
        ax, ay = cx * W + dx * ca - dy * sa, cy * H + dx * sa + dy * ca
    wx, wy = fbm(rng, 2, 3), fbm(rng, 2, 3)
    ax = np.clip(ax + wx * 300, 0, W - 1)
    ay = np.clip(ay + wy * 300, 0, H - 1)
    f = map_coordinates(fbm(rng, 3, 3), [ay, ax], order=1) + (ax * 0.55 + ay * 0.35) / W * 2.2
    s = np.sin(f * freq)
    glow = np.exp(-(1 - s) * 1.3)   # the ribbon
    edge = np.exp(-(1 - s) * 7.0)   # its brighter edge
    # Only some regions catch the light, like smoke in a beam.
    mask = np.clip(fbm(rng, 2, 2) * 1.6 + 0.5, 0.12, 1)
    return (0.55 * glow + 0.45 * edge) * mask


def render(seed, tint, curls_a, curls_b, brightness):
    rng = np.random.default_rng(seed)
    v = np.maximum(smoke(rng, curls_a, 19.0), 0.75 * smoke(rng, curls_b, 13.0))
    v = gaussian_filter(v, 3)
    v = np.clip(v * brightness * 0.85, 0, 1) ** 1.7
    rgb = np.stack([v * t for t in tint], -1) + 0.025
    return Image.fromarray((np.clip(rgb, 0, 1) * 255).astype(np.uint8))


if __name__ == "__main__":
    out = sys.argv[1]
    silver = (0.88, 0.89, 0.92)
    render(7, silver, [(0.35, 0.45, 2.2, 0.35), (0.72, 0.55, -1.8, 0.3)], [(0.6, 0.3, -1.6, 0.4)], 1.0).save(f"{out}/backdrop-silver.webp", quality=84, method=6)
    render(19, (0.72, 0.74, 0.78), [(0.6, 0.4, -2.4, 0.4)], [(0.25, 0.7, 1.8, 0.35)], 0.8).save(f"{out}/backdrop-graphite.webp", quality=84, method=6)
    render(31, (0.45, 0.58, 0.85), [(0.4, 0.6, 2.0, 0.38), (0.8, 0.3, 1.5, 0.25)], [(0.6, 0.5, -2.0, 0.4)], 0.95).save(f"{out}/backdrop-blue.webp", quality=84, method=6)
    print("ok")
