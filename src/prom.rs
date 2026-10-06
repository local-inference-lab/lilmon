// Copyright 2026 Local Inference Lab, Inc.
// SPDX-License-Identifier: Apache-2.0

//! Minimal Prometheus text-format parser for the vLLM metric families we use.
//! Counters and gauges are summed across engines (data-parallel ranks) and models.

use std::collections::BTreeMap;

pub const MAXPOS: usize = 8;

/// Cumulative histogram (Prometheus `le` buckets, cumulative counts).
#[derive(Clone, Debug, Default)]
pub struct Hist {
    pub le: Vec<f64>,
    pub cum: Vec<f64>,
    pub sum: f64,
    pub count: f64,
}

impl Hist {
    fn add_bucket(&mut self, le: f64, v: f64) {
        match self.le.iter().position(|&x| x == le) {
            Some(i) => self.cum[i] += v,
            None => {
                let i = self.le.partition_point(|&x| x < le);
                self.le.insert(i, le);
                self.cum.insert(i, v);
            }
        }
    }

    /// `self - older`, bucket-wise. Returns None if the layouts differ or counters went backwards.
    pub fn delta(&self, older: &Hist) -> Option<Hist> {
        if self.le != older.le || self.count < older.count {
            return None;
        }
        Some(Hist {
            le: self.le.clone(),
            cum: self.cum.iter().zip(&older.cum).map(|(a, b)| (a - b).max(0.0)).collect(),
            sum: self.sum - older.sum,
            count: self.count - older.count,
        })
    }

    /// Prometheus-style quantile with linear interpolation inside the bucket.
    pub fn quantile(&self, q: f64) -> Option<f64> {
        if self.count <= 0.0 || self.le.is_empty() {
            return None;
        }
        let total = *self.cum.last().unwrap();
        if total <= 0.0 {
            return None;
        }
        let rank = q * total;
        let i = self.cum.iter().position(|&c| c >= rank)?;
        let mut hi = self.le[i];
        let lo = if i == 0 { 0.0 } else { self.le[i - 1] };
        // Everything in the first bucket (e.g. queue times of ~0 under a 300 ms first bound):
        // the overall mean is that bucket's mean, so interpolate over [0, 2*mean] instead.
        if i == 0 && self.cum[0] >= total && self.count > 0.0 {
            hi = hi.min(2.0 * self.sum / self.count);
        }
        if hi.is_infinite() {
            return Some(lo);
        }
        let below = if i == 0 { 0.0 } else { self.cum[i - 1] };
        let inb = self.cum[i] - below;
        if inb <= 0.0 {
            return Some(hi);
        }
        Some(lo + (hi - lo) * ((rank - below) / inb))
    }

    /// Upper bound of the highest bucket that has any observations (finite), for delta histograms.
    pub fn max_upper(&self) -> Option<f64> {
        let mut prev = 0.0;
        let mut best = None;
        for (i, &c) in self.cum.iter().enumerate() {
            if c > prev {
                best = Some(self.le[i]);
            }
            prev = c;
        }
        best
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum H {
    Ttft,
    Itl,
    Tpot,
    E2e,
    Queue,
    PrefillTime,
    DecodeTime,
}

impl H {
    pub const ALL: [H; 7] = [H::Ttft, H::Itl, H::Tpot, H::E2e, H::Queue, H::PrefillTime, H::DecodeTime];
    fn from_family(name: &str) -> Option<H> {
        Some(match name {
            "vllm:time_to_first_token_seconds" => H::Ttft,
            "vllm:inter_token_latency_seconds" => H::Itl,
            "vllm:request_time_per_output_token_seconds" => H::Tpot,
            "vllm:e2e_request_latency_seconds" => H::E2e,
            "vllm:request_queue_time_seconds" => H::Queue,
            "vllm:request_prefill_time_seconds" => H::PrefillTime,
            "vllm:request_decode_time_seconds" => H::DecodeTime,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct Scrape {
    pub t: f64,
    pub models: Vec<String>,
    pub gen_tokens: f64,
    pub prompt_tokens: f64,
    pub prompt_computed: f64,
    pub prompt_cached: f64,
    pub has_prompt_by_source: bool,
    pub running: f64,
    pub waiting: f64,
    pub waiting_capacity: f64,
    pub waiting_deferred: f64,
    pub kv_usage: f64,
    pub kv_engines: u32,
    pub prefix_queries: f64,
    pub prefix_hits: f64,
    pub preemptions: f64,
    pub finished: BTreeMap<String, f64>,
    pub drafts: f64,
    pub draft_tokens: f64,
    pub accepted: f64,
    pub accepted_pos: [f64; MAXPOS],
    pub num_pos: usize,
    pub has_spec: bool,
    pub hist: BTreeMap<H, Hist>,
    pub req_prompt_sum: f64,
    pub req_prompt_count: f64,
    pub req_gen_sum: f64,
    pub req_gen_count: f64,
    pub prefill_kv_computed_sum: f64,
    pub cache_info: BTreeMap<String, String>,
    pub process_start: Option<f64>,
    pub asleep: bool,
}

impl Scrape {
    pub fn finished_total(&self) -> f64 {
        self.finished.values().sum()
    }
    pub fn kv_capacity_tokens(&self) -> Option<f64> {
        self.cache_info.get("kv_cache_size_tokens").and_then(|s| s.parse().ok())
    }
}

fn parse_labels(s: &str) -> Vec<(&str, &str)> {
    // s is the text between { and }, e.g. engine="0",le="0.5"
    let mut out = Vec::with_capacity(4);
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let ks = i;
        while i < b.len() && b[i] != b'=' {
            i += 1;
        }
        let key = s[ks..i].trim_matches(|c| c == ',' || c == ' ');
        i += 1; // '='
        if i >= b.len() || b[i] != b'"' {
            break;
        }
        i += 1;
        let vs = i;
        while i < b.len() && b[i] != b'"' {
            if b[i] == b'\\' {
                i += 1;
            }
            i += 1;
        }
        let val = &s[vs..i.min(b.len())];
        out.push((key, val));
        i += 1; // closing quote
        while i < b.len() && (b[i] == b',' || b[i] == b' ') {
            i += 1;
        }
    }
    out
}

fn label<'a>(ls: &[(&'a str, &'a str)], k: &str) -> Option<&'a str> {
    ls.iter().find(|(kk, _)| *kk == k).map(|(_, v)| *v)
}

fn parse_value(s: &str) -> Option<f64> {
    match s {
        "+Inf" | "Inf" => Some(f64::INFINITY),
        "-Inf" => Some(f64::NEG_INFINITY),
        "NaN" => Some(f64::NAN),
        _ => s.parse().ok(),
    }
}

pub fn parse(text: &str, t: f64) -> Scrape {
    let mut s = Scrape { t, ..Default::default() };
    for line in text.lines() {
        if line.is_empty() || line.as_bytes()[0] == b'#' {
            continue;
        }
        let (name, labels, rest) = match line.find(['{', ' ']) {
            Some(i) if line.as_bytes()[i] == b'{' => {
                let Some(j) = line[i..].rfind('}') else { continue };
                (&line[..i], &line[i + 1..i + j], &line[i + j + 1..])
            }
            Some(i) => (&line[..i], "", &line[i..]),
            None => continue,
        };
        if name == "process_start_time_seconds" {
            s.process_start = rest.trim().parse().ok();
            continue;
        }
        let Some(short) = name.strip_prefix("vllm:") else { continue };
        let Some(v) = rest.split_whitespace().next().and_then(parse_value) else { continue };
        let ls = parse_labels(labels);
        if let Some(m) = label(&ls, "model_name")
            && !s.models.iter().any(|x| x == m) {
                s.models.push(m.to_string());
            }
        match short {
            "generation_tokens_total" => s.gen_tokens += v,
            "prompt_tokens_total" => s.prompt_tokens += v,
            "prompt_tokens_by_source_total" => {
                s.has_prompt_by_source = true;
                match label(&ls, "source") {
                    Some("local_compute") => s.prompt_computed += v,
                    Some(_) => s.prompt_cached += v,
                    None => {}
                }
            }
            "num_requests_running" => s.running += v,
            "num_requests_waiting" => s.waiting += v,
            "num_requests_waiting_by_reason" => match label(&ls, "reason") {
                Some("capacity") => s.waiting_capacity += v,
                Some("deferred") => s.waiting_deferred += v,
                _ => {}
            },
            "kv_cache_usage_perc" | "gpu_cache_usage_perc" => {
                s.kv_usage += v;
                s.kv_engines += 1;
            }
            "prefix_cache_queries_total" | "gpu_prefix_cache_queries_total" => s.prefix_queries += v,
            "prefix_cache_hits_total" | "gpu_prefix_cache_hits_total" => s.prefix_hits += v,
            "num_preemptions_total" => s.preemptions += v,
            "request_success_total" => {
                let r = label(&ls, "finished_reason").unwrap_or("?").to_string();
                *s.finished.entry(r).or_default() += v;
            }
            "spec_decode_num_drafts_total" => {
                s.drafts += v;
                s.has_spec = true;
            }
            "spec_decode_num_draft_tokens_total" => s.draft_tokens += v,
            "spec_decode_num_accepted_tokens_total" => s.accepted += v,
            "spec_decode_num_accepted_tokens_per_pos_total" => {
                if let Some(p) = label(&ls, "position").and_then(|p| p.parse::<usize>().ok())
                    && p < MAXPOS {
                        s.accepted_pos[p] += v;
                        s.num_pos = s.num_pos.max(p + 1);
                    }
            }
            "request_prompt_tokens_sum" => s.req_prompt_sum += v,
            "request_prompt_tokens_count" => s.req_prompt_count += v,
            "request_generation_tokens_sum" => s.req_gen_sum += v,
            "request_generation_tokens_count" => s.req_gen_count += v,
            "request_prefill_kv_computed_tokens_sum" => s.prefill_kv_computed_sum += v,
            "cache_config_info" => {
                for (k, val) in &ls {
                    if *k != "engine" {
                        s.cache_info.insert((*k).to_string(), (*val).to_string());
                    }
                }
            }
            "engine_sleep_state" => {
                if label(&ls, "sleep_state") == Some("awake") && v == 0.0 {
                    s.asleep = true;
                }
            }
            _ => {
                if let Some(fam) = short.strip_suffix("_bucket") {
                    if let Some(h) = H::from_family(&format!("vllm:{fam}"))
                        && let Some(le) = label(&ls, "le").and_then(parse_value) {
                            s.hist.entry(h).or_default().add_bucket(le, v);
                        }
                } else if let Some(fam) = short.strip_suffix("_sum") {
                    if let Some(h) = H::from_family(&format!("vllm:{fam}")) {
                        s.hist.entry(h).or_default().sum += v;
                    }
                } else if let Some(fam) = short.strip_suffix("_count")
                    && let Some(h) = H::from_family(&format!("vllm:{fam}")) {
                        s.hist.entry(h).or_default().count += v;
                    }
            }
        }
    }
    if s.kv_engines > 1 {
        s.kv_usage /= s.kv_engines as f64;
    }
    if !s.has_prompt_by_source {
        s.prompt_computed = s.prompt_tokens;
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic() {
        let txt = r#"# HELP x
vllm:generation_tokens_total{engine="0",model_name="m"} 770974.0
vllm:prompt_tokens_by_source_total{engine="0",model_name="m",source="local_compute"} 2.6356e+06
vllm:prompt_tokens_by_source_total{engine="0",model_name="m",source="local_cache_hit"} 100.0
vllm:spec_decode_num_accepted_tokens_per_pos_total{engine="0",model_name="m",position="4"} 50937.0
vllm:time_to_first_token_seconds_bucket{engine="0",le="0.5",model_name="m"} 3.0
vllm:time_to_first_token_seconds_bucket{engine="0",le="+Inf",model_name="m"} 4.0
vllm:time_to_first_token_seconds_count{engine="0",model_name="m"} 4.0
vllm:time_to_first_token_seconds_sum{engine="0",model_name="m"} 3.5
vllm:cache_config_info{block_size="128",engine="0",kv_cache_size_tokens="3423027"} 1.0
process_start_time_seconds 1.7e9
"#;
        let s = parse(txt, 0.0);
        assert_eq!(s.gen_tokens, 770974.0);
        assert_eq!(s.prompt_computed, 2635600.0);
        assert_eq!(s.prompt_cached, 100.0);
        assert_eq!(s.num_pos, 5);
        assert_eq!(s.models, vec!["m".to_string()]);
        let h = &s.hist[&H::Ttft];
        assert_eq!(h.le, vec![0.5, f64::INFINITY]);
        assert_eq!(h.count, 4.0);
        assert_eq!(s.kv_capacity_tokens(), Some(3423027.0));
        assert_eq!(s.process_start, Some(1.7e9));
        assert!((h.quantile(0.5).unwrap() - 0.333).abs() < 0.01);
    }
}
