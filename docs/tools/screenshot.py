#!/usr/bin/env python3
# Copyright 2026 Local Inference Lab, Inc.
# SPDX-License-Identifier: Apache-2.0
"""Render lilmon frames to PNG screenshots for the README.

Frames come from lilmon's hidden snapshot mode, which emits one JSON array of
[symbol, fg, bg, bold] cells per row:

    lilmon --snapshot 300 --size 200x56 --snapshot-out /tmp/frames \
           --snapshot-themes graphite,nord,synthwave
    python3 docs/tools/screenshot.py window /tmp/frames/graphite.jsonl docs/screenshots/dashboard.png
    python3 docs/tools/screenshot.py gallery docs/screenshots/themes.png /tmp/frames/{nord,synthwave}.jsonl

For an animation, add --snapshot-every 1 to write frame_NNNN.jsonl once a second, then:

    python3 docs/tools/screenshot.py animate docs/screenshots/live.webp 880 250 /tmp/frames/frame_00[0-9][02468].jsonl

Box-drawing and block characters are drawn as geometry so borders and bars join seamlessly; everything
else uses DejaVu Sans Mono. Requires Pillow.
"""
import json
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter, ImageFont

FONT_DIR = Path("/usr/share/fonts/truetype/dejavu")
FS = 16
FONT = ImageFont.truetype(str(FONT_DIR / "DejaVuSansMono.ttf"), FS)
BOLD = ImageFont.truetype(str(FONT_DIR / "DejaVuSansMono-Bold.ttf"), FS)
CW, CH = 10, 21

LOWER = {"▁": 1, "▂": 2, "▃": 3, "▄": 4, "▅": 5, "▆": 6, "▇": 7, "█": 8}
LEFT = {"▏": 1, "▎": 2, "▍": 3, "▌": 4, "▋": 5, "▊": 6, "▉": 7}


def hexrgb(h, default=(26, 26, 25)):
    if not h:
        return default
    h = h.lstrip("#")
    return tuple(int(h[i : i + 2], 16) for i in (0, 2, 4))


def mix(a, b, t):
    return tuple(round(x + (y - x) * t) for x, y in zip(a, b))


def render_cells(rows):
    w, h = len(rows[0]), len(rows)
    bg0 = hexrgb(rows[0][0][2])
    img = Image.new("RGB", (w * CW, h * CH), bg0)
    d = ImageDraw.Draw(img)
    for y, row in enumerate(rows):
        for x, (sym, fg, bg, bold) in enumerate(row):
            X, Y = x * CW, y * CH
            if bg:
                d.rectangle([X, Y, X + CW - 1, Y + CH - 1], fill=hexrgb(bg))
            col = hexrgb(fg, (236, 236, 232))
            cx, cy = X + CW // 2, Y + CH // 2
            r = CW // 2
            if sym in LOWER:
                n = LOWER[sym]
                d.rectangle([X, Y + CH - CH * n // 8, X + CW - 1, Y + CH - 1], fill=col)
            elif sym in LEFT:
                d.rectangle([X, Y, X + CW * LEFT[sym] // 8 - 1, Y + CH - 1], fill=col)
            elif sym == "▀":
                d.rectangle([X, Y, X + CW - 1, Y + CH // 2 - 1], fill=col)
            elif sym == "─":
                d.line([X, cy, X + CW, cy], fill=col)
            elif sym == "│":
                d.line([cx, Y, cx, Y + CH], fill=col)
            elif sym == "┈":
                for i in range(0, CW, 5):
                    d.line([X + i, cy, X + i + 1, cy], fill=col)
            elif sym == "┴":
                d.line([X, cy, X + CW, cy], fill=col)
                d.line([cx, Y, cx, cy], fill=col)
            elif sym == "╭":
                d.arc([cx, cy, cx + 2 * r, cy + 2 * r], 180, 270, fill=col)
                d.line([cx + r, cy, X + CW, cy], fill=col)
                d.line([cx, cy + r, cx, Y + CH], fill=col)
            elif sym == "╮":
                d.arc([cx - 2 * r, cy, cx, cy + 2 * r], 270, 360, fill=col)
                d.line([X, cy, cx - r, cy], fill=col)
                d.line([cx, cy + r, cx, Y + CH], fill=col)
            elif sym == "╰":
                d.arc([cx, cy - 2 * r, cx + 2 * r, cy], 90, 180, fill=col)
                d.line([cx, Y, cx, cy - r], fill=col)
                d.line([cx + r, cy, X + CW, cy], fill=col)
            elif sym == "╯":
                d.arc([cx - 2 * r, cy - 2 * r, cx, cy], 0, 90, fill=col)
                d.line([cx, Y, cx, cy - r], fill=col)
                d.line([X, cy, cx - r, cy], fill=col)
            elif sym.strip():
                d.text((X, Y + (CH - FS) // 2 - 1), sym, font=BOLD if bold else FONT, fill=col)
    return img, bg0


def window(rows, title="lilmon"):
    """Frame a rendered terminal in a window with a title bar, drop shadow and backdrop."""
    term, bg = render_cells(rows)
    light = sum(bg) > 3 * 160
    pad, bar, radius = 56, 36, 14
    tw, th = term.size
    W, H = tw + 2 * pad, th + bar + 2 * pad
    top, bottom = ((232, 238, 247), (200, 210, 226)) if light else ((44, 52, 68), (14, 17, 24))
    out = Image.new("RGB", (W, H))
    grad = ImageDraw.Draw(out)
    for yy in range(H):
        grad.line([(0, yy), (W, yy)], fill=mix(top, bottom, yy / H))
    # shadow
    sh = Image.new("L", (W, H), 0)
    ImageDraw.Draw(sh).rounded_rectangle([pad, pad + 18, pad + tw, pad + bar + th + 18], radius, fill=150)
    sh = sh.filter(ImageFilter.GaussianBlur(26))
    out.paste(Image.new("RGB", (W, H), (0, 0, 0)), (0, 0), sh)
    # window body + title bar
    win = Image.new("RGB", (tw, th + bar), bg)
    wd = ImageDraw.Draw(win)
    barc = mix(bg, (0, 0, 0) if light else (255, 255, 255), 0.06)
    wd.rectangle([0, 0, tw, bar], fill=barc)
    wd.line([0, bar - 1, tw, bar - 1], fill=mix(bg, (0, 0, 0) if light else (255, 255, 255), 0.12))
    for i, c in enumerate([(255, 95, 87), (254, 188, 46), (40, 200, 64)]):
        x0 = 18 + i * 22
        wd.ellipse([x0, bar // 2 - 6, x0 + 12, bar // 2 + 6], fill=c)
    tf = ImageFont.truetype(str(FONT_DIR / "DejaVuSans.ttf"), 14)
    tl = wd.textlength(title, font=tf)
    wd.text(((tw - tl) / 2, bar // 2 - 8), title, font=tf, fill=mix(barc, (128, 128, 128) if light else (200, 200, 200), 0.8))
    win.paste(term, (0, bar))
    mask = Image.new("L", win.size, 0)
    ImageDraw.Draw(mask).rounded_rectangle([0, 0, tw - 1, th + bar - 1], radius, fill=255)
    out.paste(win, (pad, pad), mask)
    return out


def save(img, path):
    Path(path).parent.mkdir(parents=True, exist_ok=True)
    img.save(path, optimize=True)
    print(f"{path}: {img.size[0]}x{img.size[1]}, {Path(path).stat().st_size // 1024} KB")


def load(path):
    return [json.loads(line) for line in open(path) if line.strip()]


def main():
    mode = sys.argv[1]
    if mode == "window":
        src, dst = sys.argv[2], sys.argv[3]
        title = sys.argv[4] if len(sys.argv) > 4 else "lilmon"
        save(window(load(src), title), dst)
    elif mode == "crop":
        # crop SRC DST x0 y0 x1 y1 (fractions of the framed image)
        src, dst = sys.argv[2], sys.argv[3]
        img = window(load(src))
        x0, y0, x1, y1 = (float(v) for v in sys.argv[4:8])
        W, H = img.size
        save(img.crop((int(W * x0), int(H * y0), int(W * x1), int(H * y1))), dst)
    elif mode == "gallery":
        dst, srcs = sys.argv[2], sys.argv[3:]
        cols, scale, gap = 2, 0.5, 28
        label = ImageFont.truetype(str(FONT_DIR / "DejaVuSans-Bold.ttf"), 22)
        thumbs = []
        for s in srcs:
            im = window(load(s), f"lilmon · {Path(s).stem}")
            thumbs.append((Path(s).stem, im.resize((int(im.width * scale), int(im.height * scale)), Image.LANCZOS)))
        tw, th = thumbs[0][1].size
        rows = (len(thumbs) + cols - 1) // cols
        out = Image.new("RGB", (cols * tw + (cols + 1) * gap, rows * (th + 40) + gap), (22, 25, 31))
        d = ImageDraw.Draw(out)
        for i, (name, im) in enumerate(thumbs):
            x = gap + (i % cols) * (tw + gap)
            y = gap + (i // cols) * (th + 40)
            out.paste(im, (x, y))
            d.text((x + 30, y + th - 6), name, font=label, fill=(220, 224, 232))
        save(out, dst)
    elif mode == "animate":
        # animate DST.(png|gif|webp) WIDTH MS_PER_FRAME FRAME.jsonl...
        dst, width, ms, srcs = sys.argv[2], int(sys.argv[3]), int(sys.argv[4]), sys.argv[5:]
        frames = []
        for s in srcs:
            im = window(load(s), "lilmon")
            frames.append(im.resize((width, round(im.height * width / im.width)), Image.LANCZOS))
        Path(dst).parent.mkdir(parents=True, exist_ok=True)
        ext = Path(dst).suffix.lower()
        if ext == ".gif":
            # one shared palette keeps colors stable from frame to frame
            pal = frames[len(frames) // 2].quantize(colors=255, method=Image.Quantize.MEDIANCUT)
            frames = [f.quantize(palette=pal, dither=Image.Dither.NONE) for f in frames]
            frames[0].save(dst, save_all=True, append_images=frames[1:], duration=ms, loop=0, optimize=False, disposal=1)
        elif ext == ".webp":
            # quality 80 at README width (880px) keeps text crisp; every 2nd 1 s frame at 250 ms is ~1 MB
            frames[0].save(dst, save_all=True, append_images=frames[1:], duration=ms, loop=0, lossless=False, quality=80, method=6)
        else:
            frames[0].save(dst, save_all=True, append_images=frames[1:], duration=ms, loop=0, optimize=True)
        print(f"{dst}: {len(frames)} frames, {frames[0].size[0]}x{frames[0].size[1]}, {Path(dst).stat().st_size // 1024} KB")
    else:
        sys.exit(f"unknown mode {mode}")


if __name__ == "__main__":
    main()
