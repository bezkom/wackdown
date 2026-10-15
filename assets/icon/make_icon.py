"""Generates the WackDown icons: Ɯ (U+019C, the turned M) next to Markdown's down arrow.

    python assets/icon/make_icon.py

The letter is the Ɯ glyph of the embedded Iosevka Aile Bold (OFL-licensed): the same character the
docs display, in the family's wider sans member, which stays legible down to 16 px. Its outline is read from the font file with a tiny TrueType reader below
(no fontTools needed).

Writes wackdown.ico (app), document.ico (.md files), 256px PNG previews and an SVG master next
to this script. Every icon size is drawn separately from vectors. Requires Pillow.
"""
import math
import struct
from pathlib import Path

from PIL import Image, ImageDraw

OUT = Path(__file__).resolve().parent
FONT = OUT.parent / "fonts" / "IosevkaAile-Bold.ttf"

# Default theme (Cyberpunk Terminal) palette.
TILE = (14, 16, 24, 255)
TILE_EDGE = (255, 180, 0, 110)
AMBER = (255, 180, 0, 255)
CYAN = (0, 240, 255, 255)
PAGE = (20, 23, 34, 255)
PAGE_FOLD = (44, 48, 66, 255)
PAGE_EDGE = (255, 180, 0, 190)

SS = 8  # supersampling factor


# --- Minimal TrueType outline reader -------------------------------------------------------------

def glyph_contours(font_path: Path, char: str, steps: int = 10) -> list[list[tuple[float, float]]]:
    """Flattened contours of a simple glyph, in font units (y up)."""
    data = font_path.read_bytes()
    num_tables = struct.unpack_from(">H", data, 4)[0]
    tables = {}
    for i in range(num_tables):
        tag, _, offset, length = struct.unpack_from(">4sIII", data, 12 + 16 * i)
        tables[tag.decode("latin-1")] = (offset, length)

    loc_format = struct.unpack_from(">h", data, tables["head"][0] + 50)[0]
    glyph_id = _cmap_lookup(data, tables["cmap"][0], ord(char))
    loca = tables["loca"][0]
    if loc_format == 0:
        start, end = (2 * v for v in struct.unpack_from(">HH", data, loca + 2 * glyph_id))
    else:
        start, end = struct.unpack_from(">II", data, loca + 4 * glyph_id)
    g = tables["glyf"][0] + start
    if end == start:
        return []
    n_contours = struct.unpack_from(">h", data, g)[0]
    if n_contours < 0:
        raise ValueError(f"{char!r} is a composite glyph; not supported")
    p = g + 10
    end_pts = struct.unpack_from(f">{n_contours}H", data, p)
    p += 2 * n_contours
    p += 2 + struct.unpack_from(">H", data, p)[0]  # skip instructions
    n_points = end_pts[-1] + 1

    flags = []
    while len(flags) < n_points:
        f = data[p]
        p += 1
        flags.append(f)
        if f & 0x08:
            flags.extend([f] * data[p])
            p += 1

    def coords(short_bit, same_bit):
        nonlocal p
        out, v = [], 0
        for f in flags:
            if f & short_bit:
                d = data[p]
                p += 1
                v += d if f & same_bit else -d
            elif not f & same_bit:
                v += struct.unpack_from(">h", data, p)[0]
                p += 2
            out.append(v)
        return out

    xs = coords(0x02, 0x10)
    ys = coords(0x04, 0x20)

    contours, first = [], 0
    for last in end_pts:
        pts = [(xs[i], ys[i], bool(flags[i] & 0x01)) for i in range(first, last + 1)]
        first = last + 1
        contours.append(_flatten(pts, steps))
    return contours


def _cmap_lookup(data: bytes, cmap: int, code: int) -> int:
    n = struct.unpack_from(">H", data, cmap + 2)[0]
    for i in range(n):
        platform, encoding, offset = struct.unpack_from(">HHI", data, cmap + 4 + 8 * i)
        sub = cmap + offset
        if platform == 3 and encoding == 1 and struct.unpack_from(">H", data, sub)[0] == 4:
            seg_x2 = struct.unpack_from(">H", data, sub + 6)[0]
            ends = sub + 14
            starts = ends + seg_x2 + 2
            deltas = starts + seg_x2
            ranges = deltas + seg_x2
            for s in range(seg_x2 // 2):
                end_code = struct.unpack_from(">H", data, ends + 2 * s)[0]
                if code > end_code:
                    continue
                start_code = struct.unpack_from(">H", data, starts + 2 * s)[0]
                if code < start_code:
                    return 0
                delta = struct.unpack_from(">h", data, deltas + 2 * s)[0]
                range_off = struct.unpack_from(">H", data, ranges + 2 * s)[0]
                if range_off == 0:
                    return (code + delta) & 0xFFFF
                gid = struct.unpack_from(">H", data, ranges + 2 * s + range_off + 2 * (code - start_code))[0]
                return (gid + delta) & 0xFFFF if gid else 0
    raise ValueError("no Unicode BMP cmap")


def _flatten(pts, steps):
    """Quadratic TrueType contour (with implied on-curve midpoints) to a polygon."""
    n = len(pts)
    start = next((i for i, q in enumerate(pts) if q[2]), None)
    if start is None:  # all off-curve: start at the midpoint of the first two
        a, b = pts[0], pts[1]
        pts = [((a[0] + b[0]) / 2, (a[1] + b[1]) / 2, True)] + pts
        start, n = 0, n + 1
    seq = pts[start:] + pts[:start]
    out = [(seq[0][0], seq[0][1])]
    prev = seq[0]
    i = 1
    while i <= n:
        cur = seq[i % n]
        if cur[2]:
            out.append((cur[0], cur[1]))
            prev = cur
            i += 1
            continue
        nxt = seq[(i + 1) % n]
        end = nxt if nxt[2] else ((cur[0] + nxt[0]) / 2, (cur[1] + nxt[1]) / 2, True)
        for t in (k / steps for k in range(1, steps + 1)):
            u = 1 - t
            out.append((u * u * prev[0] + 2 * u * t * cur[0] + t * t * end[0], u * u * prev[1] + 2 * u * t * cur[1] + t * t * end[1]))
        prev = end
        i += 2 if nxt[2] else 1
    return out


# --- The mark -------------------------------------------------------------------------------------

def turned_m() -> list[list[tuple[float, float]]]:
    """The font's Ɯ in image coordinates (y down), normalized to height 1."""
    contours = glyph_contours(FONT, "Ɯ")
    flipped = [[(x, -y) for x, y in c] for c in contours]  # font units are y-up
    xs = [x for c in flipped for x, _ in c]
    ys = [y for c in flipped for _, y in c]
    x0, y0, h = min(xs), min(ys), max(ys) - min(ys)
    return [[((x - x0) / h, (y - y0) / h) for x, y in c] for c in flipped]


GLYPH = turned_m()
GLYPH_W = max(x for c in GLYPH for x, _ in c)  # width at height 1

# Markdown's down-arrow, in units of glyph height, weighted to match the letter's stems.
ARROW = [(0.32, 1.0), (0.0, 0.58), (0.22, 0.58), (0.22, 0.0), (0.42, 0.0), (0.42, 0.58), (0.64, 0.58)]
ARROW_W = 0.64
GAP = 0.22


def mark(height: float, ox: float, oy: float, arrow: bool):
    """(polygon, color) pairs for the mark with its top-left at (ox, oy)."""
    polys = [([(ox + x * height, oy + y * height) for x, y in c], AMBER) for c in GLYPH]
    if arrow:
        ax = ox + (GLYPH_W + GAP) * height
        polys.append(([(ax + x * height, oy + y * height) for x, y in ARROW], CYAN))
    return polys


def mark_width(height: float, arrow: bool) -> float:
    return (GLYPH_W + (GAP + ARROW_W if arrow else 0.0)) * height


def fit(box_w: float, box_h: float, arrow: bool) -> float:
    """Largest mark height that fits the box."""
    return min(box_h, box_w / mark_width(1.0, arrow))


# --- Rendering ------------------------------------------------------------------------------------

def rounded_rect(x0: float, y0: float, x1: float, y1: float, r: float, steps: int = 24):
    """Rounded rectangle as a polygon (Pillow's rounded_rectangle crashes on some builds)."""
    points = []
    for cx, cy, start in ((x1 - r, y0 + r, -90), (x1 - r, y1 - r, 0), (x0 + r, y1 - r, 90), (x0 + r, y0 + r, 180)):
        for i in range(steps + 1):
            a = math.radians(start + 90 * i / steps)
            points.append((cx + r * math.cos(a), cy + r * math.sin(a)))
    return points


def render(size: int, draw_fn) -> Image.Image:
    big = Image.new("RGBA", (size * SS, size * SS), (0, 0, 0, 0))
    draw_fn(ImageDraw.Draw(big), SS)
    return big.resize((size, size), Image.LANCZOS)


def scaled(polys, k):
    return [([(x * k, y * k) for x, y in poly], color) for poly, color in polys]


def app_icon(size: int) -> Image.Image:
    arrow = size >= 32
    margin = 0 if size <= 32 else round(size / 32)
    radius = max(2, round(size * 0.2))
    # Small sizes drop the arrow and let the letter fill the tile.
    h = round(fit(size * (0.70 if arrow else 0.76), size * (0.46 if arrow else 0.62), arrow))
    ox = round((size - mark_width(h, arrow)) / 2)
    oy = round((size - h) / 2)

    def draw(d: ImageDraw.ImageDraw, k: int):
        edge = max(1, round(size / 64)) * k
        lo, hi = margin * k, (size - margin) * k
        d.polygon(rounded_rect(lo, lo, hi, hi, radius * k), fill=TILE_EDGE)
        d.polygon(rounded_rect(lo + edge, lo + edge, hi - edge, hi - edge, radius * k - edge), fill=TILE)
        for poly, color in scaled(mark(h, ox, oy, arrow), k):
            d.polygon(poly, fill=color)

    return render(size, draw)


def doc_icon(size: int) -> Image.Image:
    small = size <= 24
    arrow = size >= 48
    # Page: portrait sheet with a folded top-right corner.
    inset = 0.09 if small else 0.16
    left, right = round(size * inset), round(size * (1 - inset))
    top, bottom = 0 if small else round(size / 32), size - (0 if small else round(size / 32))
    fold = round(size * 0.26)
    h = round(fit((right - left) * 0.72, (bottom - top - fold) * 0.5, arrow))
    ox = round(left + (right - left - mark_width(h, arrow)) / 2)
    oy = round(top + fold + (bottom - top - fold - h) / 2)

    def draw(d: ImageDraw.ImageDraw, k: int):
        edge = max(1, round(size / 48)) * k
        L, R, T, B, F = left * k, right * k - 1, top * k, bottom * k - 1, fold * k
        d.polygon([(L, T), (R - F, T), (R, T + F), (R, B), (L, B)], fill=PAGE_EDGE)
        inner = [(L + edge, T + edge), (R - F - edge * 0.4, T + edge), (R - edge, T + F + edge * 0.4), (R - edge, B - edge), (L + edge, B - edge)]
        d.polygon(inner, fill=PAGE)
        d.polygon([(R - F, T), (R - F, T + F), (R, T + F)], fill=PAGE_FOLD)
        for poly, color in scaled(mark(h, ox, oy, arrow), k):
            d.polygon(poly, fill=color)

    return render(size, draw)


def save_ico(path: Path, frames: list[Image.Image]):
    frames = sorted(frames, key=lambda im: im.width, reverse=True)
    frames[0].save(path, format="ICO", sizes=[(f.width, f.height) for f in frames], append_images=frames[1:])


def rgba(c):
    return f"rgba({c[0]},{c[1]},{c[2]},{c[3] / 255:.2f})"


SIZES = [256, 64, 48, 40, 32, 24, 20, 16]

if __name__ == "__main__":
    save_ico(OUT / "wackdown.ico", [app_icon(s) for s in SIZES])
    save_ico(OUT / "document.ico", [doc_icon(s) for s in SIZES])
    app_icon(256).save(OUT / "wackdown-256.png")
    doc_icon(256).save(OUT / "document-256.png")

    h = fit(256 * 0.70, 256 * 0.46, True)
    ox, oy = (256 - mark_width(h, True)) / 2, (256 - h) / 2
    shapes = "".join(
        f'  <polygon points="{" ".join(f"{x:.2f},{y:.2f}" for x, y in poly)}" fill="{rgba(color)}"/>\n'
        for poly, color in mark(h, ox, oy, True)
    )
    (OUT / "wackdown.svg").write_text(
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256" width="256" height="256">\n'
        f'  <rect x="4" y="4" width="248" height="248" rx="51" fill="{rgba(TILE)}" stroke="{rgba(TILE_EDGE)}" stroke-width="4"/>\n'
        + shapes
        + "</svg>\n",
        encoding="utf-8",
    )
    print("wrote", ", ".join(p.name for p in sorted(OUT.glob("*.ico"))), f"(glyph contours: {len(GLYPH)})")
