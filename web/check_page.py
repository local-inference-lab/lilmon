#!/usr/bin/env python3
# Copyright 2026 Local Inference Lab, Inc.
# SPDX-License-Identifier: Apache-2.0
"""Browser check for the web UI served by `lilmon --serve`, at the site breakpoints (320 to 1440 px).

Reports horizontal overflow, h1 count, console errors and the smallest control size, and checks that
what the page shows comes from /api/state: the model name, the endpoint tabs and the GPU count. It
exercises a chart tooltip and the window and theme buttons, confirms the not-connected panel when
the page is opened from disk, and saves screenshots to the given folder:

    uv run --no-project --with playwright python -m playwright install chromium
    lilmon --serve &
    uv run --no-project --with playwright python web/check_page.py /tmp/shots [http://127.0.0.1:7878/]
"""
import asyncio, json, sys, urllib.request
from playwright.async_api import async_playwright
from pathlib import Path
V = sys.argv[1] if len(sys.argv) > 1 else '.'
URL = sys.argv[2] if len(sys.argv) > 2 else 'http://127.0.0.1:7878/'
FILE = (Path(__file__).resolve().parent / 'index.html').as_uri()
def api():
    with urllib.request.urlopen(URL.rstrip('/') + '/api/state?ep=0&window=300&cols=100&spark=60', timeout=5) as r:
        return json.load(r)


async def main():
    d = api()
    want = {'model': d['endpoints'][0]['model'], 'tabs': [e['name'] for e in d['endpoints']], 'gpus': len(d['gpus'] or [])}
    print('api:', want)
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
                model: document.querySelector('.meta b') && document.querySelector('.meta b').textContent,
                tabs: [...document.querySelectorAll('.tabs .tab')].map(t => t.childNodes[1] ? t.childNodes[1].textContent : t.textContent),
                gpus: document.querySelectorAll('.gpu').length,
                status: document.querySelector('.live') && document.querySelector('.live').textContent.trim(),
            })''')
            got = {'model': info['model'], 'tabs': info['tabs'], 'gpus': info['gpus']}
            assert got == want, f'page shows {got}, /api/state says {want}'
            await pg.wait_for_timeout(2200)
            clock2 = await pg.evaluate("document.querySelector('.clock').textContent")
            print(f"w={w:5d} overflow={info['overflow']} h1={info['h1']} cols={info['cols']} now={info['decNow']} clock {info['clock']}->{clock2} minTarget={info['minTarget']:.0f}px height={info['height']} status={info['status']!r} errors={errs[:3]}")
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
        # opened from disk there is no /api/state: the page must say so instead of showing numbers
        pg = await b.new_page(viewport={'width': 1024, 'height': 800})
        await pg.goto(FILE); await pg.wait_for_timeout(1500)
        off = await pg.evaluate("({tag: document.querySelector('.empty .tag') && document.querySelector('.empty .tag').textContent.trim(), h2: document.querySelector('.empty h2') && document.querySelector('.empty h2').textContent, panels: document.querySelectorAll('.big').length})")
        print('from disk:', off)
        assert off['tag'] == 'Not connected' and off['panels'] == 0, off
        await pg.screenshot(path=f'{V}/shot-disconnected.png')
        await pg.close()
        await b.close()
asyncio.run(main())
