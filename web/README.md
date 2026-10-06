# lilmon web UI (design)

A browser version of the lilmon dashboard in the Local Inference Lab brand system (defined in
[local-inference-lab/web `AGENTS.md`](https://github.com/local-inference-lab/web/blob/main/AGENTS.md)).
Logos come from `docs/brand/build_logos.py`; `web/sync_logos.py` writes them into the artboards. It
mirrors the TUI's sections:

- decode and prefill rates with 1/5/15 minute peaks and a mean-to-max envelope,
- sessions and KV cache,
- MTP acceptance,
- latency percentiles,
- GPU and host telemetry,
- recent requests.

It runs on recorded data from a real vLLM server, about an hour of per-second history followed by a
258 s load test, and replays it once a second.

## Open it

Open `index.html` in a browser. No server or build step is needed. URL parameters set the starting state:
`index.html?theme=lil-light&window=1h` (themes `lil`, `lil-light`, `graphite`, `synthwave`; windows
`1m 5m 15m 1h 6h 24h`; `animate=false` stops the replay).

## Files

| path | role |
|---|---|
| `canvas/project/Main.dc.html` | source of truth: the dashboard as a design-canvas artboard |
| `canvas/project/{Light,Phone,Brand}.dc.html` | light theme, 390 px phone frame and brand sheet artboards |
| `index.html` | standalone page generated from `Main.dc.html` by `build_standalone.py` |
| `dc-shim.js` | renders the artboard markup outside the canvas runtime |
| `sample-data.js` | the recorded sample data |
| `check_page.py` | Playwright check at 320 to 1440 px: overflow, h1, console errors, controls, tooltip, themes |

Edit `canvas/project/Main.dc.html`, then run `python3 web/build_standalone.py`.

## Wiring it to live data

The page reads one compact JSON document. A live version needs lilmon to serve the same numbers it
already computes:

1. Add an opt-in `--serve 127.0.0.1:7878` HTTP listener to lilmon that serves the page and a JSON
   snapshot. The snapshot would hold the per-second buckets (decode, prefill, running, KV, accept
   length), the latency histogram percentiles, MTP per-position rates, the GPU and host samples, and the
   LiteLLM rows.
2. Replace the replay in `componentDidMount` with a poll (or server-sent events) every `interval_ms`.
   The chart binning already anchors to absolute seconds, so history scrolls without reshaping.
3. Keep the listener on loopback by default. Reuse the endpoint's API key handling for any remote use,
   and never expose `/metrics` credentials to the page.
