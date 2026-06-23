"""Compose the WiX Burn installer's brand art from the app's design tokens.

Generates three PNGs the theme references, all in the EntropiaOrme design
language (base navy #0a0e17, cyan accent #38bdf8, slate text #e2e8f0, the ibex
emblem, four-point sparkles):

  background.png  600x450  full-window dark backdrop (gradient + glow + sparkles)
  logoside.png    185x450  full-bleed sidebar: ibex, wordmark, tagline
  logo.png         48x48   small ibex mark for the non-sidebar pages
  progress.png      4x16   progress bar (left/used/unused/right column strip)
  field.png       280x26   read-only install-path box

Run from anywhere; writes next to this script. Re-run after a brand/icon change.
"""
from __future__ import annotations
import math
from pathlib import Path
from PIL import Image, ImageDraw, ImageFont, ImageFilter

HERE = Path(__file__).resolve().parent
ICONS = HERE.parent.parent / "icons"
FONTS = Path("C:/Windows/Fonts")

# --- EntropiaOrme design tokens (from frontend/src/app.css) ---
BASE = (10, 14, 23)          # #0a0e17
SURFACE = (19, 25, 38)       # #131926
RAISED = (30, 42, 58)        # #1e2a3a
ACCENT = (56, 189, 248)      # #38bdf8
ACCENT_DIM = (12, 74, 110)   # #0c4a6e
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
def make_background():
    w, h = 600, 450
    img = vgrad(w, h, BASE, (13, 19, 32)).convert("RGBA")
    img.alpha_composite(glow((w, h), (90, 70), 260, ACCENT, 0.10))
    img.alpha_composite(glow((w, h), (560, 430), 220, ACCENT_DIM, 0.16))
    ov = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    starfield(ImageDraw.Draw(ov), w, h)
    img.alpha_composite(ov)
    img.save(HERE / "eo-background.png")  # keep RGBA: thmutil's loader requires 32-bit


# ---------------------------------------------------------------- sidebar
def make_sidebar():
    w, h = 185, 450
    img = vgrad(w, h, SURFACE, BASE).convert("RGBA")
    # cosmic glow behind the emblem + faint starfield
    img.alpha_composite(glow((w, h), (w // 2, 150), 150, ACCENT, 0.14))
    ov = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    sd = ImageDraw.Draw(ov)
    starfield(sd, w, h, seed=23)
    sparkle(sd, 38, 96, 8, ACCENT, 200)
    sparkle(sd, 150, 250, 6, ACCENT, 150)
    sparkle(sd, 30, 300, 5, TEXT, 120)
    img.alpha_composite(ov)
    # ibex emblem, centred upper third, on a tight cyan halo so the dark badge
    # reads as a glowing emblem rather than a dark disc punched into the backdrop
    ex, ey = w // 2, 78 + 58
    img.alpha_composite(glow((w, h), (ex, ey), 72, ACCENT, 0.38))
    ibex = load_ibex(116)
    img.alpha_composite(ibex, ((w - 116) // 2, 78))
    d = ImageDraw.Draw(img)
    # wordmark
    wm = font("segoeuib.ttf", 26)
    tw = d.textlength("EntropiaOrme", font=wm)
    d.text(((w - tw) / 2, 214), "EntropiaOrme", font=wm, fill=TEXT)
    # tracked tagline
    tag = font("segoeui.ttf", 11)
    label = "GAMEPLAY ANALYTICS"
    lw = text_w(d, label, tag, track=2)
    tracked(d, ((w - lw) / 2, 250), label, tag, ACCENT, track=2)
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
    make_buttons()
    make_progress()
    make_field()
    print("composed art + buttons")
