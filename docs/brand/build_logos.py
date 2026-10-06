#!/usr/bin/env python3
# Copyright 2026 Local Inference Lab, Inc.
# SPDX-License-Identifier: Apache-2.0
"""Generate every lilmon logo file from the Local Inference Lab master mark.

The master geometry below is the single source of truth. It was traced from the lab's official raster
logo (local-inference-lab/web assets/local-inference-lab-logo.jpg, 460 x 460) and checked by
rendering it over the raster: 94% ring overlap and ~88% overlap on the 14 px white strokes, where the
remainder is JPEG edge blur. Never redraw the mark by hand; change it here and rerun:

    python3 docs/brand/build_logos.py
"""
import math
from pathlib import Path

OUT = Path(__file__).resolve().parent

# Brand tokens (local-inference-lab/web styles.css)
BG, INK_DARK, INK_LIGHT = "#050506", "#050506", "#f7f7fa"
VIOLET, BLUE, CYAN = "#7928ff", "#3f6fff", "#18c8e9"

# ---- master mark, 460 x 460 canvas, centered at (230, 230) ----------------------------------------
C, R = 230, 157                 # ring center line radius
RING_W, MARK_W = 15, 14         # stroke widths (round caps; the mark also has round joins)
GAP = 14.4                      # each arc ends 14.4 degrees either side of 12 and 6 o'clock
EX, EY = R * math.sin(math.radians(GAP)), R * math.cos(math.radians(GAP))
RING = (f"M{C-EX:.2f} {C-EY:.2f}A{R} {R} 0 0 0 {C-EX:.2f} {C+EY:.2f}"
        f"M{C+EX:.2f} {C-EY:.2f}A{R} {R} 0 0 1 {C+EX:.2f} {C+EY:.2f}")
MARK = ("M168 163.5H133.5V296.5H168"      # [
        "M292 163.5H326.5V296.5H292"      # ]
        "M230 163.5V296.5"                # |
        "M174 208.5V252.5H195.5"          # left L (foot points right)
        "M270.5 208.5V252.5H291.5")       # right L (foot points right)
# The ring's gradient: two colors, left to right across the ring's own bounding box. That is SVG's
# default objectBoundingBox and the same as CSS `linear-gradient(90deg, START, END)` over the ring's
# box, so it ports anywhere. The pair is the closest two-color fit to the official raster (mean error
# about 3/12/5 levels per channel; the raster's left arc stays violet a little longer).
LOGO_START, LOGO_END = "#8119ea", "#1fb0f2"
# Small-size cut (16-32 px): identical geometry, heavier strokes so the mark survives downsampling.
SMALL_RING_W, SMALL_MARK_W = 28, 24


def gradient(gid):
    """Left-to-right over the referencing shape's bounding box (CSS: linear-gradient(90deg, ...))."""
    return (f'<linearGradient id="{gid}" x2="1">'
            f'<stop offset="0" stop-color="{LOGO_START}"/><stop offset="1" stop-color="{LOGO_END}"/></linearGradient>')


def mark_group(gid, ring_w=RING_W, mark_w=MARK_W, ground="disc", transform=""):
    """The mark in 460-unit coordinates. ground: "disc" (official), "tile" (rounded square) or None."""
    g = {"disc": f'<circle cx="{C}" cy="{C}" r="230" fill="{BG}"/>',
         "tile": f'<rect width="460" height="460" rx="100" fill="{BG}"/>',
         None: ""}[ground]
    t = f' transform="{transform}"' if transform else ""
    return (f'<g{t}>{g}'
            f'<path d="{RING}" fill="none" stroke="url(#{gid})" stroke-width="{ring_w}" stroke-linecap="round"/>'
            f'<path d="{MARK}" fill="none" stroke="#ffffff" stroke-width="{mark_w}" stroke-linecap="round" stroke-linejoin="round"/>'
            f'</g>')


def svg(viewbox, title, body, defs="", style=""):
    st = f"<style>{style}</style>" if style else ""
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{viewbox}" role="img" aria-labelledby="t">'
            f'<title id="t">{title}</title>{st}<defs>{defs}</defs>{body}</svg>\n')


# ---- lilmon wordmark, drawn beside a 128-unit mark -------------------------------------------------
# Monoline strokes at the mark's weight (the mark's 14/460 stroke at 128 px tall is ~3.9; the wordmark
# letters are taller than the bracket group, so they use 7). The "o" is a ring in the mark's gradient.
WORD = ("M160 28V96M186 54V96M212 28V96M238 96V64A13 13 0 0 1 264 64V96"
        "M264 64A13 13 0 0 1 290 64V96M389 96V64A13 13 0 0 1 415 64V96")
WORD_W = 7
S = 128 / 460


def wordmark(gid_o, ink_cls=None, ink=None):
    """The wordmark, inked by CSS classes (`ink_cls` and `ink_cls`+"f") or by a fixed color."""
    stroke = f'class="{ink_cls}"' if ink_cls else f'stroke="{ink}"'
    fill = f'class="{ink_cls}f"' if ink_cls else f'fill="{ink}"'
    return (f'<path {stroke} d="{WORD}" fill="none" stroke-width="{WORD_W}" stroke-linecap="round" stroke-linejoin="round"/>'
            f'<circle {fill} cx="186" cy="35" r="4.5"/>'
            f'<circle cx="340" cy="73" r="23" fill="none" stroke="url(#{gid_o})" stroke-width="{WORD_W}"/>')


def logo(ink_style):
    defs = gradient("ring") + gradient("o")
    body = mark_group("ring", transform=f"scale({S:.6f})") + wordmark("o", ink_cls="ink")
    return svg("0 0 440 128", "lilmon", body, defs, ink_style)


# ---- inline markup for HTML pages (ids are prefixed so several logos can share one page) ----------
def inline_logo(prefix, width, height, ink_cls=None, ink=None, attrs='aria-hidden="true"'):
    defs = gradient(f"{prefix}r") + gradient(f"{prefix}o")
    body = mark_group(f"{prefix}r", transform=f"scale({S:.6f})") + wordmark(f"{prefix}o", ink_cls, ink)
    return f'<svg width="{width}" height="{height}" viewBox="0 0 440 128" {attrs}><defs>{defs}</defs>{body}</svg>'


def inline_mark(prefix, size, small=False, ground="tile", outline=None, attrs='aria-hidden="true"'):
    rw, mw = (SMALL_RING_W, SMALL_MARK_W) if small else (RING_W, MARK_W)
    edge = (f'<rect x="1.5" y="1.5" width="457" height="457" rx="98.5" fill="none" stroke="{outline}" stroke-width="3"/>'
            if outline else "")
    return (f'<svg width="{size}" height="{size}" viewBox="0 0 460 460" {attrs}><defs>{gradient(prefix)}</defs>'
            f'{mark_group(prefix, rw, mw, ground=ground)}{edge}</svg>')


def main():
    files = {
        # The Local Inference Lab master (official form: the mark on a near-black disc).
        "local-inference-lab-mark.svg": svg("0 0 460 460", "Local Inference Lab", mark_group("ring"), gradient("ring")),
        # lilmon app mark: the master on a rounded tile, nothing added.
        "lilmon-mark.svg": svg("0 0 460 460", "lilmon", mark_group("ring", ground="tile"), gradient("ring")),
        # 16-32 px: the small-size cut on the tile.
        "lilmon-favicon.svg": svg("0 0 460 460", "lilmon",
                                  mark_group("ring", SMALL_RING_W, SMALL_MARK_W, ground="tile"), gradient("ring")),
        # Horizontal logos: the mark keeps its dark disc (white strokes) on any background; only the
        # wordmark ink changes.
        "lilmon-logo-dark.svg": logo(f".ink{{stroke:{INK_LIGHT}}}.inkf{{fill:{INK_LIGHT}}}"),
        "lilmon-logo-light.svg": logo(f".ink{{stroke:{INK_DARK}}}.inkf{{fill:{INK_DARK}}}"),
        "lilmon-logo.svg": logo(f".ink{{stroke:{INK_DARK}}}.inkf{{fill:{INK_DARK}}}"
                                f"@media (prefers-color-scheme:dark){{.ink{{stroke:{INK_LIGHT}}}.inkf{{fill:{INK_LIGHT}}}}}"),
    }
    for name, text in files.items():
        (OUT / name).write_text(text)
        print(f"wrote {name} ({len(text)} bytes)")


if __name__ == "__main__":
    main()
