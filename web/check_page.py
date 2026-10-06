#!/usr/bin/env python3
# Copyright 2026 Local Inference Lab, Inc.
# SPDX-License-Identifier: Apache-2.0
"""Browser check for web/index.html at the site breakpoints (320 to 1440 px).

Reports horizontal overflow, h1 count, console errors, live updates and the smallest control size,
exercises a chart tooltip, the window and theme buttons, and saves screenshots to the given folder:

    uv run --no-project --with playwright python -m playwright install chromium
    uv run --no-project --with playwright python web/check_page.py /tmp/shots
"""
import asyncio, sys
from playwright.async_api import async_playwright
from pathlib import Path
URL = (Path(__file__).resolve().parent / 'index.html').as_uri()
V = sys.argv[1] if len(sys.argv) > 1 else '.'
async def main():
    async with async_playwright() as p:
        b = await p.chromium.launch()
        for w in (320, 390, 640, 768, 960, 1024, 1440):
            pg = await b.new_page(viewport={'width': w, 'height': 900})
            errs = []
            pg.on('console', lambda m: errs.append(m.text) if m.type == 'error' else None)
            pg.on('pageerror', lambda e: errs.append(str(e)))
            await pg.goto(URL)
            await pg.wait_for_timeout(1500)
            info = await pg.evaluate('''() => ({
                overflow: document.documentElement.scrollWidth - document.documentElement.clientWidth,
                h1: document.querySelectorAll('h1').length,
                cols: document.querySelectorAll('.cols .col').length,
                clock: document.querySelector('.clock') && document.querySelector('.clock').textContent,
                decNow: document.querySelector('.big .v') && document.querySelector('.big .v').textContent,
                minTarget: Math.min(...[...document.querySelectorAll('.seg button,.tab')].map(e => Math.min(e.getBoundingClientRect().width, e.getBoundingClientRect().height))),
                height: document.documentElement.scrollHeight,
            })''')
            await pg.wait_for_timeout(2200)
            clock2 = await pg.evaluate("document.querySelector('.clock').textContent")
            print(f"w={w:5d} overflow={info['overflow']} h1={info['h1']} cols={info['cols']} now={info['decNow']} clock {info['clock']}->{clock2} minTarget={info['minTarget']:.0f}px height={info['height']} errors={errs[:3]}")
            if w in (390, 1440):
                await pg.screenshot(path=f'{V}/shot-{w}.png', full_page=True)
            if w == 1440:
                col = pg.locator('.s-decode .col').nth(60)
                await col.hover(); await pg.wait_for_timeout(300)
                tip = await pg.evaluate("document.querySelector('.tip') && document.querySelector('.tip').textContent")
                await pg.get_by_role('button', name='1h').click(); await pg.wait_for_timeout(300)
                pressed = await pg.evaluate("[...document.querySelectorAll('.seg button[aria-pressed=true]')].map(b => b.textContent.trim())")
                await pg.get_by_role('button', name='lil light theme').click(); await pg.wait_for_timeout(400)
                bg = await pg.evaluate("getComputedStyle(document.querySelector('.lm')).backgroundColor")
                ticks = await pg.evaluate("[...document.querySelectorAll('.s-decode')].length && [...document.querySelectorAll('.tick')].slice(0,6).map(t=>t.textContent)")
                await pg.screenshot(path=f'{V}/shot-1440-light.png', full_page=False)
                print('tooltip:', tip, '| pressed:', pressed, '| light bg:', bg, '| ticks:', ticks)
            await pg.close()
        await b.close()
asyncio.run(main())
