// Copyright 2026 Local Inference Lab, Inc.
// SPDX-License-Identifier: Apache-2.0

//! Per-endpoint time series built from successive scrapes.
//!
//! Counters are turned into per-second buckets by spreading each scrape-to-scrape delta over the
//! time it covers. Prefill is special: vLLM credits a request's prompt tokens only when its first
//! token comes out, so a 1M-token prefill would show up as one enormous spike. Instead, each
//! credited batch is spread back over the measured TTFT of the requests that finished prefill in
//! that interval. The recent past can therefore be revised when a long prefill completes.

use crate::prom::{H, Hist, MAXPOS, Scrape};
use std::collections::{BTreeMap, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default)]
pub struct Bucket {
    pub seen: bool,
    pub decode: f32,
    pub prefill: f32,
    pub cached: f32,
    pub running: f32,
    pub waiting: f32,
    pub kv: f32,
    pub drafts: f32,
    pub draft_tok: f32,
    pub accepted: f32,
    pub acc_pos: [f32; MAXPOS],
    pub finished: f32,
    pub prefix_q: f32,
    pub prefix_h: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Decode,
    Prefill,
    Cached,
    Running,
    Waiting,
    Kv,
    AcceptLen,
}

impl Bucket {
    pub fn get(&self, f: Field) -> f32 {
        match f {
            Field::Decode => self.decode,
            Field::Prefill => self.prefill,
            Field::Cached => self.cached,
            Field::Running => self.running,
            Field::Waiting => self.waiting,
            Field::Kv => self.kv,
            Field::AcceptLen => {
                if self.drafts > 0.0 {
                    1.0 + self.accepted / self.drafts
                } else {
                    0.0
                }
            }
        }
    }
}

pub struct Series {
    pub t0: i64,
    pub b: VecDeque<Bucket>,
    cap: usize,
}

impl Series {
    fn new(cap: usize) -> Self {
        Series { t0: 0, b: VecDeque::new(), cap }
    }

    fn slot(&mut self, sec: i64) -> Option<&mut Bucket> {
        if self.b.is_empty() {
            self.t0 = sec;
        }
        if sec < self.t0 {
            return None;
        }
        let idx = (sec - self.t0) as usize;
        if idx >= self.b.len() {
            let grow = idx + 1 - self.b.len();
            if grow > self.cap {
                // Huge gap (e.g. laptop asleep): start over.
                self.b.clear();
                self.t0 = sec;
                self.b.push_back(Bucket::default());
            } else {
                self.b.extend(std::iter::repeat_n(Bucket::default(), grow));
            }
        }
        while self.b.len() > self.cap {
            self.b.pop_front();
            self.t0 += 1;
        }
        let idx = (sec - self.t0) as usize;
        self.b.get_mut(idx)
    }

    pub fn get(&self, sec: i64) -> Option<&Bucket> {
        if sec < self.t0 {
            return None;
        }
        self.b.get((sec - self.t0) as usize)
    }

    /// Spread `total` uniformly over [ta, tb], applying `f` to each overlapped second.
    fn spread(&mut self, ta: f64, tb: f64, total: f64, f: impl Fn(&mut Bucket, f32)) {
        if total == 0.0 {
            return;
        }
        if tb - ta <= 1e-6 {
            if let Some(b) = self.slot(tb.floor() as i64) {
                f(b, total as f32);
            }
            return;
        }
        let span = tb - ta;
        let mut s = ta.floor() as i64;
        while (s as f64) < tb {
            let ov = (tb.min(s as f64 + 1.0) - ta.max(s as f64)).max(0.0);
            if ov > 0.0
                && let Some(b) = self.slot(s) {
                    f(b, (total * ov / span) as f32);
                }
            s += 1;
        }
    }

    fn mark_seen(&mut self, ta: f64, tb: f64) {
        let mut s = ta.floor() as i64;
        while (s as f64) <= tb {
            if let Some(b) = self.slot(s) {
                b.seen = true;
            }
            s += 1;
        }
    }
}

#[derive(Clone, Debug)]
pub struct PrefillEvent {
    pub t: f64,
    pub tokens: f64,
    pub cached: f64,
    pub secs: f64,
    pub reqs: f64,
}

/// Snapshot of cumulative values that aren't bucketed (latency histograms and request sums).
#[derive(Clone)]
struct Snap {
    t: f64,
    hist: BTreeMap<H, Hist>,
    req_gen_sum: f64,
    req_gen_count: f64,
    prefill_kv_sum: f64,
    req_prompt_sum: f64,
    req_prompt_count: f64,
}

impl Snap {
    fn of(s: &Scrape) -> Snap {
        Snap {
            t: s.t,
            hist: s.hist.clone(),
            req_gen_sum: s.req_gen_sum,
            req_gen_count: s.req_gen_count,
            prefill_kv_sum: s.prefill_kv_computed_sum,
            req_prompt_sum: s.req_prompt_sum,
            req_prompt_count: s.req_prompt_count,
        }
    }
}

#[derive(Default, Clone, Debug)]
pub struct WindowLatency {
    pub secs: f64,
    pub hist: BTreeMap<H, Hist>,
    pub requests: f64,
    pub prompt_mean: Option<f64>,
    pub per_req_decode: Option<f64>,
    pub per_req_prefill: Option<f64>,
}

#[derive(Default, Clone, Debug)]
pub struct SpecStats {
    pub drafts: f64,
    pub draft_tokens: f64,
    pub accepted: f64,
    pub acc_pos: Vec<f64>,
    /// Per-position acceptance rate, using the k mix when it can be solved.
    pub pos_rate: Vec<Option<f64>>,
    pub pos_exact: bool,
    pub secs: f64,
}

impl SpecStats {
    pub fn accept_len(&self) -> Option<f64> {
        (self.drafts > 0.0).then(|| 1.0 + self.accepted / self.drafts)
    }
    pub fn accept_rate(&self) -> Option<f64> {
        (self.draft_tokens > 0.0).then(|| self.accepted / self.draft_tokens)
    }
    pub fn mean_k(&self) -> Option<f64> {
        (self.drafts > 0.0).then(|| self.draft_tokens / self.drafts)
    }
}

pub struct EndpointState {
    pub series: Series,
    pub prev: Option<Scrape>,
    snaps: VecDeque<Snap>,
    pub prefill_events: VecDeque<PrefillEvent>,
    pub decoding: f64,
    pub prefilling: f64,
    pub prefill_since: Option<f64>,
    pub restarts: u32,
    pub first_t: Option<f64>,
    pub spec_k_set: Vec<u32>,
    persisted_upto: i64,
    hist_path: Option<PathBuf>,
}

pub fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

const SNAP_EVERY: f64 = 2.0;
const SNAP_KEEP: f64 = 3600.0;
/// Buckets younger than this may still be revised by prefill back-fill, so they aren't persisted yet.
const PERSIST_LAG: i64 = 120;

impl EndpointState {
    pub fn new(history_secs: usize, spec_k_set: Vec<u32>, hist_path: Option<PathBuf>) -> Self {
        let mut st = EndpointState {
            series: Series::new(history_secs.max(3600)),
            prev: None,
            snaps: VecDeque::new(),
            prefill_events: VecDeque::new(),
            decoding: 0.0,
            prefilling: 0.0,
            prefill_since: None,
            restarts: 0,
            first_t: None,
            spec_k_set,
            persisted_upto: i64::MIN,
            hist_path,
        };
        if let Some(p) = st.hist_path.clone() {
            st.load_history(&p);
        }
        st
    }

    pub fn last(&self) -> Option<&Scrape> {
        self.prev.as_ref()
    }

    /// Latest second whose bucket is complete.
    pub fn complete_sec(&self) -> i64 {
        self.prev.as_ref().map(|s| s.t.floor() as i64 - 1).unwrap_or_else(|| now().floor() as i64 - 1)
    }

    pub fn ingest(&mut self, s: Scrape) {
        self.first_t.get_or_insert(s.t);
        if let Some(p) = self.prev.take() {
            let restarted = s.gen_tokens < p.gen_tokens
                || s.prompt_tokens < p.prompt_tokens
                || s.finished_total() < p.finished_total()
                || matches!((s.process_start, p.process_start), (Some(a), Some(b)) if (a - b).abs() > 1.0);
            if restarted {
                self.restarts += 1;
                self.snaps.clear();
                self.decoding = 0.0;
            } else {
                self.apply_delta(&p, &s);
            }
        } else {
            self.decoding = s.running;
        }

        let sec = s.t.floor() as i64;
        if let Some(b) = self.series.slot(sec) {
            b.seen = true;
            b.running = b.running.max(s.running as f32);
            b.waiting = b.waiting.max(s.waiting as f32);
            b.kv = b.kv.max(s.kv_usage as f32);
        }

        if self.snaps.back().is_none_or(|l| s.t - l.t >= SNAP_EVERY) {
            self.snaps.push_back(Snap::of(&s));
            while self.snaps.front().is_some_and(|f| s.t - f.t > SNAP_KEEP + SNAP_EVERY) {
                self.snaps.pop_front();
            }
        }
        self.prev = Some(s);
    }

    fn apply_delta(&mut self, p: &Scrape, s: &Scrape) {
        let (ta, tb) = (p.t, s.t);
        let sr = &mut self.series;
        sr.mark_seen(ta, tb);
        sr.spread(ta, tb, s.gen_tokens - p.gen_tokens, |b, x| b.decode += x);
        sr.spread(ta, tb, s.drafts - p.drafts, |b, x| b.drafts += x);
        sr.spread(ta, tb, s.draft_tokens - p.draft_tokens, |b, x| b.draft_tok += x);
        sr.spread(ta, tb, s.accepted - p.accepted, |b, x| b.accepted += x);
        for i in 0..s.num_pos.min(MAXPOS) {
            sr.spread(ta, tb, s.accepted_pos[i] - p.accepted_pos[i], |b, x| b.acc_pos[i] += x);
        }
        sr.spread(ta, tb, s.finished_total() - p.finished_total(), |b, x| b.finished += x);
        sr.spread(ta, tb, s.prefix_queries - p.prefix_queries, |b, x| b.prefix_q += x);
        sr.spread(ta, tb, s.prefix_hits - p.prefix_hits, |b, x| b.prefix_h += x);

        // Prefill back-fill over the TTFT of the requests that just got their first token.
        let ttft = match (s.hist.get(&H::Ttft), p.hist.get(&H::Ttft)) {
            (Some(a), Some(b)) => a.delta(b),
            _ => None,
        };
        let new_first = ttft.as_ref().map(|d| d.count).unwrap_or(0.0);
        let dn = (s.prompt_computed - p.prompt_computed).max(0.0);
        let dc = (s.prompt_cached - p.prompt_cached).max(0.0);
        if dn > 0.0 || dc > 0.0 {
            let mut w = tb - ta;
            if let Some(d) = ttft.as_ref().filter(|d| d.count > 0.0) {
                let ub = d.max_upper().filter(|u| u.is_finite()).unwrap_or(d.sum);
                w = d.sum.min(ub);
            }
            let w = w.clamp(0.05, 3600.0);
            sr.spread(tb - w, tb, dn, |b, x| b.prefill += x);
            sr.spread(tb - w, tb, dc, |b, x| b.cached += x);
            if dn >= 256.0 {
                self.prefill_events.push_back(PrefillEvent { t: tb, tokens: dn, cached: dc, secs: w, reqs: new_first });
                while self.prefill_events.len() > 64 {
                    self.prefill_events.pop_front();
                }
            }
        }

        // In-flight estimate: requests that are running but haven't produced a first token yet.
        let dfin = (s.finished_total() - p.finished_total()).max(0.0);
        self.decoding = (self.decoding + new_first - dfin).clamp(0.0, s.running);
        if s.running + s.waiting == 0.0 {
            self.decoding = 0.0;
        }
        self.prefilling = (s.running - self.decoding).max(0.0);
        if self.prefilling > 0.0 {
            self.prefill_since.get_or_insert((ta + tb) / 2.0);
        } else {
            self.prefill_since = None;
        }
    }

    /// Max of a field over the trailing `secs` complete seconds: (value, second).
    pub fn peak(&self, f: Field, secs: i64) -> Option<(f32, i64)> {
        let end = self.complete_sec();
        let mut best: Option<(f32, i64)> = None;
        for s in (end - secs + 1)..=end {
            if let Some(b) = self.series.get(s)
                && b.seen {
                    let v = b.get(f);
                    if best.is_none_or(|(bv, _)| v > bv) {
                        best = Some((v, s));
                    }
                }
        }
        best
    }

    /// Mean of a field over the trailing `secs` complete seconds (seen seconds only).
    pub fn mean(&self, f: Field, secs: i64) -> Option<f32> {
        let end = self.complete_sec();
        let (mut sum, mut n) = (0.0f64, 0u32);
        for s in (end - secs + 1)..=end {
            if let Some(b) = self.series.get(s).filter(|b| b.seen) {
                sum += b.get(f) as f64;
                n += 1;
            }
        }
        (n > 0).then(|| (sum / n as f64) as f32)
    }

    /// Sum of bucket fields over the trailing window.
    pub fn sum_window(&self, secs: i64) -> Bucket {
        let end = self.complete_sec();
        let mut acc = Bucket::default();
        for s in (end - secs + 1)..=end {
            if let Some(b) = self.series.get(s) {
                acc.decode += b.decode;
                acc.prefill += b.prefill;
                acc.cached += b.cached;
                acc.drafts += b.drafts;
                acc.draft_tok += b.draft_tok;
                acc.accepted += b.accepted;
                for i in 0..MAXPOS {
                    acc.acc_pos[i] += b.acc_pos[i];
                }
                acc.finished += b.finished;
                acc.prefix_q += b.prefix_q;
                acc.prefix_h += b.prefix_h;
                acc.running = acc.running.max(b.running);
                acc.kv = acc.kv.max(b.kv);
            }
        }
        acc
    }

    pub fn spec_window(&self, secs: i64) -> SpecStats {
        let w = self.sum_window(secs);
        let npos = self.prev.as_ref().map(|s| s.num_pos).unwrap_or(0);
        self.spec_from(w.drafts as f64, w.draft_tok as f64, w.accepted as f64, &w.acc_pos[..npos], secs as f64)
    }

    pub fn spec_lifetime(&self) -> SpecStats {
        match &self.prev {
            Some(s) => self.spec_from(s.drafts, s.draft_tokens, s.accepted, &s.accepted_pos[..s.num_pos], 0.0),
            None => SpecStats::default(),
        }
    }

    fn spec_from<T: Copy + Into<f64>>(&self, d: f64, t: f64, a: f64, pos: &[T], secs: f64) -> SpecStats {
        let acc_pos: Vec<f64> = pos.iter().map(|&x| x.into()).collect();
        let kmax = acc_pos.len();
        let mut ks: Vec<u32> = self.spec_k_set.clone();
        ks.sort_unstable();
        ks.dedup();
        let mut exact = false;
        // Number of drafts that included position p.
        let mut denom = vec![d; kmax];
        if d > 0.0 && kmax > 0 {
            let mean_k = t / d;
            if (mean_k - kmax as f64).abs() < 1e-3 {
                exact = true;
            } else if ks.len() == 2 && ks[1] as usize == kmax {
                let (k1, k2) = (ks[0] as f64, ks[1] as f64);
                let n2 = ((t - k1 * d) / (k2 - k1)).clamp(0.0, d);
                for (p, slot) in denom.iter_mut().enumerate() {
                    if p as f64 >= k1 {
                        *slot = n2;
                    }
                }
                exact = true;
            }
        }
        let pos_rate = acc_pos
            .iter()
            .zip(&denom)
            .map(|(&acc, &n)| (n >= 1.0).then(|| (acc / n).min(1.0)))
            .collect();
        SpecStats { drafts: d, draft_tokens: t, accepted: a, acc_pos, pos_rate, pos_exact: exact, secs }
    }

    /// Latency histograms and per-request rates over the trailing window (capped to the snapshot ring).
    pub fn latency_window(&self, secs: f64) -> WindowLatency {
        let Some(cur) = self.prev.as_ref() else { return WindowLatency::default() };
        let target = cur.t - secs;
        let Some(old) = self.snaps.iter().find(|s| s.t >= target - SNAP_EVERY).or(self.snaps.front()) else {
            return WindowLatency::default();
        };
        let mut out = WindowLatency { secs: cur.t - old.t, ..Default::default() };
        for h in H::ALL {
            if let (Some(a), Some(b)) = (cur.hist.get(&h), old.hist.get(&h))
                && let Some(d) = a.delta(b) {
                    out.hist.insert(h, d);
                }
        }
        let dreq = cur.req_gen_count - old.req_gen_count;
        out.requests = dreq.max(0.0);
        if dreq > 0.0 {
            out.prompt_mean = Some((cur.req_prompt_sum - old.req_prompt_sum) / (cur.req_prompt_count - old.req_prompt_count).max(1.0));
            if let Some(dt) = out.hist.get(&H::DecodeTime).filter(|h| h.sum > 0.0) {
                let toks = (cur.req_gen_sum - old.req_gen_sum) - dreq; // first token comes from prefill
                out.per_req_decode = Some(toks.max(0.0) / dt.sum);
            }
            if let Some(pt) = out.hist.get(&H::PrefillTime).filter(|h| h.sum > 0.0) {
                out.per_req_prefill = Some((cur.prefill_kv_computed_sum - old.prefill_kv_sum).max(0.0) / pt.sum);
            }
        }
        out
    }

    // ---- persistence -------------------------------------------------------------------------

    const COLS: [&'static str; 22] = [
        "t", "decode", "prefill", "cached", "running", "waiting", "kv", "drafts", "draft_tok", "accepted", "p0", "p1",
        "p2", "p3", "p4", "p5", "p6", "p7", "finished", "prefix_q", "prefix_h", "v",
    ];

    fn load_history(&mut self, path: &Path) {
        let Ok(f) = File::open(path) else { return };
        let cutoff = (now() as i64) - self.series.cap as i64;
        let mut lines = BufReader::new(f).lines();
        let Some(Ok(header)) = lines.next() else { return };
        let cols: Vec<String> = header.split(',').map(str::to_string).collect();
        let idx = |name: &str| cols.iter().position(|c| c == name);
        let map: Vec<Option<usize>> = Self::COLS.iter().map(|c| idx(c)).collect();
        let mut rows = 0usize;
        let mut kept = 0usize;
        for line in lines.map_while(Result::ok) {
            rows += 1;
            let v: Vec<&str> = line.split(',').collect();
            let get = |i: usize| -> f32 { map[i].and_then(|j| v.get(j)).and_then(|x| x.parse().ok()).unwrap_or(0.0) };
            let Some(t) = map[0].and_then(|j| v.get(j)).and_then(|x| x.parse::<i64>().ok()) else { continue };
            if t < cutoff {
                continue;
            }
            kept += 1;
            if let Some(b) = self.series.slot(t) {
                b.seen = true;
                b.decode = get(1);
                b.prefill = get(2);
                b.cached = get(3);
                b.running = get(4);
                b.waiting = get(5);
                b.kv = get(6);
                b.drafts = get(7);
                b.draft_tok = get(8);
                b.accepted = get(9);
                for i in 0..MAXPOS {
                    b.acc_pos[i] = get(10 + i);
                }
                b.finished = get(18);
                b.prefix_q = get(19);
                b.prefix_h = get(20);
            }
            self.persisted_upto = self.persisted_upto.max(t);
        }
        if rows > kept + kept / 4 + 3600 {
            self.rewrite_history(path);
        }
    }

    fn write_row(w: &mut impl Write, t: i64, b: &Bucket) -> std::io::Result<()> {
        write!(w, "{t},{},{},{},{},{},{}", b.decode, b.prefill, b.cached, b.running, b.waiting, b.kv)?;
        write!(w, ",{},{},{}", b.drafts, b.draft_tok, b.accepted)?;
        for x in b.acc_pos {
            write!(w, ",{x}")?;
        }
        writeln!(w, ",{},{},{},1", b.finished, b.prefix_q, b.prefix_h)
    }

    fn rewrite_history(&mut self, path: &Path) {
        let tmp = path.with_extension("tmp");
        let res = (|| -> std::io::Result<()> {
            let mut w = BufWriter::new(File::create(&tmp)?);
            writeln!(w, "{}", Self::COLS.join(","))?;
            for (i, b) in self.series.b.iter().enumerate() {
                let t = self.series.t0 + i as i64;
                if b.seen && t <= self.persisted_upto {
                    Self::write_row(&mut w, t, b)?;
                }
            }
            w.flush()?;
            std::fs::rename(&tmp, path)
        })();
        if res.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }

    /// Append buckets that can no longer change. `final_flush` writes everything (on exit).
    pub fn persist(&mut self, final_flush: bool) {
        let Some(path) = self.hist_path.clone() else { return };
        let upto = if final_flush { self.complete_sec() } else { self.complete_sec() - PERSIST_LAG };
        if upto <= self.persisted_upto {
            return;
        }
        let new_file = !path.exists();
        let res = (|| -> std::io::Result<()> {
            if let Some(d) = path.parent() {
                std::fs::create_dir_all(d)?;
            }
            let mut w = BufWriter::new(OpenOptions::new().create(true).append(true).open(&path)?);
            if new_file {
                writeln!(w, "{}", Self::COLS.join(","))?;
            }
            let start = (self.persisted_upto + 1).max(self.series.t0);
            for t in start..=upto {
                if let Some(b) = self.series.get(t).filter(|b| b.seen) {
                    Self::write_row(&mut w, t, b)?;
                }
            }
            w.flush()
        })();
        if res.is_ok() {
            self.persisted_upto = upto;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scrape(t: f64, gen_tokens: f64, computed: f64, ttft: (f64, f64), le_counts: &[(f64, f64)]) -> Scrape {
        let mut s = Scrape { t, gen_tokens, prompt_computed: computed, prompt_tokens: computed, ..Default::default() };
        let mut h = Hist { sum: ttft.0, count: ttft.1, ..Default::default() };
        for &(le, c) in le_counts {
            h.le.push(le);
            h.cum.push(c);
        }
        s.hist.insert(H::Ttft, h);
        s
    }

    #[test]
    fn decode_spread_and_prefill_backfill() {
        let mut st = EndpointState::new(3600, vec![], None);
        let le = |n: f64| vec![(1.0, 0.0), (2.5, n), (f64::INFINITY, n)];
        st.ingest(scrape(1000.0, 0.0, 0.0, (0.0, 0.0), &le(0.0)));
        st.ingest(scrape(1001.0, 300.0, 0.0, (0.0, 0.0), &le(0.0)));
        st.ingest(scrape(1002.0, 600.0, 96000.0, (2.0, 1.0), &le(1.0)));
        st.ingest(scrape(1003.0, 900.0, 96000.0, (2.0, 1.0), &le(1.0)));
        assert!((st.series.get(1001).unwrap().decode - 300.0).abs() < 1e-3);
        // 96K tokens spread over [1000, 1002]: 48K in each of seconds 1000 and 1001.
        assert!((st.series.get(1000).unwrap().prefill - 48000.0).abs() < 1.0);
        assert!((st.series.get(1001).unwrap().prefill - 48000.0).abs() < 1.0);
        assert_eq!(st.prefill_events.len(), 1);
    }

    #[test]
    fn spec_two_k_solve() {
        let st = EndpointState::new(3600, vec![3, 5], None);
        // 10 drafts: 6 with k=5, 4 with k=3 -> 42 draft tokens.
        let s = st.spec_from(10.0, 42.0, 20.0, &[8.0f64, 6.0, 3.0, 2.0, 1.0], 1.0);
        assert!(s.pos_exact);
        assert!((s.pos_rate[0].unwrap() - 0.8).abs() < 1e-9);
        assert!((s.pos_rate[3].unwrap() - 2.0 / 6.0).abs() < 1e-9);
    }
}
