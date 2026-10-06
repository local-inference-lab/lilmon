# lilmon web UI

A browser version of the lilmon dashboard in the Local Inference Lab brand system (defined in
[local-inference-lab/web `AGENTS.md`](https://github.com/local-inference-lab/web/blob/main/AGENTS.md)).
`lilmon --serve` serves it with live data; see the main [README](../README.md#web-ui) for usage.

It mirrors the TUI's sections:

- decode and prefill rates with 1/5/15 minute peaks and a mean-to-max envelope,
- sessions and KV cache,
- MTP acceptance,
- latency percentiles,
- GPU and host telemetry,
- recent requests.

## How it works

The page is a single file embedded in the lilmon binary. It polls `GET /api/state` once a second:

```
/api/state?ep=0&window=300&cols=100&spark=60
```

| parameter | meaning |
|---|---|
| `ep` | endpoint index, as in the TUI's tabs |
| `window` | chart window in seconds (60 to 86400) |
| `cols` | chart columns; lilmon bins whole seconds anchored to the clock, exactly as in the TUI |
| `spark` | columns for the sparklines (concurrency, accept length, GPU history) |

`src/serve.rs` builds the response from the same state functions the TUI draws with: peaks and means
over 1-second buckets, prefill back-filled over TTFT, MTP per-position rates, latency histogram
percentiles, NVML and `/proc` samples, and LiteLLM rows. Numbers are raw (tokens per second, seconds,
bytes); the page formats them the way the TUI does. Top-level keys:

| key | contents |
|---|---|
| `endpoints`, `sel` | every endpoint's name, URL, status (`live`, `stale`, `down`, `auth`, `connecting`), model, context, KV dtype and capacity, uptime |
| `decode`, `prefill` | now, peaks, window average, per-request and per-sequence rates, records, chart columns `[mean, max, seen]`, prefill-in-flight state and the last prefill |
| `sessions` | running, waiting, prefilling and decoding counts, KV usage and peak, prefix hits, finish reasons, preemptions, concurrency spark |
| `mtp` | accept length and rate, mean k, rates, per-position acceptance, accept-length spark |
| `latency` | p50, p90 and p99 rows, per-request decode and prefill speed, mean prompt |
| `gpus`, `host` | local telemetry, or `null` when unavailable |
| `requests` | LiteLLM host, how rows are matched, recent rows, or the error |

Keys and credentials never reach the page.

## Files

| path | role |
|---|---|
| `canvas/project/Main.dc.html` | source of truth: the dashboard as a design-canvas artboard |
| `canvas/project/{Light,Phone,Brand}.dc.html` | light theme, 390 px phone frame and brand sheet artboards |
| `index.html` | the page lilmon embeds, generated from `Main.dc.html` by `build_standalone.py` |
| `dc-shim.js` | renders the artboard markup outside the canvas runtime; also embedded |
| `favicon.svg` | the small-size cut of the mark; also embedded |
| `sync_logos.py` | writes the generated logos (from `docs/brand/build_logos.py`) into the artboards and `favicon.svg` |
| `check_page.py` | browser check against a running `lilmon --serve`: overflow, h1, console errors, controls, and values that must match `/api/state` |

## Changing the page

Edit `canvas/project/Main.dc.html`, regenerate the page, then rebuild lilmon so the new page is embedded:

```sh
python3 web/sync_logos.py          # only after editing docs/brand/build_logos.py
python3 web/build_standalone.py
cargo build --release
./target/release/lilmon --serve --headless &
uv run --no-project --with playwright python web/check_page.py /tmp/shots http://127.0.0.1:7878/
```

Opened straight from disk (or in the design canvas) the page has no `/api/state` to read, so it shows a
"not connected" panel with the command to run instead of numbers.
