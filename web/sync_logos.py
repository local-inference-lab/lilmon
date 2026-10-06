#!/usr/bin/env python3
# Copyright 2026 Local Inference Lab, Inc.
# SPDX-License-Identifier: Apache-2.0
"""Write the generated logos into the design artboards and the favicon.

Every logo in the web UI comes from docs/brand/build_logos.py, which holds the Local Inference Lab
reference geometry. Logos sit between <!--logo:NAME--> markers so this script can replace them on
each run. After running it, rebuild the standalone page:

    python3 web/sync_logos.py && python3 web/build_standalone.py
"""
import importlib.util
import re
import shutil
from pathlib import Path

WEB = Path(__file__).resolve().parent
BRAND = WEB.parent / "docs" / "brand"
spec = importlib.util.spec_from_file_location("build_logos", BRAND / "build_logos.py")
logos = importlib.util.module_from_spec(spec)
spec.loader.exec_module(logos)

LABEL = 'role="img" aria-label="{}"'
EDGE = "rgba(255,255,255,0.18)"
MAIN = {
    "header": logos.inline_logo("lm", 132, 38, ink_cls="lg-ink"),
}
BRANDSHEET = {
    "logo-dark": logos.inline_logo("bra", 560, 163, ink="#f7f7fa", attrs=LABEL.format("lilmon logo on the dark background")),
    "logo-light": logos.inline_logo("brc", 400, 116, ink="#050506", attrs=LABEL.format("lilmon logo on the light background")),
    "mark-112": logos.inline_mark("bre", 112, outline=EDGE, attrs=LABEL.format("lilmon mark")),
    "mark-56": logos.inline_mark("brf", 56, attrs=LABEL.format("lilmon mark, 56 pixels")),
    "favicon-32": logos.inline_mark("brh", 32, small=True, attrs=LABEL.format("lilmon favicon, 32 pixels")),
    "favicon-16": logos.inline_mark("bri", 16, small=True, attrs=LABEL.format("lilmon favicon, 16 pixels")),
}


def put(html, name, svg):
    """Replace the marked logo, or on first run the original <svg> that `first_run` locates."""
    pat = re.compile(rf"<!--logo:{name}-->.*?<!--/logo:{name}-->", re.S)
    block = f"<!--logo:{name}-->{svg}<!--/logo:{name}-->"
    if pat.search(html):
        return pat.sub(lambda _: block, html, count=1)
    raise SystemExit(f"sync_logos: marker logo:{name} not found")


def first_run(html, finders):
    """Wrap the original inline SVGs in markers (needed once, before markers exist)."""
    for name, finder in finders.items():
        if f"<!--logo:{name}-->" in html:
            continue
        m = re.search(finder, html, re.S)
        if not m:
            raise SystemExit(f"sync_logos: could not find the original {name} logo")
        html = html[: m.start()] + f"<!--logo:{name}--><!--/logo:{name}-->" + html[m.end():]
    return html


def svg_after(prefix_regex):
    return prefix_regex + r"\s*(<svg\b.*?</svg>)"


def main():
    main_path = WEB / "canvas" / "project" / "Main.dc.html"
    html = main_path.read_text()
    m = re.search(r'(<a class="brand"[^>]*>\s*)(<svg\b.*?</svg>)', html, re.S)
    if m and "<!--logo:header-->" not in html:
        html = html[: m.start(2)] + "<!--logo:header--><!--/logo:header-->" + html[m.end(2):]
    for name, svg in MAIN.items():
        html = put(html, name, svg)
    html = html.replace(".lg-dot{fill:var(--cyan)}", "")
    main_path.write_text(html)

    brand_path = WEB / "canvas" / "project" / "Brand.dc.html"
    html = brand_path.read_text()
    labels = {
        "logo-dark": "lilmon logo on the dark background",
        "logo-light": "lilmon logo on the light background",
        "mark-112": "lilmon mark",
        "mark-56": "lilmon mark, 56 pixels",
        "favicon-32": "lilmon favicon, 32 pixels",
        "favicon-16": "lilmon favicon, 16 pixels",
    }
    html = first_run(html, {n: rf'<svg\b[^>]*aria-label="{re.escape(l)}".*?</svg>' for n, l in labels.items()})
    for name, svg in BRANDSHEET.items():
        html = put(html, name, svg)
    brand_path.write_text(html)

    shutil.copyfile(BRAND / "lilmon-favicon.svg", WEB / "favicon.svg")
    print("synced logos into Main.dc.html, Brand.dc.html and favicon.svg")


if __name__ == "__main__":
    main()
