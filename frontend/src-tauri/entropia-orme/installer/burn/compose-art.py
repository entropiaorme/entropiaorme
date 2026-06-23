"""Compose the WiX Burn installer's brand art from the app's design tokens.

Generates the PNGs the theme references, all in the EntropiaOrme design
language (base navy #0a0e17, cyan accent #38bdf8, slate text #e2e8f0, the ibex
emblem):

  eo-background.png  600x450  full-window backdrop (glows + dot grid + watermark)
  eo-logoside.png    185x450  full-bleed sidebar: ibex emblem + wordmark lockup
  eo-logo.png         48x48   small haloed ibex for the utility pages
  eo-mark.png         48x48   bare ibex (no halo) for the progress header
  eo-wordmark.png    132x44   wordmark lockup for the progress header
  eo-progress.png      4x16   progress bar (left/used/unused/right column strip)
  eo-field.png       280x26   read-only install-path box
  eo-btn-*.png        86x26   primary/secondary button faces (rest/hover/press)

Run from anywhere; writes next to this script. Re-run after a brand/icon change.
"""
from __future__ import annotations
import math
from pathlib import Path
import numpy as np
from PIL import Image, ImageDraw, ImageFont, ImageFilter

HERE = Path(__file__).resolve().parent
ICONS = HERE.parent.parent / "icons"
STATIC = HERE.parents[3] / "static"   # frontend/static: the bespoke brand lockups
FONTS = Path("C:/Windows/Fonts")

# --- EntropiaOrme design tokens (from frontend/src/app.css) ---
BASE = (10, 14, 23)          # #0a0e17
SURFACE = (19, 25, 38)       # #131926
RAISED = (30, 42, 58)        # #1e2a3a
ACCENT = (56, 189, 248)      # #38bdf8
ACCENT_DIM = (12, 74, 110)   # #0c4a6e  (== --color-accent-muted)
BORDER_BRIGHT = (42, 58, 78)   # #2a3a4e: the dot-grid stipple colour
TEXT = (226, 232, 240)       # #e2e8f0
TEXT_2 = (148, 163, 184)     # #94a3b8
SURFACE_HOVER = (26, 34, 53)   # #1a2235
SURFACE_PRESS = (15, 22, 38)   # #0f1626: secondary-button pressed fill, a step below SURFACE
ACCENT_HOVER = (125, 211, 252)  # #7dd3fc
ACCENT_PRESS = (14, 165, 233)   # deeper cyan


def font(name: str, size: int) -> ImageFont.FreeTypeFont:
    for cand in (name, "segoeui.ttf"):
        p = FONTS / cand
        if p.exists():
            return ImageFont.truetype(str(p), size)
    return ImageFont.load_default()


def vgrad(w: int, h: int, top: tuple, bot: tuple) -> Image.Image:
    img = Image.new("RGB", (w, h), top)
    px = img.load()
    for y in range(h):
        t = y / max(1, h - 1)
        c = tuple(round(top[i] + (bot[i] - top[i]) * t) for i in range(3))
        for x in range(w):
            px[x, y] = c
    return img


def glow(size: tuple, centre: tuple, radius: int, colour: tuple, strength: float) -> Image.Image:
    """A soft radial glow as an RGBA layer."""
    g = Image.new("RGBA", size, (0, 0, 0, 0))
    d = ImageDraw.Draw(g)
    cx, cy = centre
    d.ellipse([cx - radius, cy - radius, cx + radius, cy + radius],
              fill=colour + (int(255 * strength),))
    return g.filter(ImageFilter.GaussianBlur(radius // 2))


def sparkle(d: ImageDraw.ImageDraw, x: int, y: int, r: int, colour: tuple, a: int) -> None:
    """A four-point sparkle echoing the brand mark."""
    col = colour + (a,)
    d.polygon([(x, y - r), (x + r * 0.18, y - r * 0.18), (x + r, y),
               (x + r * 0.18, y + r * 0.18), (x, y + r),
               (x - r * 0.18, y + r * 0.18), (x - r, y),
               (x - r * 0.18, y - r * 0.18)], fill=col)


def tracked(d, xy, text, fnt, fill, track):
    """Draw uppercase text with letter-spacing; returns total width."""
    x, y = xy
    for ch in text:
        d.text((x, y), ch, font=fnt, fill=fill)
        x += d.textlength(ch, font=fnt) + track
    return x - xy[0] - track


def text_w(d, text, fnt, track=0):
    return sum(d.textlength(c, font=fnt) + track for c in text) - (track if text else 0)


def load_ibex(box: int) -> Image.Image:
    for c in ("128x128@2x.png", "128x128.png", "icon.png"):
        p = ICONS / c
        if p.exists():
            im = Image.open(p).convert("RGBA")
            return im.resize((box, box), Image.LANCZOS)
    raise SystemExit("no icon found")


def starfield(d, w, h, seed=7):
    """Deterministic faint starfield (no RNG: a hashed lattice)."""
    n = 0
    for gx in range(0, w, 17):
        for gy in range(0, h, 19):
            n += 1
            hsh = (gx * 73856093) ^ (gy * 19349663) ^ (seed * 83492791)
            if hsh % 11 == 0:
                px = gx + (hsh >> 4) % 13
                py = gy + (hsh >> 8) % 15
                a = 18 + (hsh >> 3) % 40
                d.point((px, py), fill=TEXT + (a,))


# ---------------------------------------------------------------- background
def _ellipse_field(w, h, cx, cy, rx, ry):
    """Normalised elliptical distance: 0 at the centre, 1 on the rx/ry ellipse."""
    ys, xs = np.mgrid[0:h, 0:w].astype(np.float32)
    return np.sqrt(((xs - cx) / rx) ** 2 + ((ys - cy) / ry) ** 2)


def _radial_light(w, h, cx, cy, rx, ry, colour, strength):
    """Additive coloured glow: strength*colour at the centre, linear to 0 at the edge."""
    a = np.clip(1.0 - _ellipse_field(w, h, cx, cy, rx, ry), 0.0, 1.0) * strength
    return a[..., None] * np.array(colour, np.float32)


def make_background():
    """The full-window backdrop, mirroring the app's onboarding atmosphere
    (frontend/src/routes/welcome): base navy, a vignette-masked dot grid, two
    radial glows (cyan upper-right, muted-cyan lower-left), the suited-ibex
    watermark bled off the lower-right, and a faint accent rule along the top.
    Baked to a static raster (no live CSS), which is all thmutil renders behind
    the page controls."""
    w, h = 600, 450
    img = np.tile(np.array(BASE, np.float32), (h, w, 1))

    # two radial glows (welcome .bg-glow): the CSS ellipse radii scaled by each
    # gradient's transparent-stop fraction give the additive falloff radius.
    img += _radial_light(w, h, 0.78 * w, 0.18 * h, 0.60 * 0.55 * w, 0.60 * 0.45 * h, ACCENT, 0.22)
    img += _radial_light(w, h, 0.12 * w, 0.92 * h, 0.55 * 0.60 * w, 0.55 * 0.50 * h, ACCENT_DIM, 0.70)

    # dot grid (welcome .bg-grid): a 28px stipple, vignette-masked to fade at the
    # edges, in the bright-border colour at the layer*color-mix opacity.
    grid = np.zeros((h, w), np.float32)
    grid[1::28, 1::28] = 1.0
    vmask = np.clip((0.85 - _ellipse_field(w, h, 0.50 * w, 0.45 * h, 0.70 * w, 0.80 * h)) / (0.85 - 0.30), 0.0, 1.0)
    grid *= vmask * (0.55 * 0.65)
    img += grid[..., None] * np.array(BORDER_BRIGHT, np.float32)

    # faint accent rule across the very top (welcome .top-rule)
    xs = np.arange(w, dtype=np.float32)
    rule = (1.0 - np.abs(xs - w / 2.0) / (w / 2.0)) * (0.55 * 0.55)
    img[0] += rule[:, None] * np.array(ACCENT, np.float32)

    bg = Image.fromarray(np.clip(img, 0, 255).astype(np.uint8), "RGB").convert("RGBA")

    # the suited-ibex watermark, mirrored and bled off the lower-right corner
    # (welcome .bg-mascot: right:-7% bottom:-10%, scaleX(-1), near-transparent),
    # on a soft cyan halo standing in for its CSS drop-shadow.
    mark = Image.open(STATIC / "watermark.png").convert("RGBA").transpose(Image.FLIP_LEFT_RIGHT)
    mw = 320
    mh = round(mw * mark.height / mark.width)
    mark = mark.resize((mw, mh), Image.LANCZOS)
    left, top = round(1.07 * w - mw), round(1.10 * h - mh)
    bg.alpha_composite(glow((w, h), (left + mw // 2, top + mh // 2), 150, ACCENT, 0.10))
    mark.putalpha(mark.getchannel("A").point(lambda a: int(a * 0.10)))
    bg.alpha_composite(mark, (left, top))

    bg.save(HERE / "eo-background.png")  # keep RGBA: thmutil's loader requires 32-bit


# ---------------------------------------------------------------- sidebar
def make_sidebar():
    w, h = 185, 450
    img = vgrad(w, h, SURFACE, BASE).convert("RGBA")
    # cosmic glow behind the emblem + faint starfield
    img.alpha_composite(glow((w, h), (w // 2, 150), 150, ACCENT, 0.14))
    ov = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    sd = ImageDraw.Draw(ov)
    starfield(sd, w, h, seed=23)
    img.alpha_composite(ov)
    # ibex emblem, centred upper third, on a tight cyan halo so the dark badge
    # reads as a glowing emblem rather than a dark disc punched into the backdrop
    ex, ey = w // 2, 78 + 58
    img.alpha_composite(glow((w, h), (ex, ey), 72, ACCENT, 0.38))
    ibex = load_ibex(116)
    img.alpha_composite(ibex, ((w - 116) // 2, 78))
    d = ImageDraw.Draw(img)
    # bespoke wordmark lockup: the app's two-tone wordmark-on-dark PNG, scaled to
    # the sidebar width with side margins (replaces the former Segoe-bold draw).
    wm = Image.open(STATIC / "wordmark-on-dark.png").convert("RGBA")
    ww = 150
    wh = round(ww * wm.height / wm.width)
    wm = wm.resize((ww, wh), Image.LANCZOS)
    wy = 210
    img.alpha_composite(wm, ((w - ww) // 2, wy))
    # right-edge cyan accent rule (divider from content)
    d.line([(w - 1, 0), (w - 1, h)], fill=ACCENT + (255,), width=2)
    img.save(HERE / "eo-logoside.png")  # keep RGBA: thmutil's loader requires 32-bit


# ---------------------------------------------------------------- small logo
def make_logo():
    # The lone mark on the utility pages (e.g. Progress). A soft cyan halo gives
    # the dark badge a luminous edge so it reads against the dark window.
    box = 48
    inner = 38
    img = Image.new("RGBA", (box, box), (0, 0, 0, 0))
    img.alpha_composite(glow((box, box), (box // 2, box // 2), box // 2, ACCENT, 0.6))
    img.alpha_composite(load_ibex(inner), ((box - inner) // 2, (box - inner) // 2))
    img.save(HERE / "eo-logo.png")


# ----------------------------------------------- bare mark + wordmark (progress)
def make_mark():
    # The emblem on its own (no halo, no added backdrop) for the progress header.
    box = 48
    img = Image.new("RGBA", (box, box), (0, 0, 0, 0))
    img.alpha_composite(load_ibex(box), (0, 0))
    img.save(HERE / "eo-mark.png")


def make_wordmark():
    # The bespoke wordmark lockup sized for the progress header, beside the mark.
    # 132x44 preserves the source 3:1 aspect, so the ImageControl maps it 1:1.
    wm = Image.open(STATIC / "wordmark-on-dark.png").convert("RGBA").resize((132, 44), Image.LANCZOS)
    wm.save(HERE / "eo-wordmark.png")


# ---------------------------------------------------------------- buttons
def _btn(fill, outline, name):
    """A flat, rounded button face (text is drawn over it by thmutil)."""
    w, h, r = 86, 26, 5
    img = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.rounded_rectangle([0, 0, w - 1, h - 1], radius=r, fill=fill,
                        outline=outline, width=1 if outline else 0)
    img.save(HERE / name)


def make_buttons():
    # Primary (cyan fill): the affirmative action.
    _btn(ACCENT, None, "eo-btn-pri.png")
    _btn(ACCENT_HOVER, None, "eo-btn-pri-hover.png")
    _btn(ACCENT_PRESS, None, "eo-btn-pri-press.png")
    # Secondary (dark surface + cyan hairline): neutral actions. Rests at
    # SURFACE and brightens on hover (matching the app's secondary button),
    # then settles just below SURFACE on press (never down to black).
    _btn(SURFACE, ACCENT_DIM, "eo-btn-sec.png")
    _btn(SURFACE_HOVER, ACCENT, "eo-btn-sec-hover.png")
    _btn(SURFACE_PRESS, ACCENT_DIM, "eo-btn-sec-press.png")


# ---------------------------------------------------------------- progress bar
def make_progress():
    # WiX 6 DrawProgressBar samples a 4px-wide, full-height strip as four 1px
    # columns at srcX 0/1/2/3: left edge, used fill, unused fill, right edge.
    img = Image.new("RGBA", (4, 16), (0, 0, 0, 0))
    px = img.load()
    cols = [ACCENT, ACCENT, RAISED, RAISED]  # left, used, unused, right
    for x, c in enumerate(cols):
        for y in range(16):
            px[x, y] = c + (255,)
    img.save(HERE / "eo-progress.png")


# ---------------------------------------------------------------- path field
def make_field():
    # A read-only path field: a bordered box in the dark theme. Filled with BASE so
    # the path Label (FontId 0, BASE text background) sits flush; the border-bright
    # outline delineates the box.
    w, h, r = 280, 26, 4
    img = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.rounded_rectangle([0, 0, w - 1, h - 1], radius=r, fill=BASE, outline=(42, 58, 78), width=1)
    img.save(HERE / "eo-field.png")


if __name__ == "__main__":
    make_background()
    make_sidebar()
    make_logo()
    make_mark()
    make_wordmark()
    make_buttons()
    make_progress()
    make_field()
    print("composed art + buttons")
