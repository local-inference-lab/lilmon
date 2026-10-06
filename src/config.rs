// Copyright 2026 Local Inference Lab, Inc.
// SPDX-License-Identifier: Apache-2.0

//! Configuration: defaults, the TOML file, and a renderer that writes a fully commented file.
//! The same renderer produces the first-run file, `--init-config`, `--print-config`, and the
//! running config written from the settings overlay, so the file always documents every key.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    /// Scrape interval for /metrics, milliseconds.
    pub interval_ms: u64,
    /// How much 1-second history to keep in memory and on disk.
    pub history_hours: f64,
    /// Older files put `window` at the top level; `[ui].window` wins when both are set.
    #[serde(rename = "window")]
    pub legacy_window: Option<String>,
    pub ui: UiConfig,
    pub gpu: SourceConfig,
    pub host: SourceConfig,
    pub litellm: Option<LiteLlmConfig>,
    #[serde(rename = "endpoint")]
    pub endpoints: Vec<EndpointConfig>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct UiConfig {
    pub theme: String,
    /// "theme", "black", "none", or "#rrggbb".
    pub background: String,
    pub window: Option<String>,
    pub peaks: Vec<String>,
    pub decode_now_s: u32,
    pub prefill_now_s: u32,
    pub show_gpu: bool,
    pub show_host: bool,
    pub show_requests: bool,
    pub max_fps: u32,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct SourceConfig {
    pub enabled: bool,
    pub interval_ms: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct LiteLlmConfig {
    /// Base URL of the LiteLLM proxy, e.g. https://litellm.example.com
    #[serde(default)]
    pub url: Option<String>,
    /// Read-only key (a proxy_admin_viewer user; can be limited to /spend/logs/v2).
    #[serde(default)]
    pub api_key: Option<String>,
    /// File holding the key (keeps the secret out of this config); `~/` is expanded.
    #[serde(default)]
    pub api_key_file: Option<String>,
    /// Environment variable holding the key.
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default = "default_litellm_poll")]
    pub poll_s: f64,
    /// The old ssh + psql source; recognized so old files still parse, but no longer used.
    #[serde(default)]
    pub command: Option<Vec<String>>,
}

pub const LITELLM_API_KEY_ENV: &str = "LITELLM_API_KEY";

impl LiteLlmConfig {
    /// `api_key`, then `api_key_file`, then the variable named by `api_key_env`, then LITELLM_API_KEY.
    pub fn resolve_api_key(&self) -> Option<String> {
        let clean = |k: String| Some(k.trim().to_string()).filter(|k| !k.is_empty());
        let env = |name: &str| std::env::var(name).ok().and_then(clean);
        let file = |p: &str| {
            let path = match p.strip_prefix("~/") {
                Some(rest) => dirs::home_dir().map(|h| h.join(rest)).unwrap_or_else(|| PathBuf::from(p)),
                None => PathBuf::from(p),
            };
            std::fs::read_to_string(path).ok().and_then(clean)
        };
        self.api_key
            .clone()
            .and_then(clean)
            .or_else(|| self.api_key_file.as_deref().and_then(file))
            .or_else(|| self.api_key_env.as_deref().and_then(env))
            .or_else(|| env(LITELLM_API_KEY_ENV))
    }
}

fn default_litellm_poll() -> f64 {
    5.0
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct EndpointConfig {
    pub name: String,
    /// Full metrics URL, e.g. http://localhost:8000/metrics
    pub url: String,
    /// Draft lengths in use when the server varies k by batch size (e.g. [3, 5]),
    /// used to get exact per-position acceptance rates.
    #[serde(default)]
    pub spec_k_set: Vec<u32>,
    /// SQL LIKE pattern on LiteLLM_SpendLogs.model_group for this endpoint's traffic.
    #[serde(default)]
    pub litellm_model_group: Option<String>,
    /// vLLM API key (the server's --api-key), sent as `Authorization: Bearer`.
    #[serde(default)]
    pub api_key: Option<String>,
    /// Name of an environment variable holding the API key, so the key itself stays out of this file.
    #[serde(default)]
    pub api_key_env: Option<String>,
}

/// vLLM's own variable for the server key; the last fallback for every endpoint.
pub const VLLM_API_KEY_ENV: &str = "VLLM_API_KEY";

impl EndpointConfig {
    /// The key to send: `--api-key`, then `api_key`, then the variable named by `api_key_env`,
    /// then `VLLM_API_KEY`. Empty values count as unset.
    pub fn resolve_api_key(&self, cli: Option<&str>) -> Option<String> {
        let clean = |k: String| Some(k.trim().to_string()).filter(|k| !k.is_empty());
        let env = |name: &str| std::env::var(name).ok().and_then(clean);
        cli.map(str::to_string)
            .and_then(clean)
            .or_else(|| self.api_key.clone().and_then(clean))
            .or_else(|| self.api_key_env.as_deref().and_then(env))
            .or_else(|| env(VLLM_API_KEY_ENV))
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            interval_ms: 500,
            history_hours: 24.0,
            legacy_window: None,
            ui: UiConfig::default(),
            gpu: SourceConfig::default(),
            host: SourceConfig::default(),
            litellm: None,
            endpoints: vec![],
        }
    }
}

impl Default for UiConfig {
    fn default() -> Self {
        UiConfig {
            theme: "lil".into(),
            background: "theme".into(),
            window: None,
            peaks: vec!["1m".into(), "5m".into(), "15m".into()],
            decode_now_s: 2,
            prefill_now_s: 3,
            show_gpu: true,
            show_host: true,
            show_requests: true,
            max_fps: 25,
        }
    }
}

impl Default for SourceConfig {
    fn default() -> Self {
        SourceConfig { enabled: true, interval_ms: 1000 }
    }
}

impl Config {
    pub fn window(&self) -> &str {
        self.ui.window.as_deref().or(self.legacy_window.as_deref()).unwrap_or("5m")
    }

    /// Peak windows in seconds with their labels (at most three, invalid entries dropped).
    pub fn peaks(&self) -> Vec<(i64, String)> {
        let v: Vec<(i64, String)> =
            self.ui.peaks.iter().filter_map(|p| parse_duration(p).map(|s| (s, p.trim().to_string()))).take(3).collect();
        if v.is_empty() { vec![(60, "1m".into()), (300, "5m".into()), (900, "15m".into())] } else { v }
    }

    pub fn default_endpoint() -> EndpointConfig {
        EndpointConfig {
            name: "local".into(),
            url: "http://localhost:8000/metrics".into(),
            spec_k_set: vec![],
            litellm_model_group: None,
            api_key: None,
            api_key_env: None,
        }
    }
}

/// "30s", "5m", "1h", or plain seconds.
pub fn parse_duration(s: &str) -> Option<i64> {
    let s = s.trim();
    let (num, mul) = match s.chars().last()? {
        's' => (&s[..s.len() - 1], 1),
        'm' => (&s[..s.len() - 1], 60),
        'h' => (&s[..s.len() - 1], 3600),
        _ => (s, 1),
    };
    let n: f64 = num.trim().parse().ok()?;
    let secs = (n * mul as f64).round() as i64;
    (secs > 0).then_some(secs)
}

pub fn config_path() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("lilmon").join("config.toml")
}

pub fn state_dir() -> PathBuf {
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("lilmon")
}

pub fn parse(txt: &str) -> Result<Config> {
    Ok(toml::from_str(txt)?)
}

/// Loads `path`, or the default location. A missing default file yields the built-in defaults.
pub fn load(path: Option<&PathBuf>) -> Result<(Config, PathBuf, bool)> {
    let p = path.cloned().unwrap_or_else(config_path);
    if !p.exists() {
        if path.is_some() {
            anyhow::bail!("config file {} not found", p.display());
        }
        return Ok((Config::default(), p, false));
    }
    let txt = std::fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?;
    let cfg = parse(&txt).with_context(|| format!("parsing {}", p.display()))?;
    Ok((cfg, p, true))
}

/// Writes `cfg` to `path` as a commented file, keeping any previous file as `<path>.bak`.
pub fn write(path: &Path, cfg: &Config) -> Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).with_context(|| format!("creating {}", d.display()))?;
    }
    if path.exists() {
        std::fs::copy(path, path.with_extension("toml.bak")).context("backing up the previous config")?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, render(cfg))?;
    if cfg.endpoints.iter().any(|e| e.api_key.is_some()) || cfg.litellm.as_ref().is_some_and(|l| l.api_key.is_some()) {
        // the file holds a secret: owner read/write only
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Accepts a bare port (`8001`, `:8001` → localhost), `host:port`, `http://host:port`, or a full
/// `/metrics` URL.
pub fn normalize_url(u: &str) -> String {
    let mut s = u.trim().to_string();
    let port = s.strip_prefix(':').unwrap_or(&s);
    if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) {
        s = format!("localhost:{port}");
    }
    if !s.starts_with("http://") && !s.starts_with("https://") {
        s = format!("http://{s}");
    }
    let path_start = s.find("://").map(|i| i + 3).and_then(|i| s[i..].find('/').map(|j| i + j));
    if path_start.is_none() {
        s.push_str("/metrics");
    }
    s
}

pub fn name_from_url(u: &str) -> String {
    let s = u.split("://").nth(1).unwrap_or(u);
    s.split('/').next().unwrap_or(s).to_string()
}

fn q(s: &str) -> String {
    toml::Value::String(s.to_string()).to_string()
}

fn num(v: f64) -> String {
    if v.fract() == 0.0 { format!("{v:.0}") } else { format!("{v}") }
}

/// The config as a fully commented TOML file.
pub fn render(c: &Config) -> String {
    render_opts(c, false)
}

/// Like `render`, with API keys replaced by a placeholder (for printing to a terminal).
pub fn render_redacted(c: &Config) -> String {
    render_opts(c, true)
}

fn render_opts(c: &Config, redact: bool) -> String {
    let themes: Vec<&str> = crate::ui::theme::THEMES.iter().map(|t| t.name).collect();
    let mut o = String::new();
    let _ = writeln!(o, "# lilmon configuration");
    let _ = writeln!(o, "#");
    let _ = writeln!(o, "# Every key is optional; anything left out takes the default shown by `lilmon --init-config`.");
    let _ = writeln!(o, "# Command-line flags override this file for one run. Inside lilmon, `s` opens the running");
    let _ = writeln!(o, "# config (live settings) and `w` there writes it back here, keeping the old file as .bak.");
    let _ = writeln!(o);
    let _ = writeln!(o, "# How often to scrape each endpoint's /metrics, in milliseconds.");
    let _ = writeln!(o, "interval_ms = {}", c.interval_ms);
    let _ = writeln!(o);
    let _ = writeln!(o, "# Hours of 1-second history kept in memory and in ~/.local/state/lilmon/.");
    let _ = writeln!(o, "history_hours = {}", num(c.history_hours));
    let _ = writeln!(o);
    let _ = writeln!(o, "[ui]");
    let _ = writeln!(o, "# Color theme; `lilmon --list-themes` shows each with swatches and a description:");
    let chunks: Vec<_> = themes.chunks(7).collect();
    for (i, chunk) in chunks.iter().enumerate() {
        let sep = if i + 1 < chunks.len() { "," } else { "" };
        let _ = writeln!(o, "#   {}{sep}", chunk.join(", "));
    }
    let _ = writeln!(o, "theme = {}", q(&c.ui.theme));
    let _ = writeln!(o);
    let _ = writeln!(o, "# Background painted behind the whole UI:");
    let _ = writeln!(o, "#   \"theme\"   the theme's own surface (avoids patchy cells on transparent terminals)");
    let _ = writeln!(o, "#   \"black\"   pure black");
    let _ = writeln!(o, "#   \"none\"    leave the terminal's background alone (keeps transparency)");
    let _ = writeln!(o, "#   \"#rrggbb\" any color");
    let _ = writeln!(o, "background = {}", q(&c.ui.background));
    let _ = writeln!(o);
    let _ = writeln!(o, "# Starting chart window: 1m 5m 15m 1h 6h 24h.");
    let _ = writeln!(o, "window = {}", q(c.window()));
    let _ = writeln!(o);
    let _ = writeln!(o, "# Trailing windows for the peak readouts next to decode and prefill (up to three; s, m or h).");
    let peaks: Vec<String> = c.ui.peaks.iter().map(|p| q(p)).collect();
    let _ = writeln!(o, "peaks = [{}]", peaks.join(", "));
    let _ = writeln!(o);
    let _ = writeln!(o, "# Seconds averaged for the big \"now\" numbers.");
    let _ = writeln!(o, "decode_now_s = {}", c.ui.decode_now_s);
    let _ = writeln!(o, "prefill_now_s = {}", c.ui.prefill_now_s);
    let _ = writeln!(o);
    let _ = writeln!(o, "# Panels visible at start (toggle live with g, c and l).");
    let _ = writeln!(o, "show_gpu = {}", c.ui.show_gpu);
    let _ = writeln!(o, "show_host = {}", c.ui.show_host);
    let _ = writeln!(o, "show_requests = {}", c.ui.show_requests);
    let _ = writeln!(o);
    let _ = writeln!(o, "# Redraw cap. Data still arrives every interval_ms; this only bounds repaint work.");
    let _ = writeln!(o, "max_fps = {}", c.ui.max_fps);
    let _ = writeln!(o);
    let _ = writeln!(o, "[gpu]");
    let _ = writeln!(o, "# Local GPU telemetry through NVML: util, memory, clocks, temperature, power.");
    let _ = writeln!(o, "enabled = {}", c.gpu.enabled);
    let _ = writeln!(o, "interval_ms = {}", c.gpu.interval_ms);
    let _ = writeln!(o);
    let _ = writeln!(o, "[host]");
    let _ = writeln!(o, "# Local CPU, memory composition and busiest processes from /proc.");
    let _ = writeln!(o, "enabled = {}", c.host.enabled);
    let _ = writeln!(o, "interval_ms = {}", c.host.interval_ms);
    let _ = writeln!(o);
    let _ = writeln!(o, "# Per-request rows from a LiteLLM proxy's spend log, read from GET /spend/logs/v2. Use a");
    let _ = writeln!(o, "# read-only key: a user with the proxy_admin_viewer role, ideally limited to that route with");
    let _ = writeln!(o, "# allowed_routes. Key lookup: api_key, api_key_file, the variable in api_key_env, then");
    let _ = writeln!(o, "# LITELLM_API_KEY. Each endpoint opts in with litellm_model_group.");
    match &c.litellm {
        Some(l) => {
            let _ = writeln!(o, "[litellm]");
            match &l.url {
                Some(u) => {
                    let _ = writeln!(o, "url = {}", q(u));
                }
                None => {
                    let _ = writeln!(o, "# url = \"https://litellm.example.com\"");
                }
            }
            match &l.api_key {
                Some(k) => {
                    let v = if redact { "<redacted>".to_string() } else { k.clone() };
                    let _ = writeln!(o, "api_key = {}", q(&v));
                }
                None => {
                    let _ = writeln!(o, "# api_key = \"sk-...\"");
                }
            }
            match &l.api_key_file {
                Some(f) => {
                    let _ = writeln!(o, "api_key_file = {}", q(f));
                }
                None => {
                    let _ = writeln!(o, "# api_key_file = \"~/.config/lilmon/litellm.key\"");
                }
            }
            match &l.api_key_env {
                Some(e) => {
                    let _ = writeln!(o, "api_key_env = {}", q(e));
                }
                None => {
                    let _ = writeln!(o, "# api_key_env = \"LITELLM_API_KEY\"");
                }
            }
            let _ = writeln!(o, "poll_s = {}", num(l.poll_s));
        }
        None => {
            let _ = writeln!(o, "# [litellm]");
            let _ = writeln!(o, "# url = \"https://litellm.example.com\"");
            let _ = writeln!(o, "# api_key_file = \"~/.config/lilmon/litellm.key\"");
            let _ = writeln!(o, "# poll_s = 5");
        }
    }
    let _ = writeln!(o);
    let _ = writeln!(o, "# One [[endpoint]] block per vLLM server; `url` may be host:port or a full /metrics URL.");
    let _ = writeln!(o, "#   spec_k_set           draft lengths when k varies by batch size, e.g. [3, 5], for exact");
    let _ = writeln!(o, "#                        per-position MTP acceptance");
    let _ = writeln!(o, "#   litellm_model_group  SQL LIKE pattern on LiteLLM_SpendLogs.model_group for request rows");
    let _ = writeln!(o, "#   api_key              the server's --api-key, sent as a Bearer token (this file is then");
    let _ = writeln!(o, "#                        written owner-only)");
    let _ = writeln!(o, "#   api_key_env          name of an environment variable holding the key instead");
    let _ = writeln!(o, "#   With neither set, lilmon uses $VLLM_API_KEY when present; --api-key overrides all.");
    let eps = if c.endpoints.is_empty() { vec![Config::default_endpoint()] } else { c.endpoints.clone() };
    for e in &eps {
        let _ = writeln!(o, "[[endpoint]]");
        let _ = writeln!(o, "name = {}", q(&e.name));
        let _ = writeln!(o, "url = {}", q(&e.url));
        if e.spec_k_set.is_empty() {
            let _ = writeln!(o, "# spec_k_set = [3, 5]");
        } else {
            let ks: Vec<String> = e.spec_k_set.iter().map(|k| k.to_string()).collect();
            let _ = writeln!(o, "spec_k_set = [{}]", ks.join(", "));
        }
        match &e.litellm_model_group {
            Some(p) => {
                let _ = writeln!(o, "litellm_model_group = {}", q(p));
            }
            None => {
                let _ = writeln!(o, "# litellm_model_group = \"myserver/%\"");
            }
        }
        match &e.api_key {
            Some(k) => {
                let v = if redact { "<redacted>".to_string() } else { k.clone() };
                let _ = writeln!(o, "api_key = {}", q(&v));
            }
            None => {
                let _ = writeln!(o, "# api_key = \"sk-...\"");
            }
        }
        match &e.api_key_env {
            Some(v) => {
                let _ = writeln!(o, "api_key_env = {}", q(v));
            }
            None => {
                let _ = writeln!(o, "# api_key_env = \"VLLM_API_KEY\"");
            }
        }
        let _ = writeln!(o);
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rendered_defaults_parse_back_to_defaults() {
        let d = Config::default();
        let mut back = parse(&render(&d)).unwrap();
        assert_eq!(back.endpoints, vec![Config::default_endpoint()]);
        back.endpoints.clear();
        back.ui.window = None;
        assert_eq!(back, d);
    }

    #[test]
    fn rendered_custom_config_round_trips() {
        let mut c = Config::default();
        c.interval_ms = 250;
        c.ui.theme = "nord".into();
        c.ui.background = "#101010".into();
        c.ui.window = Some("15m".into());
        c.ui.peaks = vec!["30s".into(), "10m".into()];
        c.litellm = Some(LiteLlmConfig {
            url: Some("https://litellm.example.com".into()),
            api_key: None,
            api_key_file: Some("~/.config/lilmon/litellm.key".into()),
            api_key_env: Some("MY_LITELLM_KEY".into()),
            poll_s: 2.5,
            command: None,
        });
        c.endpoints = vec![
            EndpointConfig {
                name: "lab".into(),
                url: "http://localhost:8001/metrics".into(),
                spec_k_set: vec![3, 5],
                litellm_model_group: Some("lab/%".into()),
                api_key: Some("sk-test \"quoted\"".into()),
                api_key_env: None,
            },
            EndpointConfig {
                name: "other".into(),
                url: "http://otherhost:8000/metrics".into(),
                spec_k_set: vec![],
                litellm_model_group: None,
                api_key: None,
                api_key_env: Some("OTHER_KEY".into()),
            },
        ];
        assert_eq!(parse(&render(&c)).unwrap(), c);
    }

    #[test]
    fn redacted_render_hides_keys() {
        let mut c = Config::default();
        c.endpoints = vec![EndpointConfig { api_key: Some("sk-secret-123".into()), ..Config::default_endpoint() }];
        let txt = render_redacted(&c);
        assert!(!txt.contains("sk-secret-123"));
        assert!(txt.contains("api_key = \"<redacted>\""));
        assert!(render(&c).contains("sk-secret-123"));
    }

    #[test]
    fn litellm_key_redaction_and_file() {
        let dir = std::env::temp_dir().join(format!("lilmon-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let kf = dir.join("k");
        std::fs::write(&kf, "sk-from-file\n").unwrap();
        let mut l = LiteLlmConfig {
            url: Some("https://x".into()),
            api_key: Some("sk-inline".into()),
            api_key_file: Some(kf.display().to_string()),
            api_key_env: None,
            poll_s: 5.0,
            command: None,
        };
        assert_eq!(l.resolve_api_key().as_deref(), Some("sk-inline"));
        l.api_key = None;
        assert_eq!(l.resolve_api_key().as_deref(), Some("sk-from-file"));
        let c = Config { litellm: Some(LiteLlmConfig { api_key: Some("sk-inline".into()), ..l }), ..Config::default() };
        assert!(!render_redacted(&c).contains("sk-inline"));
        assert_eq!(parse(&render(&c)).unwrap().litellm, c.litellm);
        // old ssh + psql configs still parse
        assert!(parse("[litellm]\ncommand = [\"ssh\", \"db\"]\n").unwrap().litellm.unwrap().command.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn api_key_resolution_order() {
        // SAFETY: test-only env manipulation with names no other test reads.
        unsafe {
            std::env::set_var("LILMON_TEST_KEY_A", "from-env-a");
        }
        let mut e = Config::default_endpoint();
        e.api_key_env = Some("LILMON_TEST_KEY_A".into());
        assert_eq!(e.resolve_api_key(None).as_deref(), Some("from-env-a"));
        e.api_key = Some("from-file".into());
        assert_eq!(e.resolve_api_key(None).as_deref(), Some("from-file"));
        assert_eq!(e.resolve_api_key(Some("from-cli")).as_deref(), Some("from-cli"));
        // blank values count as unset, so a whitespace key falls through to the env variable
        e.api_key = Some("  ".into());
        assert_eq!(e.resolve_api_key(Some("")).as_deref(), Some("from-env-a"));
    }

    #[test]
    fn normalize_urls() {
        assert_eq!(normalize_url("8001"), "http://localhost:8001/metrics");
        assert_eq!(normalize_url(":8001"), "http://localhost:8001/metrics");
        assert_eq!(normalize_url("box:8000"), "http://box:8000/metrics");
        assert_eq!(normalize_url("https://box/x/metrics"), "https://box/x/metrics");
    }

    #[test]
    fn legacy_top_level_window() {
        let c = parse("window = \"1h\"\n").unwrap();
        assert_eq!(c.window(), "1h");
        let c = parse("window = \"1h\"\n[ui]\nwindow = \"15m\"\n").unwrap();
        assert_eq!(c.window(), "15m");
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("90s"), Some(90));
        assert_eq!(parse_duration("5m"), Some(300));
        assert_eq!(parse_duration("1h"), Some(3600));
        assert_eq!(parse_duration("0m"), None);
        assert_eq!(parse_duration("x"), None);
    }
}
