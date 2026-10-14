#!/usr/bin/env python3
"""Build every Tabler logo and app-icon file from one source of truth: the cell grid below.

    python3 design/logo/build.py            # from the repo root

Writes the SVG masters to design/logo/ and design/app-icon.svg, and renders the app icons
(src-tauri/icons/*, src-tauri/app-icon.png, public/favicon.png). Every icon size is drawn on its
own pixel grid (whole-pixel cells and gaps), so 16 and 32 px stay sharp instead of being
downsampled from 1024.

Needs: Python 3.9+, fontTools and Pillow (pip install fonttools pillow), the Inter Tight
variable font (SIL OFL 1.1) in ~/Library/Fonts, Google Chrome (rendering) and macOS iconutil.
"""
import math
import os
import shutil
import subprocess
import sys
import tempfile
import time

from fontTools.pens.boundsPen import BoundsPen
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont
from PIL import Image

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
LOGO_DIR = os.path.join(ROOT, "design", "logo")

# --------------------------------------------------------------------------- the mark
# An elephant drawn with table cells, facing right.
#   X = body cell   E = ear cell (the "selected" cells)   o = eye cell (a cell with a hole)
GRID = [
    ".....XX.",
    "....EEXX",
    ".XXXEEoX",
    "XXXXEEXX",
    "XXXXX..X",
    "XX.XX..X",
    "XX.XX.XX",
]
COLS, ROWS = len(GRID[0]), len(GRID)
BACK_ROW = 2            # the elephant's back; the wordmark's cap height runs from here to the feet

# Palette
BLUE = "#2563EB"        # Tabler Blue: body cells
SELECT = "#8FB3F9"      # Selection Blue: ear cells
INK = "#0F1A3A"         # Ink: wordmark
BLUE_DARK_BG = "#3B82F6"    # body cells on dark backgrounds
SELECT_DARK_BG = "#A5C4FF"  # ear cells on dark backgrounds
TILE = ("#FFFFFF", "#E8EEFB")  # app-icon tile, top to bottom

# Proportions (master: 24-unit cells, 6-unit gaps, 5-unit corner radius)
GAP_RATIO = 0.25
RADIUS_RATIO = 0.21
EYE_RATIO = 0.23


def f(n):
    s = f"{n:.2f}".rstrip("0").rstrip(".")
    return "0" if s in ("-0", "") else s


def rect(x, y, w, h, r):
    """Clockwise (rounded) rectangle."""
    if r <= 0:
        return f"M{f(x)} {f(y)}H{f(x + w)}V{f(y + h)}H{f(x)}Z"
    return (f"M{f(x + r)} {f(y)}H{f(x + w - r)}A{f(r)} {f(r)} 0 0 1 {f(x + w)} {f(y + r)}"
            f"V{f(y + h - r)}A{f(r)} {f(r)} 0 0 1 {f(x + w - r)} {f(y + h)}"
            f"H{f(x + r)}A{f(r)} {f(r)} 0 0 1 {f(x)} {f(y + h - r)}"
            f"V{f(y + r)}A{f(r)} {f(r)} 0 0 1 {f(x + r)} {f(y)}Z")


def hole(cx, cy, r):
    """Counter-clockwise circle, which cuts a hole under the nonzero fill rule."""
    return (f"M{f(cx - r)} {f(cy)}A{f(r)} {f(r)} 0 1 0 {f(cx + r)} {f(cy)}"
            f"A{f(r)} {f(r)} 0 1 0 {f(cx - r)} {f(cy)}Z")


def grid_size(cell, gap):
    return COLS * (cell + gap) - gap, ROWS * (cell + gap) - gap


def square_hole(x, y, w, h):
    """Counter-clockwise rectangle (a hole under the nonzero fill rule)."""
    return f"M{f(x)} {f(y)}V{f(y + h)}H{f(x + w)}V{f(y)}Z"


def mark(x0, y0, cell, gap=None, radius=None, body=BLUE, ear=SELECT, eye="hole"):
    """The two <path>s of the mark. eye: 'hole' (round hole in the cell), 'square' (whole-pixel square hole,
    for 3-5 px cells) or 'gap' (the cell is left out, for 1-2 px cells where no hole fits)."""
    gap = cell * GAP_RATIO if gap is None else gap
    radius = cell * RADIUS_RATIO if radius is None else radius
    pitch = cell + gap
    paths = {"X": [], "E": []}
    for r, row in enumerate(GRID):
        for c, ch in enumerate(row):
            if ch == "." or (ch == "o" and eye == "gap"):
                continue
            x, y = x0 + c * pitch, y0 + r * pitch
            d = rect(x, y, cell, cell, radius)
            if ch == "o" and eye == "square":
                hs = max(1, round(cell * 0.4))
                off = (cell - hs) // 2
                d += square_hole(x + off, y + off, hs, hs)
            elif ch == "o":
                d += hole(x + cell / 2, y + cell / 2, cell * EYE_RATIO)
            paths["E" if ch == "E" else "X"].append(d)
    return (f'<path fill="{body}" d="{"".join(paths["X"])}"/>\n'
            f'<path fill="{ear}" d="{"".join(paths["E"])}"/>')


# --------------------------------------------------------------------------- the wordmark
# "Tabler" in Inter Tight Bold (SIL OFL 1.1), outlined; GPOS kerning plus a little display tracking.
FONT = os.path.expanduser("~/Library/Fonts/InterTight-VariableFont_wght.ttf")
WEIGHT = 700
WORD = "Tabler"
TRACKING = -20          # font units added between letters
OPTICAL = {}            # extra per-pair corrections, e.g. {("T", "a"): -10}
_font = None


def _load_font():
    global _font
    if _font is None:
        if not os.path.exists(FONT):
            sys.exit(f"missing font: {FONT} (Inter Tight variable, https://fonts.google.com/specimen/Inter+Tight)")
        _font = instantiateVariableFont(TTFont(FONT), {"wght": WEIGHT})
    return _font


def _kern(fnt, left, right):
    """GPOS pair adjustment (formats 1 and 2) for two glyph names, in font units."""
    for lookup in fnt["GPOS"].table.LookupList.Lookup:
        subs = lookup.SubTable
        if lookup.LookupType == 9:
            subs = [s.ExtSubTable for s in subs]
        elif lookup.LookupType != 2:
            continue
        for st in subs:
            if getattr(st, "LookupType", 2) != 2 or left not in st.Coverage.glyphs:
                continue
            if st.Format == 1:
                for rec in st.PairSet[st.Coverage.glyphs.index(left)].PairValueRecord:
                    if rec.SecondGlyph == right and rec.Value1 is not None:
                        return getattr(rec.Value1, "XAdvance", 0) or 0
            elif st.Format == 2:
                rec = st.Class1Record[st.ClassDef1.classDefs.get(left, 0)] \
                    .Class2Record[st.ClassDef2.classDefs.get(right, 0)]
                v = getattr(rec.Value1, "XAdvance", 0) if rec.Value1 is not None else 0
                if v:
                    return v
    return 0


def wordmark(x, baseline, cap_px):
    """Outlined wordmark whose ink starts at x and whose cap height is cap_px.
    Returns (path data, ink bounds (xmin, ymin, xmax, ymax))."""
    fnt = _load_font()
    s = cap_px / fnt["OS/2"].sCapHeight
    cmap, gs, hmtx = fnt.getBestCmap(), fnt.getGlyphSet(), fnt["hmtx"]
    names = [cmap[ord(ch)] for ch in WORD]
    lsb = hmtx[names[0]][1] * s

    def draw(pen, origin):
        cursor = 0.0
        for i, g in enumerate(names):
            gs[g].draw(TransformPen(pen, (s, 0, 0, -s, origin + cursor * s, baseline)))
            if i + 1 < len(names):
                cursor += hmtx[g][0] + _kern(fnt, g, names[i + 1]) + TRACKING
                cursor += OPTICAL.get((WORD[i], WORD[i + 1]), 0)

    pen, bounds = SVGPathPen(gs, ntos=f), BoundsPen(gs)
    draw(pen, x - lsb)
    draw(bounds, x - lsb)
    return pen.getCommands(), bounds.bounds


# --------------------------------------------------------------------------- SVG files
def svg(title, w, h, body, extra=""):
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {f(w)} {f(h)}" width="{f(w)}" '
            f'height="{f(h)}"{extra} role="img" aria-labelledby="title">\n<title id="title">{title}</title>\n'
            f"{body}\n</svg>\n")


def write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as fh:
        fh.write(text)
    print("wrote", os.path.relpath(path, ROOT))


SCHEMES = {
    # name: (body, ear, word)
    "": (BLUE, SELECT, INK),
    "-dark": (BLUE_DARK_BG, SELECT_DARK_BG, "#FFFFFF"),
    "-black": ("#000000", "#000000", "#000000"),
    "-white": ("#FFFFFF", "#FFFFFF", "#FFFFFF"),
}
M_CELL, M_GAP, M_RADIUS = 24, 6, 5      # master proportions on the 256 grid
MARGIN = 26


def build_masters():
    w, h = grid_size(M_CELL, M_GAP)                     # 234 x 204
    pitch = M_CELL + M_GAP
    for suffix, (body, ear, word) in SCHEMES.items():
        # symbol on a 256 square
        x0, y0 = (256 - w) / 2, (256 - h) / 2
        write(os.path.join(LOGO_DIR, f"tabler-symbol{suffix}.svg"),
              svg("Tabler", 256, 256, mark(x0, y0, M_CELL, M_GAP, M_RADIUS, body, ear)))

        # small-size cut: one unit per cell, no gaps; sharp at whole multiples of 16 px (16, 32, 48),
        # where the half-unit vertical offset still lands on whole pixels. The eye is a missing cell.
        write(os.path.join(LOGO_DIR, f"tabler-symbol-small{suffix}.svg"),
              svg("Tabler", 8, 8, mark(0, 0.5, 1, 0, 0, body, ear, eye="gap"), ' shape-rendering="crispEdges"'))

        # horizontal lockup: the elephant stands on the baseline, the cap height is its back
        cap = (ROWS - BACK_ROW) * pitch - M_GAP          # 144
        baseline = MARGIN + h
        tx = MARGIN + w + 1.5 * pitch
        d, (_, _, xmax, _) = wordmark(tx, baseline, cap)
        lw = math.ceil(xmax + MARGIN)
        body_svg = mark(MARGIN, MARGIN, M_CELL, M_GAP, M_RADIUS, body, ear) + f'\n<path fill="{word}" d="{d}"/>'
        write(os.path.join(LOGO_DIR, f"tabler-logo-horizontal{suffix}.svg"), svg("Tabler", lw, 256, body_svg))

        # stacked lockup
        scap = 96
        d0, (sx0, _, sx1, _) = wordmark(0, 0, scap)
        tw = sx1 - sx0
        sw = math.ceil(max(w, tw) + 2 * MARGIN)
        sh = MARGIN + h + pitch + scap + MARGIN
        d, _ = wordmark((sw - tw) / 2, MARGIN + h + pitch + scap, scap)
        body_svg = mark((sw - w) / 2, MARGIN, M_CELL, M_GAP, M_RADIUS, body, ear) + f'\n<path fill="{word}" d="{d}"/>'
        write(os.path.join(LOGO_DIR, f"tabler-logo-stacked{suffix}.svg"), svg("Tabler", sw, sh, body_svg))


# --------------------------------------------------------------------------- app icon
def icon_cells(n):
    """Largest whole-pixel (cell, gap) whose elephant fits ~65 % of the tile at an n-px icon."""
    limit = 0.65 * n * 824 / 1024
    best = (1, 0)
    for cell in range(1, 200):
        options = [0] if cell <= 2 else ([0, 1] if cell == 3 else [max(1, round(cell * GAP_RATIO))])
        for gap in options:
            w, _ = grid_size(cell, gap)
            if w <= limit and (cell, gap) > best:
                best = (cell, gap)
    return best


def app_icon_svg(n, vector=False):
    """macOS-style icon on an n-px canvas: the 824/1024 tile with 100/1024 margins (Big Sur grid).
    vector=True keeps fractional master proportions (for the 1024 master file)."""
    k = n / 1024
    t0, ts, tr = 100 * k, 824 * k, 185 * k
    if vector:
        cell, gap = 54 * k, 14 * k
    else:
        cell, gap = icon_cells(n)
    w, h = grid_size(cell, gap)
    x0, y0 = (n - w) / 2, (n - h) / 2 - 0.012 * n
    if not vector:
        x0, y0 = round(x0), round(y0)
    radius = cell * RADIUS_RATIO if cell >= 6 else 0
    eye = "hole" if cell >= 6 else ("square" if cell >= 3 else "gap")
    # ids carry the size: all sizes share one HTML page when rendered, and ids must not collide
    defs = (f'<linearGradient id="tile-{n}" x1="0" y1="{f(t0)}" x2="0" y2="{f(t0 + ts)}" gradientUnits="userSpaceOnUse">'
            f'<stop offset="0" stop-color="{TILE[0]}"/><stop offset="1" stop-color="{TILE[1]}"/></linearGradient>')
    shadow = ""
    if n >= 128:
        defs += (f'<filter id="shadow-{n}" x="-20%" y="-20%" width="140%" height="140%">'
                 f'<feDropShadow dx="0" dy="{f(10 * k)}" stdDeviation="{f(12 * k)}" flood-color="{INK}" '
                 f'flood-opacity="0.30"/></filter>')
        shadow = f' filter="url(#shadow-{n})"'
    border_w = max(1.0, 2 * k) if n >= 32 else 0.75
    body = (f"<defs>{defs}</defs>\n"
            f'<rect x="{f(t0)}" y="{f(t0)}" width="{f(ts)}" height="{f(ts)}" rx="{f(tr)}" fill="url(#tile-{n})"{shadow}/>\n'
            f'<rect x="{f(t0 + border_w / 2)}" y="{f(t0 + border_w / 2)}" width="{f(ts - border_w)}" '
            f'height="{f(ts - border_w)}" rx="{f(tr - border_w / 2)}" fill="none" stroke="{INK}" '
            f'stroke-opacity="0.10" stroke-width="{f(border_w)}"/>\n'
            + mark(x0, y0, cell, gap, radius, BLUE, SELECT, eye))
    return svg("Tabler", n, n, body)


def find_chrome():
    for c in ("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
              "/Applications/Chromium.app/Contents/MacOS/Chromium",
              shutil.which("google-chrome") or "", shutil.which("chromium") or ""):
        if c and os.path.exists(c):
            return c
    sys.exit("Google Chrome or Chromium is needed to render the PNGs")


def render_sheet(sizes):
    """Render every icon size in one headless-Chrome screenshot (1 CSS px = 1 device px) and crop."""
    pad = 8
    width = sum(sizes) + pad * (len(sizes) + 1)
    height = max(sizes) + 2 * pad
    x, items, html = pad, {}, []
    for n in sizes:
        html.append(f'<div style="position:absolute;left:{x}px;top:{pad}px;width:{n}px;height:{n}px">'
                    f'{app_icon_svg(n, vector=(n >= 512))}</div>')
        items[n] = (x, pad)
        x += n + pad
    tmp = tempfile.mkdtemp(prefix="tabler-icons-")
    page = os.path.join(tmp, "sheet.html")
    shot = os.path.join(tmp, "sheet.png")
    with open(page, "w") as fh:
        fh.write('<!doctype html><html><head><meta charset="utf-8"><style>html,body{margin:0;background:transparent}'
                 'svg{display:block}</style></head><body>' + "".join(html) + "</body></html>")
    cmd = [find_chrome(), "--headless=new", "--disable-gpu", "--hide-scrollbars", "--no-first-run",
           "--no-default-browser-check", f"--user-data-dir={os.path.join(tmp, 'profile')}",
           "--force-device-scale-factor=1", "--default-background-color=00000000",
           f"--window-size={width},{height}", f"--screenshot={shot}", "file://" + page]
    proc = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    deadline = time.time() + 60
    while time.time() < deadline and not (os.path.exists(shot) and proc.poll() is not None):
        time.sleep(0.2)
    if proc.poll() is None:
        proc.kill()
    sheet = Image.open(shot).convert("RGBA")
    out = {n: sheet.crop((x, y, x + n, y + n)) for n, (x, y) in items.items()}
    shutil.rmtree(tmp, ignore_errors=True)
    return out


def build_icons():
    write(os.path.join(ROOT, "design", "app-icon.svg"), app_icon_svg(1024, vector=True))
    sizes = [16, 24, 30, 32, 44, 48, 50, 64, 71, 89, 107, 128, 142, 150, 256, 284, 310, 512, 1024]
    png = render_sheet(sizes)
    icons = os.path.join(ROOT, "src-tauri", "icons")

    def save(img, path):
        os.makedirs(os.path.dirname(path), exist_ok=True)
        img.save(path, optimize=True)
        print("wrote", os.path.relpath(path, ROOT))

    save(png[1024], os.path.join(ROOT, "src-tauri", "app-icon.png"))
    save(png[32], os.path.join(ROOT, "public", "favicon.png"))
    for n, name in ((32, "32x32.png"), (64, "64x64.png"), (128, "128x128.png"), (256, "128x128@2x.png"),
                    (512, "icon.png"), (50, "StoreLogo.png")):
        save(png[n], os.path.join(icons, name))
    for n in (30, 44, 71, 89, 107, 142, 150, 284, 310):
        save(png[n], os.path.join(icons, f"Square{n}x{n}Logo.png"))

    # Windows .ico (largest first so Pillow keeps every size from its own drawing)
    ico_sizes = [256, 64, 48, 32, 24, 16]
    ico = os.path.join(icons, "icon.ico")
    os.makedirs(icons, exist_ok=True)
    png[256].save(ico, format="ICO", sizes=[(n, n) for n in ico_sizes],
                  append_images=[png[n] for n in ico_sizes[1:]])
    print("wrote", os.path.relpath(ico, ROOT))

    # macOS .icns via iconutil
    tmp = tempfile.mkdtemp(prefix="tabler-iconset-")
    iconset = os.path.join(tmp, "icon.iconset")
    os.makedirs(iconset)
    for pt in (16, 32, 128, 256, 512):
        png[pt].save(os.path.join(iconset, f"icon_{pt}x{pt}.png"))
        png[pt * 2].save(os.path.join(iconset, f"icon_{pt}x{pt}@2x.png"))
    icns = os.path.join(icons, "icon.icns")
    subprocess.run(["iconutil", "-c", "icns", iconset, "-o", icns], check=True)
    shutil.rmtree(tmp, ignore_errors=True)
    print("wrote", os.path.relpath(icns, ROOT))


if __name__ == "__main__":
    if len(sys.argv) > 1:
        ROOT = os.path.abspath(sys.argv[1])
        LOGO_DIR = os.path.join(ROOT, "design", "logo")
    build_masters()
    build_icons()
