// Copyright 2026 Local Inference Lab, Inc.
// SPDX-License-Identifier: Apache-2.0

//! `lilmon --serve`: a small HTTP server for the web UI. It serves the page (embedded in the binary)
//! and `/api/state`, a JSON view of the same numbers the TUI draws, computed by the same functions.
//! It binds to loopback unless told otherwise; the page never sees endpoint credentials.

use crate::prom::H;
use crate::state::{Field, now};
use crate::ui::{Timeline, hist_columns, series_columns};
use crate::{App, Endpoint, WINDOWS};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

const PAGE: &str = include_str!("../web/index.html");
const SHIM: &str = include_str!("../web/dc-shim.js");
const FAVICON: &str = include_str!("../web/favicon.svg");

pub fn spawn(app: Arc<Mutex<App>>, addr: String) -> anyhow::Result<()> {
    let server = tiny_http::Server::http(&addr).map_err(|e| anyhow::anyhow!("--serve {addr}: {e}"))?;
    std::thread::Builder::new().name("serve".into()).spawn(move || {
        for req in server.incoming_requests() {
            let url = req.url().to_string();
            let (path, query) = url.split_once('?').unwrap_or((url.as_str(), ""));
            let (status, ctype, body) = match path {
                "/" | "/index.html" => (200, "text/html; charset=utf-8", PAGE.to_string()),
                "/dc-shim.js" => (200, "text/javascript; charset=utf-8", SHIM.to_string()),
                "/favicon.svg" => (200, "image/svg+xml", FAVICON.to_string()),
                "/api/state" => {
                    let q = |k: &str| query.split('&').find_map(|kv| kv.strip_prefix(k).and_then(|v| v.strip_prefix('=')));
                    let ep = q("ep").and_then(|v| v.parse().ok()).unwrap_or(0usize);
                    let win = q("window").and_then(|v| v.parse().ok()).unwrap_or(300i64).clamp(60, 86400);
                    let cols = q("cols").and_then(|v| v.parse().ok()).unwrap_or(120u16).clamp(10, 600);
                    let spark = q("spark").and_then(|v| v.parse().ok()).unwrap_or(60u16).clamp(10, 300);
                    let v = match app.lock() {
                        Ok(a) => state_json(&a, ep, win, cols, spark),
                        Err(_) => json!({"error": "lilmon state unavailable"}),
                    };
                    (200, "application/json", v.to_string())
                }
                _ => (404, "text/plain; charset=utf-8", "not found".to_string()),
            };
            let header = |k: &str, v: &str| tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()).expect("static header");
            let resp = tiny_http::Response::from_string(body)
                .with_status_code(status)
                .with_header(header("Content-Type", ctype))
                .with_header(header("Cache-Control", "no-store"))
                .with_header(header("X-Content-Type-Options", "nosniff"));
            let _ = req.respond(resp);
        }
    })?;
    Ok(())
}

fn opt<T: Into<Value>>(v: Option<T>) -> Value {
    v.map(Into::into).unwrap_or(Value::Null)
}

/// Connection state of an endpoint, with the same rules and text as the TUI header.
fn status(ep: &Endpoint, interval_s: f64) -> (&'static str, String) {
    let t = now();
    let age = ep.last_ok.map(|l| t - l);
    match (&ep.err, age) {
        (Some((e, _)), a) if a.is_none_or(|a| a > 3.0 * interval_s) => {
            if e.starts_with("401 ") || e.starts_with("403 ") {
                ("auth", e.chars().take(72).collect())
            } else {
                ("down", e.chars().take(72).collect())
            }
        }
        (_, Some(a)) if a > 3.0 * interval_s => ("stale", format!("no scrape for {:.0}s", a)),
        (_, Some(_)) => ("live", format!("scraped every {} ms, last took {:.0} ms", (interval_s * 1000.0).round(), ep.scrape_ms)),
        _ => ("connecting", "waiting for the first scrape".into()),
    }
}

fn cols_json(cols: &[crate::ui::widgets::Col]) -> Value {
    Value::Array(cols.iter().map(|c| json!([c.mean, c.max, c.seen])).collect())
}

fn ctx(n: u64) -> String {
    crate::ui::widgets::ctx(n)
}

/// The full state the page renders, for endpoint `epi` over a `win`-second window.
pub fn state_json(app: &App, epi: usize, win: i64, cols: u16, spark_w: u16) -> Value {
    let interval_s = app.interval_ms.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1000.0;
    let endpoints: Vec<Value> = app
        .eps
        .iter()
        .map(|e| {
            let (kind, text) = status(e, interval_s);
            let s = e.st.last();
            json!({
                "name": e.cfg.name,
                "url": e.cfg.url,
                "status": kind,
                "status_text": text,
                "model": e.model(),
                "ctx": opt(e.info.as_ref().and_then(|i| i.max_model_len).map(ctx)),
                "kv_dtype": opt(s.and_then(|s| s.cache_info.get("cache_dtype").cloned())),
                "kv_capacity": opt(s.and_then(|s| s.kv_capacity_tokens())),
                "uptime_s": opt(s.and_then(|s| s.process_start).map(|p| now() - p)),
                "has_data": s.is_some(),
            })
        })
        .collect();
    let epi = epi.min(app.eps.len().saturating_sub(1));
    let mut out = json!({
        "now": now(),
        "version": env!("CARGO_PKG_VERSION"),
        "endpoints": endpoints,
        "sel": epi,
        "window": win,
        "windows": WINDOWS.iter().map(|w| w.0).collect::<Vec<_>>(),
        "peaks": app.peaks.iter().map(|(s, l)| json!([s, l])).collect::<Vec<_>>(),
    });
    let Some(ep) = app.eps.get(epi) else { return out };
    let st = &ep.st;
    let Some(s) = st.last() else { return out };
    let end = st.complete_sec();
    let o = out.as_object_mut().expect("object");
    let rec = app.records.get(&ep.record_key());

    // throughput
    let tl = Timeline::new(win, cols, end);
    let peaks = |f: Field| -> Vec<Value> { app.peaks.iter().map(|(secs, l)| json!([l, opt(st.peak(f, *secs).map(|p| p.0))])).collect() };
    let lat = st.latency_window((win as f64).min(3600.0));
    let decode_now = st.mean(Field::Decode, app.cfg.ui.decode_now_s.max(1) as i64);
    let running = s.running;
    o.insert("decode".into(), json!({
        "now": opt(decode_now),
        "peaks": peaks(Field::Decode),
        "avg": opt(st.mean(Field::Decode, win)),
        "per_req": opt(lat.per_req_decode),
        "per_seq": opt(st.mean(Field::Decode, 2).filter(|_| running > 0.0).map(|v| v as f64 / running)),
        "running": running,
        "record": opt(rec.map(|r| r.decode.v).filter(|v| *v > 0.0)),
        "cols": cols_json(&series_columns(st, Field::Decode, &tl)),
    }));
    let w = st.sum_window(win);
    let last = st.prefill_events.iter().rev().find(|e| e.tokens >= 8192.0).or(st.prefill_events.back());
    o.insert("prefill".into(), json!({
        "now": opt(st.mean(Field::Prefill, app.cfg.ui.prefill_now_s.max(1) as i64)),
        "peaks": peaks(Field::Prefill),
        "avg": opt(st.mean(Field::Prefill, win)),
        "cached_rate": w.cached as f64 / win as f64,
        "hit": opt((w.prefix_q > 0.0).then(|| w.prefix_h as f64 / w.prefix_q as f64)),
        "prefilling": st.prefilling,
        "prefilling_for": opt(st.prefill_since.filter(|_| st.prefilling > 0.0).map(|t| now() - t)),
        "last": opt(last.map(|e| json!({"tokens": e.tokens, "secs": e.secs, "rate": e.tokens / e.secs, "ago": now() - e.t}))),
        "record": opt(rec.map(|r| r.prefill.v).filter(|v| *v > 0.0)),
        "cols": cols_json(&series_columns(st, Field::Prefill, &tl)),
    }));

    // sessions
    let sp = Timeline::new(win, spark_w, end);
    let conc: Vec<Value> = series_columns(st, Field::Running, &sp).iter().map(|c| if c.seen { json!(c.max) } else { Value::Null }).collect();
    let mut reasons: Vec<(&String, &f64)> = s.finished.iter().filter(|(k, v)| **v > 0.0 || k.as_str() == "stop").collect();
    reasons.sort_by_key(|(k, _)| (k.as_str() != "stop", k.as_str()));
    o.insert("sessions".into(), json!({
        "running": running,
        "waiting": s.waiting,
        "deferred": s.waiting_deferred,
        "prefilling": st.prefilling,
        "decoding": st.decoding,
        "kv": s.kv_usage,
        "kv_peak": opt(st.peak(Field::Kv, win).map(|p| p.0)),
        "kv_capacity": opt(s.kv_capacity_tokens()),
        "hit": opt((w.prefix_q > 0.0).then(|| w.prefix_h as f64 / w.prefix_q as f64)),
        "hit_life": opt((s.prefix_queries > 0.0).then(|| s.prefix_hits / s.prefix_queries)),
        "finished": w.finished,
        "per_min": w.finished as f64 / (win as f64 / 60.0),
        "reasons": reasons.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(),
        "preemptions": s.preemptions,
        "conc": conc,
    }));

    // MTP, with the TUI's lifetime fallback when the window has no drafts
    let mut spec = st.spec_window(win);
    let mut lifetime = false;
    if spec.drafts < 1.0 {
        spec = st.spec_lifetime();
        lifetime = true;
    }
    let per_s = |n: f64| if lifetime { Value::Null } else { json!(n / win as f64) };
    let al: Vec<Value> = sp
        .columns(|a, b| {
            let (mut d, mut acc) = (0.0f32, 0.0f32);
            for t in a..b {
                if let Some(bk) = st.series.get(t) {
                    d += bk.drafts;
                    acc += bk.accepted;
                }
            }
            (d > 0.0).then(|| 1.0 + acc / d)
        })
        .into_iter()
        .map(opt)
        .collect();
    o.insert("mtp".into(), json!({
        "enabled": s.has_spec,
        "lifetime": lifetime,
        "accept_len": opt(spec.accept_len()),
        "accept_rate": opt(spec.accept_rate()),
        "mean_k": opt(spec.mean_k()),
        "drafts_per_s": per_s(spec.drafts),
        "accepted_per_s": per_s(spec.accepted),
        "drafted_per_s": per_s(spec.draft_tokens),
        "exact": spec.pos_exact,
        "pos": spec.pos_rate.iter().map(|r| opt(*r)).collect::<Vec<_>>(),
        "k_max": s.num_pos,
        "spark": al,
    }));

    // latency
    let rows: Vec<Value> = [("TTFT", H::Ttft), ("ITL per step", H::Itl), ("TPOT", H::Tpot), ("E2E", H::E2e), ("Queue", H::Queue), ("Prefill", H::PrefillTime)]
        .iter()
        .map(|(n, h)| {
            let hh = lat.hist.get(h);
            let q = |p: f64| opt(hh.and_then(|x| x.quantile(p)));
            json!([n, q(0.5), q(0.9), q(0.99)])
        })
        .collect();
    o.insert("latency".into(), json!({
        "secs": lat.secs,
        "requests": lat.requests,
        "rows": rows,
        "decode_req": opt(lat.per_req_decode),
        "prefill_req": opt(lat.per_req_prefill),
        "mean_prompt": opt(lat.prompt_mean),
    }));

    // GPUs and host describe the machine lilmon runs on
    let gwin = win.min(3600);
    let gpus: Value = if !app.gpu.enabled || app.gpu.unavailable {
        Value::Null
    } else {
        let mut gs: Vec<_> = app.gpu.samples.iter().collect();
        gs.sort_by_key(|g| std::cmp::Reverse(g.mem_total.unwrap_or(0)));
        let gend = app.gpu.t.floor() as i64;
        Value::Array(
            gs.iter()
                .map(|g| {
                    let hist: Vec<Value> = app
                        .gpu
                        .util
                        .get(&g.index)
                        .map(|h| hist_columns(h, &Timeline::new(gwin, spark_w, gend)).iter().map(|c| if c.seen { json!(c.max) } else { Value::Null }).collect())
                        .unwrap_or_default();
                    json!({
                        "index": g.index, "name": g.name, "util": opt(g.util), "mem_util": opt(g.mem_util),
                        "mem_used": opt(g.mem_used), "mem_total": opt(g.mem_total), "sm": opt(g.sm_clock), "sm_max": opt(g.sm_clock_max),
                        "temp": opt(g.temp), "power": opt(g.power_w), "power_limit": opt(g.power_limit_w), "hist": hist,
                    })
                })
                .collect(),
        )
    };
    o.insert("gpus".into(), gpus);
    let host = match (&app.host.last, app.host.enabled && !app.host.unavailable) {
        (Some(h), true) => {
            let kernel = h.mem_total.saturating_sub(h.anon + h.shmem + h.cache + h.mem_free);
            json!({
                "hostname": h.hostname, "cpu": h.cpu, "iowait": h.iowait, "cores": h.cores, "load": h.load,
                "mem_total": h.mem_total, "mem_used": h.mem_used(), "mem_avail": h.mem_avail, "mem_free": h.mem_free,
                "anon": h.anon, "shmem": h.shmem, "cache": h.cache, "kernel": kernel,
                "top": h.top.iter().map(|p| json!({"name": p.name, "cpu": p.cpu, "rss": p.rss, "vllm": p.vllm})).collect::<Vec<_>>(),
            })
        }
        _ => Value::Null,
    };
    o.insert("host".into(), host);

    // LiteLLM rows
    let ll_host = app
        .cfg
        .litellm
        .as_ref()
        .and_then(|l| l.url.as_deref())
        .and_then(crate::litellm::host_port)
        .map(|(h, p)| if p == 443 || p == 80 { h } else { format!("{h}:{p}") });
    let matched = match &ep.cfg.litellm_model_group {
        Some(p) => p.clone(),
        None => crate::litellm::host_port(&ep.cfg.url).map(|(_, p)| format!("api_base :{p}")).unwrap_or_default(),
    };
    o.insert("requests".into(), json!({
        "enabled": app.litellm_on,
        "host": opt(ll_host),
        "match": matched,
        "error": opt(ep.ll_err.clone()),
        "polled_ago": opt(ep.ll_t.map(|t| now() - t)),
        "rows": ep.ll_rows.iter().take(40).map(|r| json!({
            "start": r.start, "client": r.client, "prompt": r.prompt, "completion": r.completion,
            "ttft": opt(r.ttft()), "rate": opt(r.decode_rate()), "dur": r.end - r.start,
            "ok": r.status.is_empty() || r.status == "success",
        })).collect::<Vec<_>>(),
    }));
    out
}
