#!/usr/bin/env python3
"""Writes SolidDraft's icon catalogue: original drawings, no third-party art.

Run it from anywhere; --check fails if the committed files differ. Nothing under
crates/soliddraft-app/assets/icons is edited by hand.

Every 3D icon is built from one isometric projection on a 32 x 32 grid, so the set shares
its light direction and line weight. Colour says what a command does:

  grey   the part as it stands
  blue   what the command adds
  coral  what the command removes
  amber  the sketch or profile it works from
  teal   reference geometry
  violet a surface (a sheet with no thickness)

Chrome glyphs (undo, eye, gear and so on) are single-colour line icons on a 16 x 16 grid and
follow the text colour.
"""
import math
import pathlib
import random
import re
import sys

C = 0.866
TONES = {
    "n": dict(top="#F1F3F7", left="#CDD4DE", right="#A9B3C1", line="#3A4659"),
    "a": dict(top="#A9CBFF", left="#5A92F2", right="#2F67DC", line="#17408F"),
    "c": dict(top="#FFD0C6", left="#F08A74", right="#D65B44", line="#8C2D1D"),
    "t": dict(top="#C9EEE8", left="#7CCFC4", right="#3FA99C", line="#0E6F66"),
    "v": dict(top="#DCD2FF", left="#AE97F2", right="#7F63DE", line="#4B3399"),
}
AMBER = "#E8930C"
AMBER_FILL = "#FFE7B8"
TEAL = "#0E7C72"
INK = "#3A4659"
# Colours on the bare background rather than on a fill, one step from their neighbours: the
# app swaps these for lighter ones on a dark panel, while ink on a fill must stay dark.
FREE = "#3A4658"
FREE_BLUE = "#17408E"
FREE_TEAL = "#0E6F67"
FREE_MESH = "#C9EEE7"
FREE_DOT = "#2F67DD"
SW = 0.75


EXTENT = [99.0, 99.0, -99.0, -99.0]


def iso(x, y, z, ox=16.0, oy=20.0):
    p = (ox + (x - y) * C, oy + (x + y) * 0.5 - z)
    EXTENT[0], EXTENT[1] = min(EXTENT[0], p[0]), min(EXTENT[1], p[1])
    EXTENT[2], EXTENT[3] = max(EXTENT[2], p[0]), max(EXTENT[3], p[1])
    return p


# How far past the tracked points a drawing's raw paths reach (left, top, right, bottom):
# fitted() only sees points that went through iso().
REACH = {
    "measure": (0.25, 0.0, 0.25, 0.0),
    "move_comp": (0.25, 0.0, 0.25, 0.0),
    "pc_import": (0.0, 1.1, 0.0, 0.0),
    "sweep": (0.35, 0.0, 0.0, 0.0),
}


def fitted(fn, reach=(0.0, 0.0, 0.0, 0.0)):
    """The icon's body and a square viewBox that holds everything it drew."""
    EXTENT[:] = [3.0, 3.0, 29.0, 29.0]
    body = fn()
    x0, y0, x1, y1 = EXTENT[0] - reach[0], EXTENT[1] - reach[1], EXTENT[2] + reach[2], EXTENT[3] + reach[3]
    side = max(x1 - x0, y1 - y0) + 2.0
    cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
    return body, f"{cx - side / 2:.2f} {cy - side / 2:.2f} {side:.2f} {side:.2f}"


def fmt(points):
    return " ".join(f"{a:.2f},{b:.2f}" for a, b in points)


def poly3(points, fill, line=None, w=SW, extra=""):
    p = fmt(iso(*q) for q in points)
    s = f'stroke="{line}" stroke-width="{w}" stroke-linejoin="round"' if line else 'stroke="none"'
    return f'<polygon points="{p}" fill="{fill}" {s} {extra}/>'


def line3(points, color, w=1.2, dash=None, cap="round"):
    p = fmt(iso(*q) for q in points)
    d = f' stroke-dasharray="{dash}"' if dash else ""
    return f'<polyline points="{p}" fill="none" stroke="{color}" stroke-width="{w}" stroke-linecap="{cap}" stroke-linejoin="round"{d}/>'


def box(x0, y0, z0, x1, y1, z1, tone="n", w=SW):
    t = TONES[tone]
    return (
        poly3([(x0, y1, z0), (x1, y1, z0), (x1, y1, z1), (x0, y1, z1)], t["left"], t["line"], w)
        + poly3([(x1, y0, z0), (x1, y1, z0), (x1, y1, z1), (x1, y0, z1)], t["right"], t["line"], w)
        + poly3([(x0, y0, z1), (x1, y0, z1), (x1, y1, z1), (x0, y1, z1)], t["top"], t["line"], w)
    )


def circle(cx, cy, z, r, n=40, a0=0.0, a1=360.0):
    return [(cx + r * math.cos(math.radians(a0 + (a1 - a0) * i / n)), cy + r * math.sin(math.radians(a0 + (a1 - a0) * i / n)), z) for i in range(n + 1)]


def cyl(cx, cy, r, z0, z1, tone="n", w=SW, top=True):
    """An upright cylinder: the visible half of the wall, shaded by bands, then the cap."""
    t = TONES[tone]
    out = []
    bands = 8
    # The silhouette in this projection falls at -45 and 135 degrees in plan.
    a0, a1 = -45.0, 135.0
    for i in range(bands):
        s = a0 + (a1 - a0) * i / bands
        e = a0 + (a1 - a0) * (i + 1) / bands
        lo = circle(cx, cy, z0, r, 6, s, e)
        hi = circle(cx, cy, z1, r, 6, s, e)
        # Bands run from the right silhouette round the front to the left one.
        col = {0: "right", 1: "right", 2: "right", 3: "left", 4: "left", 5: "left", 6: "left", 7: "left"}[i]
        out.append(poly3(lo + list(reversed(hi)), t[col]))
    lo = circle(cx, cy, z0, r, 24, a0, a1)
    hi = circle(cx, cy, z1, r, 24, a0, a1)
    out.append(poly3(lo + list(reversed(hi)), "none", t["line"], w))
    if top:
        out.append(poly3(circle(cx, cy, z1, r, 40)[:-1], t["top"], t["line"], w))
    return "".join(out)


def hole_in_top(cx, cy, z, r, depth=4.0, tone="c"):
    """A round hole seen from above: the far wall shows through the rim."""
    t = TONES[tone]
    ring = circle(cx, cy, z, r, 40)[:-1]
    far = circle(cx, cy, z, r, 24, 135, 315)
    lowered = [(x, y, z - depth) for x, y, _ in reversed(far)]
    HOLES.append(1)
    cid = f"h{len(HOLES)}"
    clip = f'<clipPath id="{cid}"><polygon points="{fmt(iso(*q) for q in ring)}"/></clipPath>'
    return (
        clip
        + poly3(ring, t["right"])
        + f'<g clip-path="url(#{cid})">' + poly3(far + lowered, t["left"]) + "</g>"
        + poly3(ring, "none", t["line"], SW)
    )


HOLES = []


def arrow3(p, q, color=FREE, w=1.3, head=2.6):
    a, b = iso(*p), iso(*q)
    ang = math.atan2(b[1] - a[1], b[0] - a[0])
    h1 = (b[0] - head * math.cos(ang - 0.5), b[1] - head * math.sin(ang - 0.5))
    h2 = (b[0] - head * math.cos(ang + 0.5), b[1] - head * math.sin(ang + 0.5))
    return (
        f'<line x1="{a[0]:.2f}" y1="{a[1]:.2f}" x2="{b[0]:.2f}" y2="{b[1]:.2f}" stroke="{color}" stroke-width="{w}" stroke-linecap="round"/>'
        f'<polygon points="{fmt([b, h1, h2])}" fill="{color}" stroke="{color}" stroke-width=".4" stroke-linejoin="round"/>'
    )


def arrow2(a, b, color=FREE, w=1.3, head=2.8):
    ang = math.atan2(b[1] - a[1], b[0] - a[0])
    h1 = (b[0] - head * math.cos(ang - 0.5), b[1] - head * math.sin(ang - 0.5))
    h2 = (b[0] - head * math.cos(ang + 0.5), b[1] - head * math.sin(ang + 0.5))
    return (
        f'<line x1="{a[0]}" y1="{a[1]}" x2="{b[0]}" y2="{b[1]}" stroke="{color}" stroke-width="{w}" stroke-linecap="round"/>'
        f'<polygon points="{fmt([b, h1, h2])}" fill="{color}" stroke="{color}" stroke-width=".4" stroke-linejoin="round"/>'
    )


def sheet(a, b, tone="v", bands=6):
    """A ruled surface between two 3D polylines of equal length, shaded along its length."""
    t = TONES[tone]
    out = []
    n = len(a) - 1
    for i in range(n):
        f = i / max(n - 1, 1)
        col = t["top"] if f < 0.34 else (t["left"] if f < 0.7 else t["right"])
        out.append(poly3([a[i], a[i + 1], b[i + 1], b[i]], col, col, 0.25))
    out.append(line3(a, t["line"], 0.9))
    out.append(line3(b, t["line"], 0.9))
    out.append(line3([a[0], b[0]], t["line"], 0.9))
    out.append(line3([a[-1], b[-1]], t["line"], 0.9))
    return "".join(out)


def wave(x0, x1, y, z, amp, n=16, axis="z", phase=0.0, cycles=1.0):
    pts = []
    for i in range(n + 1):
        f = i / n
        v = amp * math.sin(f * 2 * math.pi * cycles + phase)
        x = x0 + (x1 - x0) * f
        pts.append((x, y, z + v) if axis == "z" else (x, y + v, z))
    return pts


def dots(points, color, r=0.9):
    return "".join(f'<circle cx="{a:.2f}" cy="{b:.2f}" r="{r}" fill="{color}"/>' for a, b in points)


def cloud(seed, n, ox=16, oy=16, rx=10.5, ry=8.5):
    rnd = random.Random(seed)
    pts = []
    while len(pts) < n:
        a, b = rnd.uniform(-1, 1), rnd.uniform(-1, 1)
        if a * a + b * b <= 1:
            pts.append((ox + a * rx, oy + b * ry))
    return pts


def vertex(x, y, s=2.4):
    return f'<rect x="{x - s / 2}" y="{y - s / 2}" width="{s}" height="{s}" rx=".4" fill="#fff" stroke="{TONES["a"]["line"]}" stroke-width=".9"/>'


def sk(d, w=1.9, dash=None):
    a = f' stroke-dasharray="{dash}"' if dash else ""
    return f'<path d="{d}" fill="none" stroke="{AMBER}" stroke-width="{w}" stroke-linecap="round" stroke-linejoin="round"{a}/>'


def tick(x, y, s=1.0):
    return f'<path d="M{x - 3 * s} {y} l{2.2 * s} {2.4 * s} l{4.4 * s} -{5.2 * s}" fill="none" stroke="#1E9E5A" stroke-width="{2 * s}" stroke-linecap="round" stroke-linejoin="round"/>'


def rounded_profile(chamfer=False):
    """A block with its near top edge rounded or bevelled, shown along X."""
    t = TONES["n"]
    a = TONES["a"]
    x0, x1 = -10, 9
    y0, y1 = -8, 9
    z0, z1 = 0, 10
    r = 7.0
    arc = []
    if chamfer:
        arc = [(y1 - r, z1), (y1, z1 - r)]
    else:
        for i in range(13):
            ang = math.radians(90 - 90 * i / 12)
            arc.append((y1 - r + r * math.cos(ang), z1 - r + r * math.sin(ang)))
    end = [(y0, z0), (y1, z0)] + list(reversed(arc)) + [(y0, z1)]
    out = [poly3([(x1, y, z) for y, z in end], t["right"], t["line"])]
    out.append(poly3([(x0, y1, z0), (x1, y1, z0), (x1, y1, z1 - r), (x0, y1, z1 - r)], t["left"], t["line"]))
    out.append(poly3([(x0, y0, z1), (x1, y0, z1), (x1, y1 - r, z1), (x0, y1 - r, z1)], t["top"], t["line"]))
    n = len(arc) - 1
    for i in range(n):
        f = i / max(n, 1)
        col = a["top"] if f < 0.4 else (a["left"] if f < 0.8 else a["right"])
        (ya, za), (yb, zb) = arc[i], arc[i + 1]
        out.append(poly3([(x0, ya, za), (x1, ya, za), (x1, yb, zb), (x0, yb, zb)], col, col if not chamfer else a["line"], 0.3 if not chamfer else SW))
    if not chamfer:
        out.append(line3([(x0, arc[0][0], arc[0][1]), (x1, arc[0][0], arc[0][1])], a["line"], SW))
        out.append(line3([(x0, arc[-1][0], arc[-1][1]), (x1, arc[-1][0], arc[-1][1])], a["line"], SW))
    return "".join(out)


def pocket(x0, y0, x1, y1, z, d, tone="c"):
    """A rectangular pocket in a top face: the two far walls show."""
    t = TONES[tone]
    return (
        poly3([(x0, y0, z), (x1, y0, z), (x1, y1, z), (x0, y1, z)], t["right"], t["line"])
        + poly3([(x0, y0, z), (x0, y1, z), (x0, y1, z - d), (x0, y0, z - d)], t["right"])
        + poly3([(x0, y0, z), (x1, y0, z), (x1, y0, z - d), (x0, y0, z - d)], t["left"])
        + poly3([(x0, y0, z - d), (x1, y0, z - d), (x1, y1, z - d), (x0, y1, z - d)], t["top"])
        + poly3([(x0, y0, z), (x1, y0, z), (x1, y1, z), (x0, y1, z)], "none", t["line"])
    )


def icon_extrude():
    return (
        poly3([(-9, -9, 0), (9, -9, 0), (9, 9, 0), (-9, 9, 0)], AMBER_FILL, AMBER, 1.3)
        + box(-7, -7, 0, 7, 7, 8, "a")
        + arrow3((0, 0, 8), (0, 0, 17), FREE, 1.5, 3.2)
    )


def icon_revolve():
    return (
        line3([(0, 0, -5), (0, 0, 19)], FREE, 1.1, "2 1.6")
        + cyl(0, 0, 8.5, 0, 9, "a")
        + hole_in_top(0, 0, 9, 3.2, 5, "a")
        + f'<path d="M6.8 8.3 C 9.5 4.4, 22.5 4.4, 25.2 8.3" fill="none" stroke="{FREE}" stroke-width="1.3" stroke-linecap="round"/>'
        + f'<polygon points="27.2,10.8 22.9,8.8 26.1,6.3" fill="{FREE}"/>'
    )


def icon_sweep():
    c = TONES["a"]
    path = "M6 25 C 6 13, 15 21, 17 13 C 18.6 7, 23 6, 27 7"
    return (
        f'<path d="{path}" fill="none" stroke="{c["line"]}" stroke-width="7" stroke-linecap="butt"/>'
        f'<path d="{path}" fill="none" stroke="{c["left"]}" stroke-width="5.6" stroke-linecap="butt"/>'
        f'<path d="{path}" fill="none" stroke="{c["top"]}" stroke-width="2" stroke-linecap="butt" transform="translate(-.6 -.7)"/>'
        f'<ellipse cx="6" cy="25.5" rx="3.6" ry="1.9" fill="{AMBER_FILL}" stroke="{AMBER}" stroke-width="1.3"/>'
    )


def icon_loft():
    t = TONES["a"]
    bot = [(-10, -10, 0), (10, -10, 0), (10, 10, 0), (-10, 10, 0)]
    topc = [(4.2 * math.cos(math.radians(a)), 4.2 * math.sin(math.radians(a)), 15) for a in range(0, 360, 12)]
    return (
        poly3(bot, AMBER_FILL, AMBER, 1.2)
        + poly3([(-10, 10, 0), (10, 10, 0), (3, 3, 15), (-3, 3, 15)], t["left"], t["line"])
        + poly3([(10, -10, 0), (10, 10, 0), (3, 3, 15), (3, -3, 15)], t["right"], t["line"])
        + poly3(topc, AMBER_FILL, AMBER, 1.2)
    )


def icon_cut():
    return (
        box(-10, -10, 0, 10, 10, 8)
        + pocket(-5.5, -5.5, 5.5, 5.5, 8, 5)
        + arrow3((0, 0, 19), (0, 0, 10), TONES["c"]["line"], 1.5, 3.2)
    )


def icon_hole():
    return box(-10, -10, 0, 10, 10, 8) + hole_in_top(0, 0, 8, 5.4, 6)


def icon_slot():
    t = TONES["c"]
    a = circle(-4.5, 0, 8, 3.6, 16, 90, 270)
    b = circle(4.5, 0, 8, 3.6, 16, -90, 90)
    ring = a + b
    return box(-11, -9, 0, 11, 9, 8) + poly3(ring, t["right"], t["line"]) + "".join([
        f'<clipPath id="slotc"><polygon points="{fmt(iso(*q) for q in ring)}"/></clipPath>',
        '<g clip-path="url(#slotc)">',
        poly3([(-9, -3.6, 8), (9, -3.6, 8), (9, -3.6, 3), (-9, -3.6, 3)], t["left"]),
        poly3([(-8.1, -3.6, 8), (-8.1, 3.6, 8), (-8.1, 3.6, 3), (-8.1, -3.6, 3)], t["right"]),
        "</g>",
        poly3(ring, "none", t["line"]),
    ])


def icon_fillet():
    return rounded_profile(False)


def icon_chamfer():
    return rounded_profile(True)


def icon_shell():
    t = TONES["a"]
    n = TONES["n"]
    return (
        box(-10, -10, 0, 10, 10, 11, "n")
        + poly3([(-10, -10, 11), (10, -10, 11), (10, 10, 11), (-10, 10, 11)], t["top"], t["line"])
        + poly3([(-7.4, -7.4, 11), (7.4, -7.4, 11), (7.4, 7.4, 11), (-7.4, 7.4, 11)], n["right"], n["line"])
        + poly3([(-7.4, -7.4, 11), (-7.4, 7.4, 11), (-7.4, 7.4, 4), (-7.4, -7.4, 4)], n["right"])
        + poly3([(-7.4, -7.4, 11), (7.4, -7.4, 11), (7.4, -7.4, 4), (-7.4, -7.4, 4)], n["left"])
        + poly3([(-7.4, -7.4, 4), (7.4, -7.4, 4), (7.4, 7.4, 4), (-7.4, 7.4, 4)], n["top"])
        + poly3([(-7.4, -7.4, 11), (7.4, -7.4, 11), (7.4, 7.4, 11), (-7.4, 7.4, 11)], "none", n["line"])
    )


def icon_draft():
    t = TONES["a"]
    n = TONES["n"]
    return (
        poly3([(-10, 10, 0), (10, 10, 0), (6, 6, 13), (-6, 6, 13)], t["left"], t["line"])
        + poly3([(10, -10, 0), (10, 10, 0), (6, 6, 13), (6, -6, 13)], t["right"], t["line"])
        + poly3([(-6, -6, 13), (6, -6, 13), (6, 6, 13), (-6, 6, 13)], n["top"], n["line"])
        + line3([(10, 10, 0), (10, 10, 15)], FREE, 1, "1.8 1.5")
    )


def icon_lattice():
    t = TONES["a"]
    c = t["right"]
    a, b = -8.5, 8.5
    z0, z1 = 0, 15
    out = []
    corners = [(a, a), (b, a), (b, b), (a, b)]
    for (x, y) in [(a, a)]:
        out.append(line3([(x, y, z0), (x, y, z1)], t["left"], 1.2))
    out.append(line3([(a, a, z0), (b, a, z0)], t["left"], 1.2))
    out.append(line3([(a, a, z0), (a, b, z0)], t["left"], 1.2))
    out.append(line3([(a, a, z0), (b, b, z1)], c, 1.5))
    out.append(line3([(b, a, z0), (a, b, z1)], c, 1.5))
    out.append(line3([(a, b, z0), (b, a, z1)], c, 1.5))
    out.append(line3([(b, b, z0), (a, a, z1)], c, 1.5))
    for (x, y) in corners[1:]:
        out.append(line3([(x, y, z0), (x, y, z1)], FREE_BLUE, 1.2))
    out.append(line3([(b, a, z0), (b, b, z0), (a, b, z0)], FREE_BLUE, 1.2))
    out.append(line3([(a, a, z1), (b, a, z1), (b, b, z1), (a, b, z1), (a, a, z1)], FREE_BLUE, 1.2))
    return "".join(out)


def icon_linear():
    items = []
    for gy in (-12, -2, 8):
        for gx in (-12, -2, 8):
            tone = "a" if gx == -12 and gy == 8 else "n"
            items.append((gx + gy, box(gx, gy, 0, gx + 6.5, gy + 6.5, 2.6, tone)))
    items.sort(key=lambda p: p[0])
    return "".join(s for _, s in items)


def icon_circular():
    items = []
    for i in range(6):
        a = math.radians(i * 60 + 15)
        x, y = 11 * math.cos(a), 11 * math.sin(a)
        items.append((x + y, cyl(x, y, 3.5, 0, 2.4, "a" if i == 1 else "n")))
    items.sort(key=lambda p: p[0])
    centre = iso(0, 0, 0)
    return (
        line3(circle(0, 0, 0, 11, 40), FREE, 0.9, "1.6 1.4")
        + f'<circle cx="{centre[0]:.2f}" cy="{centre[1]:.2f}" r="1.5" fill="{FREE}"/>'
        + "".join(s for _, s in items)
    )


def icon_mirror():
    t = TONES["t"]
    return (
        box(-11, -8, 0, -4, 8, 6, "n")
        + poly3([(0, -11, -3), (0, 11, -3), (0, 11, 15), (0, -11, 15)], t["top"] + "", t["line"], 0.9, 'fill-opacity=".85"')
        + box(4, -8, 0, 11, 8, 6, "a")
    )


def icon_plane():
    t = TONES["t"]
    return (
        poly3([(-12, -12, 3), (12, -12, 3), (12, 12, 3), (-12, 12, 3)], t["top"], t["line"], 1.1)
        + line3([(-4, -12, 3), (-4, 12, 3)], t["left"], 0.7)
        + line3([(4, -12, 3), (4, 12, 3)], t["left"], 0.7)
        + line3([(-12, -4, 3), (12, -4, 3)], t["left"], 0.7)
        + line3([(-12, 4, 3), (12, 4, 3)], t["left"], 0.7)
    )


def icon_axis():
    t = TONES["t"]
    return (
        poly3([(-9, -9, 2), (9, -9, 2), (9, 9, 2), (-9, 9, 2)], "#E8F7F5", t["left"], 0.8)
        + line3([(0, 0, -7), (0, 0, 2)], FREE_TEAL, 1.6, "2.4 1.8")
        + arrow3((0, 0, 2), (0, 0, 19), FREE_TEAL, 1.7, 3.6)
    )


def icon_refpoint():
    t = TONES["t"]
    x, y = iso(0, 0, 4)
    return (
        poly3([(-9, -9, 2), (9, -9, 2), (9, 9, 2), (-9, 9, 2)], "#E8F7F5", t["left"], 0.8)
        + f'<path d="M{x} {y - 8} v5 M{x} {y + 3} v5 M{x - 8} {y} h5 M{x + 3} {y} h5" stroke="{FREE_TEAL}" stroke-width="1.4" stroke-linecap="round"/>'
        + f'<circle cx="{x}" cy="{y}" r="2.3" fill="#fff" stroke="{t["line"]}" stroke-width="1.5"/>'
    )


def icon_origin():
    a = iso(0, 0, 0)
    return (
        arrow3((0, 0, 0), (13, 0, 0), "#D23B3B", 1.6, 3.2)
        + arrow3((0, 0, 0), (0, 13, 0), "#2E9E5B", 1.6, 3.2)
        + arrow3((0, 0, 0), (0, 0, 15), "#1F5FD0", 1.6, 3.2)
        + f'<circle cx="{a[0]:.2f}" cy="{a[1]:.2f}" r="2" fill="#fff" stroke="{INK}" stroke-width="1.1"/>'
    )


def icon_drawing():
    a = TONES["a"]
    return (
        f'<rect x="4" y="4" width="24" height="24" rx="1.5" fill="#fff" stroke="{INK}" stroke-width="1"/>'
        f'<rect x="17" y="22.5" width="11" height="5.5" fill="{a["top"]}" stroke="{INK}" stroke-width=".8"/>'
        f'<g transform="translate(-1.6 -2.8) scale(.62) translate(9 4)">' + box(-7, -7, 0, 7, 7, 6) + "</g>"
        f'<path d="M7.5 22 h8" stroke="{a["right"]}" stroke-width="1.1"/><path d="M7.5 20.6 v2.8 M15.5 20.6 v2.8" stroke="{a["right"]}" stroke-width="1"/>'
    )


def icon_open_drawings():
    return (
        f'<path d="M4 9 h9 l2 2.4 h13 v15.6 h-24z" fill="#FFE7B8" stroke="{AMBER}" stroke-width="1.1" stroke-linejoin="round"/>'
        f'<rect x="11" y="5" width="13" height="15" fill="#fff" stroke="{INK}" stroke-width=".9"/>'
        f'<path d="M4 14 h24 v13 h-24z" fill="#FFD48A" stroke="{AMBER}" stroke-width="1.1" stroke-linejoin="round"/>'
    )


def icon_sketch():
    t = TONES["t"]
    return (
        poly3([(-11, -11, 0), (11, -11, 0), (11, 11, 0), (-11, 11, 0)], "#E8F7F5", t["left"], 1)
        + poly3([(-6, -6, 0), (6, -6, 0), (6, 6, 0), (-6, 6, 0)], AMBER_FILL, AMBER, 1.5)
        + f'<g transform="translate(16.5 2.5)"><path d="M0 15 L1.2 10.6 L10.6 1.2 a1.5 1.5 0 0 1 2.1 0 l1.1 1.1 a1.5 1.5 0 0 1 0 2.1 L4.4 13.8z" fill="#FFD48A" stroke="{INK}" stroke-width=".95" stroke-linejoin="round"/><path d="M1.2 10.6 L4.4 13.8" stroke="{INK}" stroke-width=".95"/><path d="M0 15 L.7 12.4 L2.6 14.3z" fill="{INK}"/></g>'
    )


def icon_exit_sketch():
    t = TONES["t"]
    return (
        poly3([(-11, -11, 0), (11, -11, 0), (11, 11, 0), (-11, 11, 0)], "#E8F7F5", t["left"], 1)
        + poly3([(-6, -6, 0), (6, -6, 0), (6, 6, 0), (-6, 6, 0)], AMBER_FILL, AMBER, 1.5)
        + tick(23, 9, 1.4)
    )


def icon_line():
    return sk("M6 25 L26 7") + vertex(6, 25) + vertex(26, 7)


def icon_circle():
    return sk("M26 16 a10 10 0 1 1 -20 0 a10 10 0 1 1 20 0") + vertex(16, 16)


def icon_rect():
    return sk("M5.5 8.5 h21 v15 h-21 z") + vertex(5.5, 8.5) + vertex(26.5, 23.5)


def icon_arc():
    return sk("M6 24 A 14 14 0 0 1 26 12") + vertex(6, 24) + vertex(26, 12) + vertex(14, 24.5, 2)


def icon_slot_sketch():
    return sk("M10 9.5 h12 a6.5 6.5 0 0 1 0 13 h-12 a6.5 6.5 0 0 1 0 -13 z") + vertex(10, 16, 2) + vertex(22, 16, 2)


def icon_sk_point():
    return f'<path d="M16 6 v6 M16 20 v6 M6 16 h6 M20 16 h6" stroke="{AMBER}" stroke-width="1.8" stroke-linecap="round"/>' + vertex(16, 16, 3.4)


def icon_smart_dim():
    a = TONES["a"]["right"]
    return (
        f'<path d="M6 25 V13 M26 25 V13" stroke="{FREE}" stroke-width="1.1" stroke-linecap="round"/>'
        + arrow2((16, 18), (6.5, 18), a, 1.4, 3.4)
        + arrow2((16, 18), (25.5, 18), a, 1.4, 3.4)
        + f'<rect x="9.5" y="4" width="13" height="9" rx="1.6" fill="#fff" stroke="{a}" stroke-width="1"/>'
        + f'<text x="16" y="11" font-family="Inter,system-ui,sans-serif" font-size="7.2" font-weight="700" text-anchor="middle" fill="{a}">10</text>'
    )


def icon_construction():
    return sk("M6 25 L26 7", 1.7, "3.2 2.6") + (
        f'<circle cx="6" cy="25" r="2" fill="#fff" stroke="{AMBER}" stroke-width="1.2"/><circle cx="26" cy="7" r="2" fill="#fff" stroke="{AMBER}" stroke-width="1.2"/>'
    )


def icon_relations():
    a = TONES["a"]["right"]
    return sk("M6 25 H26") + sk("M11 25 V6") + f'<path d="M11 19 h6 v6" fill="none" stroke="{a}" stroke-width="1.5" stroke-linejoin="round"/>'


def icon_normal_to():
    t = TONES["t"]
    return (
        poly3([(-11, -11, 0), (11, -11, 0), (11, 11, 0), (-11, 11, 0)], t["top"], t["line"], 1)
        + arrow3((0, 0, 0), (0, 0, 16), TONES["a"]["right"], 1.8, 3.8)
        + f'<circle cx="16" cy="20" r="1.4" fill="{TONES["a"]["right"]}"/>'
    )


# Assembly -------------------------------------------------------------------------------
def icon_insert_comp():
    return (
        box(-9, -9, 0, 9, 9, 9, "n")
        + f'<circle cx="23.5" cy="8.5" r="6" fill="{TONES["a"]["left"]}" stroke="{TONES["a"]["line"]}" stroke-width=".8"/>'
        + '<path d="M23.5 5.4 v6.2 M20.4 8.5 h6.2" stroke="#fff" stroke-width="1.7" stroke-linecap="round"/>'
    )


def icon_mate():
    return (
        box(-11, -9, 0, 9, 9, 5, "n")
        + box(-7, -6, 12, 5, 6, 17, "a")
        + arrow3((-1, 0, 12), (-1, 0, 6.5), FREE, 1.3, 2.8)
    )


def icon_move_comp():
    x, y = iso(0, 0, 5)
    return box(-6, -6, 0, 6, 6, 8, "a") + (
        arrow2((6, 11), (2.5, 9.3), FREE, 1.3, 2.6)
        + arrow2((26, 11), (29.5, 9.3), FREE, 1.3, 2.6)
        + arrow2((6, 25), (2.5, 26.7), FREE, 1.3, 2.6)
        + arrow2((26, 25), (29.5, 26.7), FREE, 1.3, 2.6)
    )


def icon_interference():
    c = TONES["c"]
    return box(-11, -8, 0, 3, 8, 8, "n") + box(-3, -6, 4, 11, 6, 13, "a") + (
        poly3([(-3, -6, 8), (3, -6, 8), (3, 6, 8), (-3, 6, 8)], c["left"], c["line"], 0.9)
    )


def icon_explode():
    return (
        line3([(0, 0, -6), (0, 0, 20)], FREE, 1, "1.8 1.5")
        + box(-9, -9, -4, 9, 9, -1, "n")
        + box(-7, -7, 5, 7, 7, 8, "a")
        + box(-5, -5, 14, 5, 5, 17, "n")
    )


# Evaluate --------------------------------------------------------------------------------
def icon_measure():
    t = TONES["n"]
    return (
        '<g transform="rotate(-38 16 16)">'
        f'<rect x="2" y="11" width="28" height="10" rx="1.2" fill="#FFD98A" stroke="{INK}" stroke-width="1"/>'
        + "".join(f'<path d="M{x} 11 v{5.2 if i % 2 == 0 else 3}" stroke="{INK}" stroke-width=".95"/>' for i, x in enumerate(range(6, 28, 4)))
        + "</g>"
    )


def icon_section():
    c = TONES["c"]
    t = TONES["t"]
    return (
        box(-11, -10, 0, 0, 10, 12, "n")
        + poly3([(0, -10, 0), (0, 10, 0), (0, 10, 12), (0, -10, 12)], c["top"], c["line"])
        + "".join(line3([(0, -10 + i * 4, 0), (0, -10 + i * 4 + 4, 8)], c["left"], 0.7) for i in range(5))
        + poly3([(4, -14, -2), (4, 14, -2), (4, 14, 15), (4, -14, 15)], t["top"], t["line"], 0.9, 'fill-opacity=".45"')
    )


def icon_check():
    return box(-10, -10, 0, 10, 10, 8, "n") + tick(16, 13, 1.7)


def icon_draft_check():
    return (
        poly3([(-10, 10, 0), (10, 10, 0), (6, 6, 13), (-6, 6, 13)], "#4CC38A", "#17643F", 0.8)
        + poly3([(10, -10, 0), (10, 10, 0), (6, 6, 13), (6, -6, 13)], "#F2C14E", "#8A6A12", 0.8)
        + poly3([(-6, -6, 13), (6, -6, 13), (6, 6, 13), (-6, 6, 13)], "#EF6A5A", "#8C2D1D", 0.8)
    )


def icon_mass():
    n = TONES["n"]
    return (
        f'<path d="M9 12 h14 l4.5 15 h-23z" fill="{n["left"]}" stroke="{n["line"]}" stroke-width=".9" stroke-linejoin="round"/>'
        f'<path d="M9 12 h4 l-3.6 15 h-4.9z" fill="{n["top"]}"/>'
        f'<circle cx="16" cy="8" r="4" fill="none" stroke="{FREE}" stroke-width="1.6"/>'
        f'<text x="16" y="23.5" font-family="Inter,system-ui,sans-serif" font-size="8" font-weight="700" text-anchor="middle" fill="{n["line"]}">kg</text>'
    )


def icon_zebra():
    out = [f'<clipPath id="zeb"><path d="M4 22 C 9 8, 23 8, 28 22 L 28 27 L 4 27z"/></clipPath>', '<g clip-path="url(#zeb)">', '<rect width="32" height="32" fill="#fff"/>']
    for i in range(-2, 7):
        out.append(f'<path d="M{i * 5 + 1} 28 C {i * 5 + 3} 18, {i * 5 + 6} 14, {i * 5 + 9} 6" stroke="#2B3442" stroke-width="2.4" fill="none"/>')
    out.append("</g>")
    out.append(f'<path d="M4 22 C 9 8, 23 8, 28 22 L 28 27 L 4 27z" fill="none" stroke="{FREE}" stroke-width="1"/>')
    return "".join(out)


# Surface ---------------------------------------------------------------------------------
def icon_srf_extrude():
    a = wave(-11, 11, 3, 0, 2.6, axis="y")
    b = [(x, y, 13) for x, y, _ in a]
    return sheet(a, b) + line3(a, AMBER, 1.5)


def icon_srf_revolve():
    pts_a = circle(0, 0, 2, 10, 24, -60, 200)
    pts_b = circle(0, 0, 14, 4.5, 24, -60, 200)
    return line3([(0, 0, -4), (0, 0, 18)], FREE, 1, "1.8 1.5") + sheet(pts_a, pts_b)


def icon_srf_sweep():
    a = [(-11 + 22 * i / 16, 7 * math.sin(i / 16 * math.pi * 1.5), 0) for i in range(17)]
    b = [(x + 2, y - 7, z + 7) for x, y, z in a]
    return sheet(a, b) + line3(a, AMBER, 1.5)


def icon_srf_loft():
    a = wave(-11, 11, 7, 0, 3)
    b = wave(-9, 9, -6, 12, 2.5, phase=2.4)
    return sheet(a, b) + line3(a, AMBER, 1.5) + line3(b, AMBER, 1.5)


def icon_srf_boundary():
    a = wave(-11, 11, 7, 0, 2)
    b = wave(-11, 11, -7, 11, 2, phase=1.4)
    return sheet(a, b) + line3(a, AMBER, 1.5) + line3(b, AMBER, 1.5) + line3([a[0], b[0]], AMBER, 1.5) + line3([a[-1], b[-1]], AMBER, 1.5)


def icon_srf_fill():
    t = TONES["v"]
    n = TONES["n"]
    return (
        poly3([(-12, -12, 3), (12, -12, 3), (12, 12, 3), (-12, 12, 3)], n["top"], n["line"])
        + poly3(circle(0, 0, 3, 7.5, 40)[:-1], t["left"], t["line"], 1)
        + line3(circle(0, 0, 3, 7.5, 40), AMBER, 1.4, "2 1.5")
    )


def icon_srf_offset():
    a = wave(-11, 11, 7, -2, 1.8)
    b = wave(-11, 11, -8, 0, 1.8)
    a2 = [(x, y, z + 9) for x, y, z in a]
    b2 = [(x, y, z + 9) for x, y, z in b]
    return sheet(a, b, "n") + sheet(a2, b2) + arrow3((0, 0, 3), (0, 0, 8), FREE, 1.2, 2.4)


def icon_srf_trim():
    t = TONES["v"]
    return (
        poly3([(-12, -12, 2), (12, -12, 2), (12, 12, 2), (-12, 12, 2)], t["left"], t["line"])
        + poly3([(-12, 12, 2), (-2, 12, 2), (-2, 2, 2), (-12, 2, 2)], "#FFFFFF", TONES["c"]["line"], 1, 'stroke-dasharray="2 1.5"')
        + line3([(-14, 2, 2), (4, 2, 2)], TONES["c"]["right"], 1.4)
        + line3([(-2, 14, 2), (-2, -4, 2)], TONES["c"]["right"], 1.4)
    )


def icon_srf_extend():
    t = TONES["v"]
    return (
        poly3([(-12, -10, 2), (3, -10, 2), (3, 10, 2), (-12, 10, 2)], t["left"], t["line"])
        + poly3([(3, -10, 2), (12, -10, 2), (12, 10, 2), (3, 10, 2)], t["top"], t["line"], 0.9, 'stroke-dasharray="2 1.5"')
        + arrow3((4, 0, 2), (15, 0, 2), FREE, 1.4, 3)
    )


def icon_srf_knit():
    a = TONES["v"]
    return (
        poly3([(-12, -10, 2), (-1, -10, 2), (-1, 10, 2), (-12, 10, 2)], a["left"], a["line"])
        + poly3([(1, -10, 2), (12, -10, 2), (12, 10, 2), (1, 10, 2)], a["top"], a["line"])
        + "".join(line3([(-2.2, y, 2), (2.2, y + 1.6, 2)], FREE, 1.05) for y in (-8, -4, 0, 4, 8))
    )


def icon_srf_thicken():
    a = wave(-11, 11, 8, 8, 1.6)
    b = wave(-11, 11, -8, 8, 1.6)
    a0 = [(x, y, z - 6) for x, y, z in a]
    return sheet(a0, a, "a", 6) + sheet(a, b)


# Point cloud -----------------------------------------------------------------------------
def cloud_color(i):
    return ["#0E7C72", "#1D9A8D", "#0B5F58"][i % 3]


def scatter(seed, n, **kw):
    pts = cloud(seed, n, **kw)
    return "".join(f'<circle cx="{a:.2f}" cy="{b:.2f}" r=".95" fill="{cloud_color(i)}"/>' for i, (a, b) in enumerate(pts))


def icon_pc_import():
    return scatter(3, 34, oy=19) + arrow2((16, 2), (16, 13), TONES["a"]["right"], 2, 4.4)


def icon_pc_clean():
    return scatter(5, 28, ox=14, oy=17, rx=9, ry=9) + (
        f'<path d="M23.5 6.5 l5 5 M28.5 6.5 l-5 5" stroke="{TONES["c"]["right"]}" stroke-width="2" stroke-linecap="round"/>'
        f'<circle cx="26" cy="23" r="1.3" fill="{TONES["c"]["right"]}"/><circle cx="4.5" cy="6" r="1.3" fill="{TONES["c"]["right"]}"/>'
    )


def icon_pc_decimate():
    return (
        scatter(7, 26, ox=9, oy=16, rx=6.5, ry=10)
        + arrow2((15.4, 16), (19.6, 16), FREE, 1.3, 2.8)
        + "".join(f'<circle cx="{x}" cy="{y}" r="1.25" fill="#0E7C72"/>' for x, y in [(23, 8), (28, 13), (22, 15), (27, 20), (23, 24)])
    )


def icon_pc_register():
    return (
        scatter(9, 16, ox=11, oy=12, rx=7.5, ry=7)
        + "".join(f'<circle cx="{a:.2f}" cy="{b:.2f}" r=".95" fill="{FREE_DOT}"/>' for a, b in cloud(11, 16, ox=21, oy=21, rx=7.5, ry=7))
        + arrow2((25, 6), (20, 11), FREE, 1.3, 2.8)
    )


def icon_pc_mesh():
    v = [(5, 22), (11, 11), (18, 6), (26, 10), (12, 24), (19, 16), (27, 22)]
    e = [(0, 1), (1, 2), (2, 3), (0, 4), (1, 4), (1, 5), (2, 5), (3, 5), (4, 5), (4, 6), (5, 6), (3, 6)]
    t = TONES["t"]
    return (
        f'<polygon points="5,22 11,11 18,6 26,10 27,22 12,24" fill="{FREE_MESH}"/>'
        + "".join(f'<line x1="{v[a][0]}" y1="{v[a][1]}" x2="{v[b][0]}" y2="{v[b][1]}" stroke="{FREE_TEAL}" stroke-width=".95"/>' for a, b in e)
        + dots(v, "#0B5F58", 1.5)
    )


def icon_pc_fit():
    t = TONES["a"]
    return (
        poly3([(-12, -12, 2), (12, -12, 2), (12, 12, 2), (-12, 12, 2)], t["top"], t["line"], 0.9)
        + dots([iso(random.Random(i).uniform(-10, 10), random.Random(i + 40).uniform(-10, 10), 2 + random.Random(i + 80).uniform(-1.8, 2.4)) for i in range(26)], "#0B5F58", 0.95)
    )


def icon_pc_deviation():
    pal = ["#2F67DC", "#2FA9C9", "#3DBB7A", "#F2C14E", "#EF6A5A"]
    pts = cloud(13, 36, oy=15, ry=10)
    out = "".join(f'<circle cx="{a:.2f}" cy="{b:.2f}" r="1.05" fill="{pal[min(4, int((a - 5) / 22 * 5))]}"/>' for a, b in pts)
    bar = "".join(f'<rect x="{5 + i * 4.4}" y="27" width="4.4" height="2.6" fill="{c}"/>' for i, c in enumerate(pal))
    return out + bar


# Tree and part items ---------------------------------------------------------------------
def icon_part():
    return box(-10, -10, 0, 10, 10, 11, "n")


def icon_body():
    return box(-10, -10, 0, 10, 10, 11, "a")


def icon_material():
    return (
        '<defs><radialGradient id="mat" cx=".35" cy=".3" r=".8"><stop offset="0" stop-color="#FFFFFF"/><stop offset=".5" stop-color="#B9C2CF"/><stop offset="1" stop-color="#5D6A7D"/></radialGradient></defs>'
        f'<circle cx="16" cy="16" r="11.5" fill="url(#mat)" stroke="{INK}" stroke-width=".8"/>'
    )


CAD = {k[5:]: v for k, v in list(globals().items()) if k.startswith("icon_")}

LINE = {
    "undo": '<path d="M6 4 L3 7 l3 3"/><path d="M3 7 h7 a3 3 0 0 1 0 6 h-3"/>',
    "redo": '<path d="M10 4 l3 3 -3 3"/><path d="M13 7 h-7 a3 3 0 0 0 0 6 h3"/>',
    "save": '<path d="M3 3 h8 l2 2 v8 h-10z"/><path d="M5.5 3 v3 h5 v-3"/><path d="M5.5 13 v-4 h5 v4"/>',
    "open": '<path d="M2.5 12.5 v-9 h4 l1.5 1.5 h5.5 v2"/><path d="M2.5 12.5 l2 -5 h9.5 l-2 5z"/>',
    "new": '<path d="M4 2.5 h6 l3 3 v8 h-9z"/><path d="M10 2.5 v3 h3"/>',
    "search": '<circle cx="7" cy="7" r="4"/><path d="M10 10 L13.5 13.5"/>',
    "gear": '<circle cx="8" cy="8" r="2"/><path d="M8 1.8 v2 M8 12.2 v2 M1.8 8 h2 M12.2 8 h2 M3.6 3.6 l1.4 1.4 M11 11 l1.4 1.4 M12.4 3.6 L11 5 M5 11 l-1.4 1.4"/>',
    "eye": '<path d="M1.5 8 c2.8 -4.4 10.2 -4.4 13 0 c-2.8 4.4 -10.2 4.4 -13 0z"/><circle cx="8" cy="8" r="2" fill="currentColor" stroke="none"/>',
    "eyeoff": '<path d="M2.5 9.8 C 1.9 9.2 1.8 8.6 1.5 8 c1.2 -1.9 3 -3 4.8 -3.4"/><path d="M13.6 6.2 c.4 .5 .7 1.2 .9 1.8 c-2 3.2 -6.3 4.1 -9.4 2.6"/><path d="M3 13 L13 3"/>',
    "chevr": '<path d="M6 4 l4 4 -4 4"/>',
    "chevd": '<path d="M4 6 l4 4 4 -4"/>',
    "close": '<path d="M4 4 l8 8 M12 4 l-8 8"/>',
    "term": '<path d="M3 4 l4 4 -4 4"/><path d="M8.5 12 h5"/>',
    "spark": '<path d="M8 1.8 l1.6 4.6 L14.2 8 l-4.6 1.6 L8 14.2 l-1.6 -4.6 L1.8 8 l4.6 -1.6z" fill="currentColor" stroke="none"/>',
    "grip": '<g fill="currentColor" stroke="none"><circle cx="6" cy="4.5" r="1.05"/><circle cx="10" cy="4.5" r="1.05"/><circle cx="6" cy="8" r="1.05"/><circle cx="10" cy="8" r="1.05"/><circle cx="6" cy="11.5" r="1.05"/><circle cx="10" cy="11.5" r="1.05"/></g>',
    "plus": '<path d="M8 3.5 v9 M3.5 8 h9"/>',
    "fit": '<path d="M2.5 6 V2.5 H6 M10 2.5 h3.5 V6 M13.5 10 v3.5 H10 M6 13.5 H2.5 V10"/>',
    "iso": '<path d="M8 1.8 l5.4 3.1 v6.2 L8 14.2 l-5.4 -3.1 V4.9z"/><path d="M8 8 l5.4 -3.1 M8 8 v6.2 M8 8 L2.6 4.9"/>',
    "shade": '<circle cx="8" cy="8" r="5.5"/><path d="M8 2.5 a5.5 5.5 0 0 1 0 11z" fill="currentColor" stroke="none"/>',
    "section": '<path d="M2.5 2.5 h11 v11 h-11z"/><path d="M1 8 h14" stroke-dasharray="2 1.6"/>',
    "ruler": '<path d="M2 11 L11 2 l3 3 L5 14z"/><path d="M5 8 l1.5 1.5 M7 6 l1.5 1.5 M9 4 l1.5 1.5"/>',
    "grid": '<path d="M2.5 2.5 h11 v11 h-11z M6.2 2.5 v11 M9.8 2.5 v11 M2.5 6.2 h11 M2.5 9.8 h11"/>',
    "planes": '<path d="M2 11 L6 5 h8 L10 11z"/><path d="M6 5 V2"/>',
    "cursor": '<path d="M3.5 2.5 L12.5 7.5 L8.3 8.8 L6.2 13z"/>',
    "cancel": '<circle cx="8" cy="8" r="5.5"/><path d="M4.2 11.8 L11.8 4.2"/>',
    "mouse": '<rect x="4" y="1.8" width="8" height="12.4" rx="4"/><path d="M8 1.8 v4.4"/>',
    "normal": '<path d="M2 12 L5.5 7 h8.5 L10.5 12z"/><path d="M8 9.5 V2.5"/><path d="M5.8 4.7 L8 2.5 l2.2 2.2"/>',
    "bolt": '<path d="M9 1.5 L3.5 9 H8 L7 14.5 L12.5 7 H8z" fill="currentColor" stroke="none"/>',
    "plug": '<path d="M5.5 2 v3 M10.5 2 v3 M4 5 h8 v2.5 a4 4 0 0 1 -8 0z M8 11.5 V14"/>',
    "check": '<path d="M3 8.5 l3.2 3.2 L13 4.5"/>',
    "lock": '<rect x="3.5" y="7" width="9" height="6.5" rx="1"/><path d="M5.5 7 V5 a2.5 2.5 0 0 1 5 0 v2"/>',
    "menu": '<path d="M2.5 4.5 h11 M2.5 8 h11 M2.5 11.5 h11"/>',
    "drag": '<path d="M8 2 v12 M2 8 h12 M8 2 l-2 2 M8 2 l2 2 M8 14 l-2 -2 M8 14 l2 -2 M2 8 l2 -2 M2 8 l2 2 M14 8 l-2 -2 M14 8 l-2 2"/>',
    "rollback": '<path d="M2 8 h12"/><path d="M5 5 L2 8 l3 3"/>',
    "folder": '<path d="M2 12.5 v-9 h4.5 l1.5 1.5 h6 v7.5z"/>',
    "swap": '<path d="M2.5 5.5 h10 l-2.5 -2.5 M13.5 10.5 h-10 l2.5 2.5"/>',
}


ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT / "crates/soliddraft-app/assets/icons"
NS = 'xmlns="http://www.w3.org/2000/svg"'


def namespaced(key, body):
    """Each file's ids start with its key, so no two assets can share one."""
    body = re.sub(r'id="([^"]+)"', lambda m: f'id="{key}-{m.group(1)}"', body)
    return re.sub(r"url\(#([^)]+)\)", lambda m: f"url(#{key}-{m.group(1)})", body)


def cad_svg(key, fn):
    HOLES.clear()
    body, view = fitted(fn, REACH.get(key, (0.0, 0.0, 0.0, 0.0)))
    return f'<svg {NS} viewBox="{view}">{namespaced(key, body)}</svg>\n'


def line_svg(body):
    return (
        f'<svg {NS} viewBox="0 0 16 16"><g fill="none" stroke="currentColor" stroke-width="1.3" '
        f'stroke-linecap="round" stroke-linejoin="round">{body}</g></svg>\n'
    )


def catalogue_rs():
    rows = ["// Written by scripts/generate_cad_icons.py. Do not edit.", ""]
    for name, family, keys in (("CAD", "cad", CAD), ("GLYPHS", "line", LINE)):
        rows.append(f"pub const {name}: [(&str, &str); {len(keys)}] = [")
        rows += [f'    ("{key}", include_str!("{family}/{key}.svg")),' for key in keys]
        rows.append("];")
    return "\n".join(rows) + "\n"


def files():
    out = {f"cad/{key}.svg": cad_svg(key, fn) for key, fn in CAD.items()}
    out.update({f"line/{key}.svg": line_svg(body) for key, body in LINE.items()})
    out["catalogue.rs"] = catalogue_rs()
    return out


def main():
    wanted = files()
    if "--check" in sys.argv:
        stale = [p for p, text in wanted.items() if not (OUT / p).is_file() or (OUT / p).read_text() != text]
        extra = {p.relative_to(OUT).as_posix() for p in OUT.rglob("*") if p.is_file()} - set(wanted)
        if stale or extra:
            sys.exit(f"stale icon assets: {sorted(stale)} unexpected: {sorted(extra)}")
        return
    for path, text in wanted.items():
        target = OUT / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text)
    print(f"{len(CAD)} cad icons, {len(LINE)} line icons")


if __name__ == "__main__":
    main()
