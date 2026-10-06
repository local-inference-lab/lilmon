// Copyright 2026 Local Inference Lab, Inc.
// SPDX-License-Identifier: Apache-2.0

//! Recent per-request rows from a LiteLLM proxy's spend log, read through its HTTP API
//! (`GET /spend/logs/v2`). A read-only key is enough: a user with the `proxy_admin_viewer` role,
//! optionally limited to that single route with `allowed_routes`.

use serde::Deserialize;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct Row {
    pub start: f64,
    pub first: Option<f64>,
    pub end: f64,
    pub prompt: i64,
    pub completion: i64,
    pub status: String,
    pub client: String,
    pub model_group: String,
    pub api_base: String,
}

impl Row {
    /// Non-streaming responses record completionStartTime == endTime, so there is no TTFT.
    pub fn ttft(&self) -> Option<f64> {
        let f = self.first.filter(|f| *f >= self.start)?;
        (self.end - f > 0.005 || self.completion <= 1).then_some(f - self.start)
    }
    pub fn decode_rate(&self) -> Option<f64> {
        let f = self.first?;
        let d = self.end - f;
        (d > 0.05 && self.completion > 1).then(|| (self.completion - 1) as f64 / d)
    }
}

/// One spend-log entry as the API returns it (only the fields lilmon reads).
#[derive(Deserialize)]
struct ApiRow {
    #[serde(rename = "startTime")]
    start: Option<String>,
    #[serde(rename = "completionStartTime")]
    first: Option<String>,
    #[serde(rename = "endTime")]
    end: Option<String>,
    #[serde(default)]
    prompt_tokens: Option<i64>,
    #[serde(default)]
    completion_tokens: Option<i64>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    model_group: Option<String>,
    #[serde(default)]
    api_base: Option<String>,
    #[serde(default)]
    end_user: Option<String>,
    #[serde(default)]
    request_tags: Option<serde_json::Value>,
    #[serde(default)]
    metadata: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct Page {
    data: Vec<ApiRow>,
}

impl ApiRow {
    fn into_row(self) -> Option<Row> {
        let start = parse_time(self.start.as_deref()?)?;
        let end = self.end.as_deref().and_then(parse_time).unwrap_or(start);
        let meta = |k: &str| {
            self.metadata.as_ref().and_then(|m| m.get(k)).and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(str::to_string)
        };
        // Client label: key alias, then team alias, then end user, then the longest User-Agent tag.
        let ua = self.request_tags.as_ref().and_then(|t| t.as_array()).and_then(|tags| {
            tags.iter()
                .filter_map(|t| t.as_str()?.strip_prefix("User-Agent: "))
                .max_by_key(|s| s.len())
                .map(str::to_string)
        });
        let client = meta("user_api_key_alias")
            .or_else(|| meta("user_api_key_team_alias"))
            .or_else(|| self.end_user.clone().filter(|s| !s.is_empty()))
            .or(ua)
            .unwrap_or_default();
        Some(Row {
            start,
            first: self.first.as_deref().and_then(parse_time),
            end,
            prompt: self.prompt_tokens.unwrap_or(0),
            completion: self.completion_tokens.unwrap_or(0),
            status: self.status.unwrap_or_default(),
            client,
            model_group: self.model_group.unwrap_or_default(),
            api_base: self.api_base.unwrap_or_default(),
        })
    }
}

/// Days since 1970-01-01 for a civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { yoe + era * 400 + 1 } else { yoe + era * 400 }, m, d)
}

/// ISO-8601 timestamp ("2026-10-06T01:16:11.458+00:00", "...Z", or no zone = UTC) to unix seconds.
pub fn parse_time(s: &str) -> Option<f64> {
    let s = s.trim();
    let (date, rest) = s.split_once(['T', ' '])?;
    let mut dp = date.split('-');
    let (y, mo, d): (i64, i64, i64) = (dp.next()?.parse().ok()?, dp.next()?.parse().ok()?, dp.next()?.parse().ok()?);
    let (time, offset) = match rest.find(['+', '-', 'Z']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let mut tp = time.split(':');
    let (h, mi): (i64, i64) = (tp.next()?.parse().ok()?, tp.next()?.parse().ok()?);
    let sec: f64 = tp.next().unwrap_or("0").parse().ok()?;
    let off = match offset {
        "" | "Z" => 0,
        o => {
            let sign = if o.starts_with('-') { -1 } else { 1 };
            let o = &o[1..];
            let (oh, om) = o.split_once(':').unwrap_or((o.get(..2)?, o.get(2..).unwrap_or("0")));
            sign * (oh.parse::<i64>().ok()? * 3600 + om.parse::<i64>().unwrap_or(0) * 60)
        }
    };
    Some((days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 - off) as f64 + sec)
}

/// Unix seconds to the "YYYY-MM-DD HH:MM:SS" UTC form the API's date filters take.
fn fmt_utc(t: f64) -> String {
    let secs = t.floor() as i64;
    let (y, m, d) = civil_from_days(secs.div_euclid(86400));
    let r = secs.rem_euclid(86400);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}", r / 3600, (r / 60) % 60, r % 60)
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Which spend-log rows belong to an endpoint.
#[derive(Clone, Debug)]
pub enum Target {
    /// `litellm_model_group`: a trailing `%` or `/` makes it a prefix (models get swapped behind a stable
    /// prefix like `gracie/`); otherwise it is an exact model group.
    Group(String),
    /// No model group configured: rows whose `api_base` points at the endpoint's host and port.
    Server { host: String, port: u16 },
}

/// (host, port) of a URL, with the scheme's default port.
pub fn host_port(url: &str) -> Option<(String, u16)> {
    let (scheme, rest) = url.split_once("://").unwrap_or(("http", url));
    let authority = rest.split('/').next()?;
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty() => (h, p.parse().ok()?),
        _ => (authority, if scheme == "https" { 443 } else { 80 }),
    };
    Some((host.trim_matches(['[', ']']).to_ascii_lowercase(), port))
}

fn this_host() -> &'static str {
    use std::sync::OnceLock;
    static H: OnceLock<String> = OnceLock::new();
    H.get_or_init(|| std::fs::read_to_string("/proc/sys/kernel/hostname").map(|s| s.trim().to_ascii_lowercase()).unwrap_or_default())
}

/// localhost, loopback addresses, and this machine's own name all mean "here".
fn is_local(h: &str) -> bool {
    let me = this_host();
    matches!(h, "localhost" | "127.0.0.1" | "::1" | "0.0.0.0")
        || (!me.is_empty() && (h == me || h.split('.').next() == me.split('.').next()))
}

impl Target {
    fn matches(&self, row: &Row) -> bool {
        match self {
            Target::Group(pattern) => {
                let p = pattern.trim_end_matches('%');
                if pattern.ends_with('%') || pattern.ends_with('/') { row.model_group.starts_with(p) } else { row.model_group == p }
            }
            Target::Server { host, port } => host_port(&row.api_base)
                .is_some_and(|(h, p)| p == *port && (h == *host || (is_local(&h) && is_local(host)))),
        }
    }
}

const LOOKBACK_S: f64 = 6.0 * 3600.0;
const ROWS: usize = 60;

/// `targets`: (endpoint index, which rows are its own).
pub fn spawn(tx: Sender<crate::Msg>, base: String, key: Option<String>, poll: Duration, targets: Vec<(usize, Target)>) {
    if targets.is_empty() {
        return;
    }
    std::thread::Builder::new()
        .name("litellm".into())
        .spawn(move || {
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(15)))
                .http_status_as_error(false)
                .build()
                .into();
            let base = base.trim_end_matches('/').to_string();
            // One exact pattern can be filtered server-side; prefixes are filtered here.
            let exact = match targets.as_slice() {
                [(_, Target::Group(p))] if !p.ends_with('%') && !p.ends_with('/') => Some(p.clone()),
                _ => None,
            };
            let mut backoff = Duration::from_secs(2);
            loop {
                let t0 = Instant::now();
                let now = crate::state::now();
                let mut url = format!(
                    "{base}/spend/logs/v2?start_date={}&end_date={}&page=1&page_size=500&sort_by=startTime&sort_order=desc",
                    enc(&fmt_utc(now - LOOKBACK_S)),
                    enc(&fmt_utc(now + 60.0)),
                );
                if let Some(g) = &exact {
                    url.push_str(&format!("&model_group={}", enc(g)));
                }
                let mut req = agent.get(&url);
                if let Some(k) = &key {
                    req = req.header("Authorization", &format!("Bearer {k}"));
                }
                let result: Result<Vec<Row>, String> = match req.call() {
                    Ok(mut r) => match r.status().as_u16() {
                        200 => r
                            .body_mut()
                            .with_config()
                            .limit(64 << 20)
                            .read_to_string()
                            .map_err(|e| e.to_string())
                            .and_then(|t| serde_json::from_str::<Page>(&t).map_err(|e| format!("bad reply: {e}")))
                            .map(|p| p.data.into_iter().filter_map(ApiRow::into_row).collect()),
                        401 if key.is_none() => Err("401: set a LiteLLM key (api_key_file, api_key_env or LITELLM_API_KEY)".into()),
                        401 | 403 => Err(format!("{}: key rejected; it needs read access to /spend/logs/v2", r.status().as_u16())),
                        s => Err(format!("HTTP {s}")),
                    },
                    Err(e) => Err(e.to_string()),
                };
                match result {
                    Ok(rows) => {
                        backoff = Duration::from_secs(2);
                        for (ep, pat) in &targets {
                            let mine: Vec<Row> = rows.iter().filter(|r| pat.matches(r)).take(ROWS).cloned().collect();
                            if tx.send(crate::Msg::LiteLlm(*ep, mine, crate::state::now())).is_err() {
                                return;
                            }
                        }
                        std::thread::sleep(poll.saturating_sub(t0.elapsed()));
                    }
                    Err(e) => {
                        for (ep, _) in &targets {
                            let _ = tx.send(crate::Msg::LiteLlmErr(*ep, e.clone()));
                        }
                        std::thread::sleep(backoff.max(poll));
                        backoff = (backoff * 2).min(Duration::from_secs(60));
                    }
                }
            }
        })
        .expect("spawn litellm thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_timestamps() {
        let a = parse_time("2026-10-06T01:16:11.458+00:00").unwrap();
        assert!((a - 1791249371.458).abs() < 1e-6);
        assert_eq!(parse_time("2026-10-06T01:16:11.458Z"), Some(a));
        assert_eq!(parse_time("2026-10-05T21:16:11.458-04:00"), Some(a));
        assert_eq!(parse_time("2026-10-06 01:16:11.458"), Some(a));
        assert_eq!(fmt_utc(a), "2026-10-06 01:16:11");
        assert_eq!(fmt_utc(0.0), "1970-01-01 00:00:00");
    }

    fn row(group: &str, base: &str) -> Row {
        Row {
            start: 0.0,
            first: None,
            end: 0.0,
            prompt: 0,
            completion: 0,
            status: String::new(),
            client: String::new(),
            model_group: group.into(),
            api_base: base.into(),
        }
    }

    #[test]
    fn model_group_patterns() {
        let g = |p: &str| Target::Group(p.into());
        assert!(g("gracie/%").matches(&row("gracie/dsv41-flash-uva", "")));
        assert!(g("gracie/").matches(&row("gracie/qwen38", "")));
        assert!(!g("gracie/%").matches(&row("maxwell/qwen3-8b", "")));
        assert!(g("gracie/dsv41").matches(&row("gracie/dsv41", "")));
        assert!(!g("gracie/dsv41").matches(&row("gracie/dsv41-flash-uva", "")));
    }

    #[test]
    fn server_matching_by_api_base() {
        let t = |u: &str| {
            let (host, port) = host_port(u).unwrap();
            Target::Server { host, port }
        };
        assert!(t("http://inference-box:8000/metrics").matches(&row("", "http://inference-box:8000/v1")));
        assert!(!t("http://inference-box:8000/metrics").matches(&row("", "http://inference-box:8001/v1")));
        assert!(!t("http://inference-box:8000/metrics").matches(&row("", "http://other:8000/v1")));
        // localhost on this machine is the same server LiteLLM reaches by this machine's name
        let me = this_host().to_string();
        if !me.is_empty() {
            assert!(t("http://localhost:30006/metrics").matches(&row("", &format!("http://{me}:30006/v1"))));
        }
        assert_eq!(host_port("https://x.example.com/v1"), Some(("x.example.com".into(), 443)));
    }

    #[test]
    fn api_row_mapping() {
        let j = r#"{"data":[{"startTime":"2026-10-06T01:16:11.458+00:00","endTime":"2026-10-06T01:16:12.787+00:00",
            "completionStartTime":"2026-10-06T01:16:11.986+00:00","prompt_tokens":159978,"completion_tokens":79,
            "status":"success","model_group":"gracie/dsv41-flash-uva","end_user":"",
            "request_tags":["User-Agent: OpenAI","User-Agent: OpenAI/Python 2.24.0"],"metadata":{}}]}"#;
        let p: Page = serde_json::from_str(j).unwrap();
        let r = p.data.into_iter().next().unwrap().into_row().unwrap();
        assert_eq!(r.client, "OpenAI/Python 2.24.0");
        assert_eq!(r.prompt, 159978);
        assert!((r.ttft().unwrap() - 0.528).abs() < 1e-3);
    }
}
