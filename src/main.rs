// Copyright 2026 Local Inference Lab, Inc.
// SPDX-License-Identifier: Apache-2.0

mod config;
mod gpu;
mod host;
mod litellm;
mod prom;
mod state;
mod ui;

use anyhow::Result;
use clap::Parser;
use config::{Config, EndpointConfig};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use serde::{Deserialize, Serialize};
use state::{EndpointState, Field};
use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use ui::theme::{self, Background, THEMES};
use std::time::{Duration, Instant};

#[derive(Parser, Debug)]
#[command(
    version,
    about = "lilmon, the Local Inference Lab monitor: a terminal dashboard for vLLM's Prometheus metrics",
    after_help = "Copyright 2026 Local Inference Lab, Inc. Licensed under the Apache License, Version 2.0."
)]
struct Cli {
    /// Metrics endpoints (host:port or full URL); replaces the endpoints in the config file
    urls: Vec<String>,
    /// Config file (default ~/.config/lilmon/config.toml)
    #[arg(short, long)]
    config: Option<PathBuf>,
    /// Scrape interval in milliseconds
    #[arg(short, long)]
    interval: Option<u64>,
    /// Color theme (see --list-themes)
    #[arg(short, long)]
    theme: Option<String>,
    /// Background: theme, black, none (keep terminal transparency) or #rrggbb
    #[arg(long, value_name = "BG")]
    bg: Option<String>,
    /// Starting chart window: 1m 5m 15m 1h 6h 24h
    #[arg(short, long)]
    window: Option<String>,
    /// List the themes with swatches and descriptions, then exit
    #[arg(long)]
    list_themes: bool,
    /// Print the effective config (defaults + file + flags) as a commented file, then exit
    #[arg(long)]
    print_config: bool,
    /// Write the effective config to the config file, then exit (needs --force if it exists)
    #[arg(long)]
    init_config: bool,
    /// Allow --init-config to replace an existing file (the old one is kept as .bak)
    #[arg(long)]
    force: bool,
    /// vLLM API key for every endpoint, this run only (prefer api_key_env or VLLM_API_KEY: command
    /// lines are visible to other users)
    #[arg(long, value_name = "KEY")]
    api_key: Option<String>,
    /// Draft lengths in use (e.g. 3,5) for exact per-position acceptance on CLI endpoints
    #[arg(long, value_delimiter = ',')]
    spec_k: Vec<u32>,
    /// Disable the NVML GPU panel
    #[arg(long)]
    no_gpu: bool,
    /// Disable the host CPU / memory panel
    #[arg(long)]
    no_host: bool,
    /// Disable the LiteLLM request panel
    #[arg(long)]
    no_litellm: bool,
    /// Don't load or save history
    #[arg(long)]
    no_history: bool,
    /// Headless: collect for N seconds, print one rendered frame as text, and exit
    #[arg(long, value_name = "SECS")]
    snapshot: Option<f64>,
    /// Frame size for --snapshot
    #[arg(long, default_value = "200x56")]
    size: String,
    /// With --snapshot: emit one JSON array of [symbol, fg, bg, bold] cells per row
    #[arg(long, hide = true)]
    cells: bool,
    /// With --snapshot: open an overlay first (help, records, settings)
    #[arg(long, hide = true)]
    overlay: Option<String>,
    /// With --snapshot: write cell JSON for each of --snapshot-themes into this directory
    /// (<theme>.jsonl and <theme>-settings.jsonl), all from the same collected data
    #[arg(long, hide = true)]
    snapshot_out: Option<PathBuf>,
    #[arg(long, hide = true, value_delimiter = ',')]
    snapshot_themes: Vec<String>,
    /// Print a JSON map from every color theme FROM can draw to the matching color in TO
    /// (FROM:TO), used to re-color captured screenshot frames
    #[arg(long, hide = true, value_name = "FROM:TO")]
    theme_map: Option<String>,
    /// With --snapshot-out: also write frame_NNNN.jsonl every this many seconds while collecting
    #[arg(long, hide = true)]
    snapshot_every: Option<f64>,
}

pub enum Msg {
    Scrape(usize, Box<prom::Scrape>, f64),
    ScrapeErr(usize, String, f64),
    Info(usize, ServerInfo),
    Gpu(Vec<gpu::GpuSample>, f64),
    GpuUnavailable,
    Host(Box<host::HostSample>, f64),
    HostUnavailable,
    LiteLlm(usize, Vec<litellm::Row>, f64),
    LiteLlmErr(usize, String),
    Input(Event),
}

#[derive(Clone, Debug, Default)]
pub struct ServerInfo {
    pub model: String,
    pub max_model_len: Option<u64>,
}

#[derive(Serialize, Deserialize, Default, Clone, Copy, Debug)]
pub struct Rec {
    pub v: f64,
    pub at: f64,
}

impl Rec {
    fn offer(&mut self, v: f64, at: f64) -> bool {
        if v.is_finite() && v > self.v {
            self.v = v;
            self.at = at;
            true
        } else {
            false
        }
    }
}

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct ModelRecords {
    pub decode: Rec,
    pub prefill: Rec,
    pub prefill_req: Rec,
    pub running: Rec,
    pub kv: Rec,
    pub accept_len: Rec,
    #[serde(default)]
    pub first_seen: f64,
}

pub struct Endpoint {
    pub cfg: EndpointConfig,
    pub st: EndpointState,
    pub err: Option<(String, f64)>,
    pub last_ok: Option<f64>,
    pub scrape_ms: f64,
    pub info: Option<ServerInfo>,
    pub ll_rows: Vec<litellm::Row>,
    pub ll_t: Option<f64>,
    pub ll_err: Option<String>,
}

impl Endpoint {
    pub fn model(&self) -> String {
        self.st
            .last()
            .and_then(|s| s.models.first().cloned())
            .or_else(|| self.info.as_ref().map(|i| i.model.clone()))
            .unwrap_or_else(|| "—".into())
    }
    pub fn record_key(&self) -> String {
        format!("{}/{}", self.cfg.name, self.model())
    }
}

#[derive(Default)]
pub struct HostState {
    pub last: Option<host::HostSample>,
    /// (unix second, busy %) and (unix second, used bytes as f32), last hour.
    pub cpu: VecDeque<(i64, f32)>,
    pub mem: VecDeque<(i64, f32)>,
    pub t: f64,
    pub unavailable: bool,
    pub enabled: bool,
}

fn push_sec(h: &mut VecDeque<(i64, f32)>, sec: i64, v: f32) {
    match h.back_mut() {
        Some(last) if last.0 == sec => last.1 = v,
        _ => h.push_back((sec, v)),
    }
    while h.front().is_some_and(|f| f.0 < sec - 3600) {
        h.pop_front();
    }
}

#[derive(Default)]
pub struct GpuState {
    pub samples: Vec<gpu::GpuSample>,
    /// (unix second, util %) per GPU, one entry per second, last hour.
    pub util: BTreeMap<u32, VecDeque<(i64, f32)>>,
    pub t: f64,
    pub unavailable: bool,
    pub enabled: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    None,
    Help,
    Records,
    Settings,
}

/// Rows of the settings overlay (the running config).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Setting {
    Theme,
    Background,
    Window,
    Interval,
    Gpu,
    Host,
    Requests,
}

pub const SETTINGS: [Setting; 7] =
    [Setting::Theme, Setting::Background, Setting::Window, Setting::Interval, Setting::Gpu, Setting::Host, Setting::Requests];

pub const INTERVALS: [u64; 6] = [250, 500, 1000, 2000, 5000, 10000];

pub const WINDOWS: [(i64, &str); 6] = [(60, "1m"), (300, "5m"), (900, "15m"), (3600, "1h"), (21600, "6h"), (86400, "24h")];

pub struct App {
    pub cfg: Config,
    pub eps: Vec<Endpoint>,
    pub sel: usize,
    pub win: usize,
    pub paused: bool,
    pub overlay: Overlay,
    pub show_gpu: bool,
    pub show_host: bool,
    pub host: HostState,
    pub show_req: bool,
    pub gpu: GpuState,
    pub records: BTreeMap<String, ModelRecords>,
    pub litellm_on: bool,
    records_dirty: bool,
    /// Running config: the file's values plus flags and live changes (see `running_config`).
    pub config_path: PathBuf,
    /// Rendering of the config as it is on disk, to flag unsaved changes.
    pub saved_render: String,
    pub bg: Background,
    /// A custom background from the file or flags, offered in the background cycle.
    pub bg_custom: Option<Background>,
    pub interval_ms: Arc<AtomicU64>,
    pub set_sel: usize,
    pub toast: Option<(String, Instant)>,
    pub peaks: Vec<(i64, String)>,
    /// `--api-key`: runtime only, never part of the running config or any rendering.
    cli_api_key: Option<String>,
}

impl App {
    pub fn ep(&self) -> &Endpoint {
        &self.eps[self.sel]
    }
    pub fn window(&self) -> (i64, &'static str) {
        WINDOWS[self.win]
    }

    /// The config as it stands now: file values, flags, and anything changed live.
    pub fn running_config(&self) -> Config {
        let mut c = self.cfg.clone();
        c.interval_ms = self.interval_ms.load(Ordering::Relaxed);
        c.legacy_window = None;
        c.ui.theme = theme::th().name.to_string();
        c.ui.background = self.bg.label();
        c.ui.window = Some(WINDOWS[self.win].1.to_string());
        c.ui.show_gpu = self.show_gpu;
        c.ui.show_host = self.show_host;
        c.ui.show_requests = self.show_req;
        c
    }

    pub fn config_modified(&self) -> bool {
        config::render(&self.running_config()) != self.saved_render
    }

    pub fn toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now()));
    }

    fn cycle_theme(&mut self, d: isize) {
        let n = THEMES.len() as isize;
        let i = (theme::current_index() as isize + d).rem_euclid(n) as usize;
        theme::set_index(i);
        self.toast(format!("theme {}: {}", THEMES[i].name, THEMES[i].description));
    }

    fn background_choices(&self) -> Vec<Background> {
        let mut v = vec![Background::Theme, Background::Color(theme::rgb(0x000000)), Background::None];
        if let Some(c) = self.bg_custom.filter(|c| !v.contains(c)) {
            v.push(c);
        }
        v
    }

    fn cycle_background(&mut self, d: isize) {
        let ch = self.background_choices();
        let i = ch.iter().position(|b| *b == self.bg).unwrap_or(0) as isize;
        self.bg = ch[(i + d).rem_euclid(ch.len() as isize) as usize];
        let what = match self.bg {
            Background::Theme => "the theme's own surface",
            Background::None => "terminal default (transparent)",
            Background::Color(_) => "solid color",
        };
        self.toast(format!("background {}: {what}", self.bg.label()));
    }

    fn change_setting(&mut self, s: Setting, d: isize) {
        match s {
            Setting::Theme => self.cycle_theme(d),
            Setting::Background => self.cycle_background(d),
            Setting::Window => self.win = (self.win as isize + d).clamp(0, WINDOWS.len() as isize - 1) as usize,
            Setting::Interval => {
                let cur = self.interval_ms.load(Ordering::Relaxed);
                let i = INTERVALS.iter().position(|&v| v >= cur).unwrap_or(INTERVALS.len() - 1) as isize;
                let v = INTERVALS[(i + d).clamp(0, INTERVALS.len() as isize - 1) as usize];
                self.interval_ms.store(v, Ordering::Relaxed);
            }
            Setting::Gpu => self.show_gpu = !self.show_gpu,
            Setting::Host => self.show_host = !self.show_host,
            Setting::Requests => self.show_req = !self.show_req,
        }
    }

    fn write_config(&mut self) {
        let rc = self.running_config();
        match config::write(&self.config_path, &rc) {
            Ok(()) => {
                self.saved_render = config::render(&rc);
                self.toast(format!("saved running config to {}", self.config_path.display()));
            }
            Err(e) => self.toast(format!("could not write {}: {e:#}", self.config_path.display())),
        }
    }

    fn handle(&mut self, m: Msg) -> bool {
        match m {
            Msg::Scrape(i, s, ms) => {
                let ep = &mut self.eps[i];
                ep.last_ok = Some(s.t);
                ep.err = None;
                ep.scrape_ms = ms;
                ep.st.ingest(*s);
                self.update_records(i);
            }
            Msg::ScrapeErr(i, e, t) => self.eps[i].err = Some((e, t)),
            Msg::Info(i, info) => self.eps[i].info = Some(info),
            Msg::Gpu(v, t) => {
                let sec = t.floor() as i64;
                for g in &v {
                    let h = self.gpu.util.entry(g.index).or_default();
                    let u = g.util.unwrap_or(0) as f32;
                    match h.back_mut() {
                        Some(last) if last.0 == sec => last.1 = last.1.max(u),
                        _ => h.push_back((sec, u)),
                    }
                    while h.front().is_some_and(|f| f.0 < sec - 3600) {
                        h.pop_front();
                    }
                }
                self.gpu.samples = v;
                self.gpu.t = t;
            }
            Msg::GpuUnavailable => self.gpu.unavailable = true,
            Msg::Host(h, t) => {
                let sec = t.floor() as i64;
                push_sec(&mut self.host.cpu, sec, h.cpu * 100.0);
                push_sec(&mut self.host.mem, sec, h.mem_used() as f32);
                self.host.last = Some(*h);
                self.host.t = t;
            }
            Msg::HostUnavailable => self.host.unavailable = true,
            Msg::LiteLlm(i, rows, t) => {
                let ep = &mut self.eps[i];
                ep.ll_rows = rows;
                ep.ll_t = Some(t);
                ep.ll_err = None;
            }
            Msg::LiteLlmErr(i, e) => self.eps[i].ll_err = Some(e),
            Msg::Input(ev) => return self.key(ev),
        }
        false
    }

    /// Returns true to quit.
    fn key(&mut self, ev: Event) -> bool {
        let Event::Key(k) = ev else { return false };
        if k.kind != KeyEventKind::Press {
            return false;
        }
        if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
            return true;
        }
        if self.overlay == Overlay::Settings {
            let n = SETTINGS.len();
            match k.code {
                KeyCode::Up | KeyCode::Char('k') => self.set_sel = (self.set_sel + n - 1) % n,
                KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => self.set_sel = (self.set_sel + 1) % n,
                KeyCode::Left | KeyCode::Char('h') => self.change_setting(SETTINGS[self.set_sel], -1),
                KeyCode::Right | KeyCode::Char('l') | KeyCode::Enter | KeyCode::Char(' ') => {
                    self.change_setting(SETTINGS[self.set_sel], 1)
                }
                KeyCode::Char('w') => self.write_config(),
                KeyCode::Esc | KeyCode::Char('s') => self.overlay = Overlay::None,
                KeyCode::Char('q') => return true,
                _ => {}
            }
            return false;
        }
        match k.code {
            KeyCode::Char('q') => return true,
            KeyCode::Char('s') => self.overlay = Overlay::Settings,
            KeyCode::Char('t') => self.cycle_theme(1),
            KeyCode::Char('T') => self.cycle_theme(-1),
            KeyCode::Char('b') => self.cycle_background(1),
            KeyCode::Esc => {
                if self.overlay == Overlay::None {
                    return true;
                }
                self.overlay = Overlay::None;
            }
            KeyCode::Tab => self.sel = (self.sel + 1) % self.eps.len(),
            KeyCode::BackTab => self.sel = (self.sel + self.eps.len() - 1) % self.eps.len(),
            KeyCode::Char(c @ '1'..='9') => {
                let i = c as usize - '1' as usize;
                if i < self.eps.len() {
                    self.sel = i;
                }
            }
            KeyCode::Left | KeyCode::Char('[') | KeyCode::Char('-') => self.win = self.win.saturating_sub(1),
            KeyCode::Right | KeyCode::Char(']') | KeyCode::Char('+') | KeyCode::Char('=') => {
                self.win = (self.win + 1).min(WINDOWS.len() - 1)
            }
            KeyCode::Char('w') => self.win = (self.win + 1) % WINDOWS.len(),
            KeyCode::Char('r') => self.overlay = if self.overlay == Overlay::Records { Overlay::None } else { Overlay::Records },
            KeyCode::Char('?') | KeyCode::Char('h') => {
                self.overlay = if self.overlay == Overlay::Help { Overlay::None } else { Overlay::Help }
            }
            KeyCode::Char('g') => self.show_gpu = !self.show_gpu,
            KeyCode::Char('c') => self.show_host = !self.show_host,
            KeyCode::Char('l') => self.show_req = !self.show_req,
            KeyCode::Char(' ') | KeyCode::Char('p') => self.paused = !self.paused,
            _ => {}
        }
        false
    }

    fn update_records(&mut self, i: usize) {
        let ep = &self.eps[i];
        let Some(s) = ep.st.last() else { return };
        if s.models.is_empty() {
            return;
        }
        let t = s.t;
        let key = ep.record_key();
        let r = self.records.entry(key).or_insert_with(|| ModelRecords { first_seen: t, ..Default::default() });
        let mut changed = false;
        if let Some((v, sec)) = ep.st.peak(Field::Decode, 60) {
            changed |= r.decode.offer(v as f64, sec as f64);
        }
        if let Some((v, sec)) = ep.st.peak(Field::Prefill, 60) {
            changed |= r.prefill.offer(v as f64, sec as f64);
        }
        changed |= r.running.offer(s.running, t);
        changed |= r.kv.offer(s.kv_usage, t);
        if let Some(e) = ep.st.prefill_events.back().filter(|e| e.tokens >= 32768.0 && e.secs >= 0.25) {
            changed |= r.prefill_req.offer(e.tokens / e.secs, e.t);
        }
        let sp = ep.st.spec_window(60);
        if sp.drafts >= 500.0
            && let Some(al) = sp.accept_len() {
                changed |= r.accept_len.offer(al, t);
            }
        self.records_dirty |= changed;
    }

    fn save_records(&mut self, path: &PathBuf) {
        if !self.records_dirty {
            return;
        }
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(txt) = serde_json::to_string_pretty(&self.records) {
            let tmp = path.with_extension("tmp");
            if std::fs::write(&tmp, txt).is_ok() && std::fs::rename(&tmp, path).is_ok() {
                self.records_dirty = false;
            }
        }
    }
}

fn get(agent: &ureq::Agent, url: &str, key: Option<&str>) -> Result<String, ureq::Error> {
    let mut req = agent.get(url);
    if let Some(k) = key {
        req = req.header("Authorization", format!("Bearer {k}"));
    }
    req.call().and_then(|mut r| r.body_mut().read_to_string())
}

/// Scrape errors as shown in the header. Never includes key material.
fn describe_error(e: &ureq::Error, key_sent: bool) -> String {
    match e {
        ureq::Error::StatusCode(code @ (401 | 403)) if key_sent => format!("{code} API key rejected"),
        ureq::Error::StatusCode(code @ (401 | 403)) => {
            format!("{code} unauthorized — set api_key / api_key_env / VLLM_API_KEY")
        }
        other => other.to_string(),
    }
}

fn fetch_info(agent: &ureq::Agent, base: &str, key: Option<&str>) -> Option<ServerInfo> {
    let txt = get(agent, &format!("{base}/v1/models"), key).ok()?;
    let v: serde_json::Value = serde_json::from_str(&txt).ok()?;
    let m = v.get("data")?.get(0)?;
    Some(ServerInfo {
        model: m.get("id")?.as_str()?.to_string(),
        max_model_len: m.get("max_model_len").and_then(|x| x.as_u64()),
    })
}

fn spawn_poller(idx: usize, url: String, api_key: Option<String>, interval_ms: Arc<AtomicU64>, tx: Sender<Msg>) {
    std::thread::Builder::new()
        .name(format!("poll-{idx}"))
        .spawn(move || {
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(5)))
                .http_status_as_error(true)
                .build()
                .into();
            let base = url.strip_suffix("/metrics").unwrap_or(&url).to_string();
            let mut info_for: Option<Option<f64>> = None;
            let mut next = Instant::now();
            loop {
                let wall = state::now();
                let t0 = Instant::now();
                let res = get(&agent, &url, api_key.as_deref());
                let el = t0.elapsed().as_secs_f64();
                let msg = match res {
                    Ok(text) => {
                        let s = prom::parse(&text, wall + el / 2.0);
                        if info_for != Some(s.process_start)
                            && let Some(info) = fetch_info(&agent, &base, api_key.as_deref()) {
                                let _ = tx.send(Msg::Info(idx, info));
                                info_for = Some(s.process_start);
                            }
                        Msg::Scrape(idx, Box::new(s), el * 1000.0)
                    }
                    Err(e) => Msg::ScrapeErr(idx, describe_error(&e, api_key.is_some()), state::now()),
                };
                if tx.send(msg).is_err() {
                    return;
                }
                next += Duration::from_millis(interval_ms.load(Ordering::Relaxed).max(100));
                let nowi = Instant::now();
                if next < nowi {
                    next = nowi;
                }
                std::thread::sleep(next - nowi);
            }
        })
        .expect("spawn poller");
}

/// Fills in what the file may leave out, so the running config and the saved rendering agree.
fn normalize(cfg: &mut Config) {
    if cfg.endpoints.is_empty() {
        cfg.endpoints.push(Config::default_endpoint());
    }
    for e in &mut cfg.endpoints {
        e.url = config::normalize_url(&e.url);
    }
}

/// The file's config plus command-line overrides. Returns (file config, effective config, path, exists).
fn effective_config(cli: &Cli) -> Result<(Config, Config, PathBuf, bool)> {
    let (mut file_cfg, path, exists) = config::load(cli.config.as_ref())?;
    normalize(&mut file_cfg);
    let mut cfg = file_cfg.clone();
    if let Some(ms) = cli.interval {
        cfg.interval_ms = ms;
    }
    if let Some(t) = &cli.theme {
        let i = theme::find(t).ok_or_else(|| {
            let names: Vec<&str> = THEMES.iter().map(|t| t.name).collect();
            anyhow::anyhow!("unknown theme {t:?}; choose one of: {}", names.join(", "))
        })?;
        cfg.ui.theme = THEMES[i].name.to_string();
    }
    if let Some(b) = &cli.bg {
        Background::parse(b).ok_or_else(|| anyhow::anyhow!("--bg takes theme, black, none or #rrggbb (got {b:?})"))?;
        cfg.ui.background = b.clone();
    }
    if let Some(w) = &cli.window {
        if !WINDOWS.iter().any(|x| x.1 == w) {
            anyhow::bail!("--window takes one of 1m 5m 15m 1h 6h 24h (got {w:?})");
        }
        cfg.ui.window = Some(w.clone());
    }
    if !cli.urls.is_empty() {
        cfg.endpoints = cli
            .urls
            .iter()
            .map(|u| {
                let url = config::normalize_url(u);
                let existing = cfg.endpoints.iter().find(|e| e.url == url).cloned();
                existing.unwrap_or(EndpointConfig {
                    name: config::name_from_url(&url),
                    url,
                    spec_k_set: cli.spec_k.clone(),
                    litellm_model_group: None,
                    api_key: None,
                    api_key_env: None,
                })
            })
            .collect();
    }
    for e in &mut cfg.endpoints {
        if e.spec_k_set.is_empty() && !cli.spec_k.is_empty() {
            e.spec_k_set = cli.spec_k.clone();
        }
    }
    Ok((file_cfg, cfg, path, exists))
}

fn build_app(cli: &Cli, file_cfg: &Config, cfg: Config, path: PathBuf) -> Result<App> {
    let mut startup_note = None;
    match theme::find(&cfg.ui.theme) {
        Some(i) => theme::set_index(i),
        None => startup_note = Some(format!("unknown theme {:?} in config; using lil", cfg.ui.theme)),
    }
    let bg = Background::parse(&cfg.ui.background).unwrap_or_else(|| {
        startup_note = Some(format!("bad background {:?} in config; using the theme's", cfg.ui.background));
        Background::Theme
    });
    let bg_custom = matches!(bg, Background::Color(_)).then_some(bg);
    let hist_secs = (cfg.history_hours * 3600.0) as usize;
    let sdir = config::state_dir();
    let eps = cfg
        .endpoints
        .iter()
        .map(|e| {
            let safe: String = e.name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
            // --snapshot reads history but never persists (persist is only called from the TUI loop).
            let hp = (!cli.no_history).then(|| sdir.join(format!("{safe}.hist.csv")));
            Endpoint {
                cfg: e.clone(),
                st: EndpointState::new(hist_secs, e.spec_k_set.clone(), hp),
                err: None,
                last_ok: None,
                scrape_ms: 0.0,
                info: None,
                ll_rows: vec![],
                ll_t: None,
                ll_err: None,
            }
        })
        .collect();
    let records = if cli.no_history {
        BTreeMap::new()
    } else {
        std::fs::read_to_string(sdir.join("records.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    };
    let win = WINDOWS.iter().position(|w| w.1 == cfg.window()).unwrap_or(1);
    let litellm_on = !cli.no_litellm
        && cfg.litellm.as_ref().is_some_and(|l| l.url.is_some())
        && cfg.endpoints.iter().any(|e| e.litellm_model_group.is_some());
    if cfg.litellm.as_ref().is_some_and(|l| l.url.is_none() && l.command.is_some()) {
        startup_note = Some(
            "LiteLLM now reads the spend log over its API: set [litellm] url and a read-only key (the psql command is ignored)".into(),
        );
    }
    let mut app = App {
        gpu: GpuState { enabled: !cli.no_gpu && cfg.gpu.enabled, ..Default::default() },
        host: HostState { enabled: !cli.no_host && cfg.host.enabled, ..Default::default() },
        show_host: cfg.ui.show_host,
        show_gpu: cfg.ui.show_gpu,
        show_req: cfg.ui.show_requests,
        interval_ms: Arc::new(AtomicU64::new(cfg.interval_ms.max(100))),
        peaks: cfg.peaks(),
        saved_render: config::render(file_cfg),
        config_path: path,
        bg,
        bg_custom,
        set_sel: 0,
        toast: None,
        cli_api_key: cli.api_key.clone(),
        cfg,
        eps,
        sel: 0,
        win,
        paused: false,
        overlay: Overlay::None,
        records,
        litellm_on,
        records_dirty: false,
    };
    if let Some(n) = startup_note {
        app.toast(n);
    }
    Ok(app)
}

/// Carries data over from the tool's former name (`vllmtop`). Safe to run every start while the
/// old directories exist: the config is only created when lilmon has none, history gains only rows
/// newer than lilmon's own, and records keep the best value per field.
fn migrate_legacy() -> Vec<String> {
    let mut notes = vec![];
    let new_cfg = config::config_path();
    let old_cfg = new_cfg.parent().and_then(|p| p.parent()).map(|p| p.join("vllmtop").join("config.toml"));
    if let Some(old) = old_cfg.filter(|o| o.exists() && !new_cfg.exists()) {
        let done = std::fs::read_to_string(&old)
            .ok()
            .and_then(|t| config::parse(&t).ok())
            .map(|c| config::write(&new_cfg, &c).is_ok())
            .unwrap_or(false)
            || std::fs::create_dir_all(new_cfg.parent().unwrap()).and_then(|_| std::fs::copy(&old, &new_cfg)).is_ok();
        if done {
            notes.push(format!("migrated config from {}", old.display()));
        }
    }
    let new_state = config::state_dir();
    let Some(old_state) = new_state.parent().map(|p| p.join("vllmtop")) else { return notes };
    let Ok(rd) = std::fs::read_dir(&old_state) else { return notes };
    let _ = std::fs::create_dir_all(&new_state);
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let (old, new) = (e.path(), new_state.join(&name));
        if name.ends_with(".hist.csv") {
            if !new.exists() {
                if std::fs::copy(&old, &new).is_ok() {
                    notes.push(format!("migrated history {name}"));
                }
                continue;
            }
            // append legacy rows newer than lilmon's last row, if both files share a header
            let (Ok(a), Ok(b)) = (std::fs::read_to_string(&new), std::fs::read_to_string(&old)) else { continue };
            let (ha, hb) = (a.lines().next().unwrap_or(""), b.lines().next().unwrap_or(""));
            if ha != hb {
                continue;
            }
            let t_of = |l: &str| l.split(',').next().and_then(|x| x.parse::<i64>().ok());
            let last = a.lines().skip(1).filter_map(t_of).max().unwrap_or(i64::MIN);
            let extra: Vec<&str> = b.lines().skip(1).filter(|l| t_of(l).is_some_and(|t| t > last)).collect();
            if !extra.is_empty() {
                use std::io::Write;
                if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&new) {
                    let _ = writeln!(f, "{}", extra.join("\n"));
                    notes.push(format!("merged {} history rows from {name}", extra.len()));
                }
            }
        } else if name == "records.json" {
            let read = |p: &PathBuf| -> BTreeMap<String, ModelRecords> {
                std::fs::read_to_string(p).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
            };
            let mut mine = read(&new);
            let theirs = read(&old);
            let before = serde_json::to_string(&mine).unwrap_or_default();
            for (k, o) in theirs {
                let m = mine.entry(k).or_insert_with(|| ModelRecords { first_seen: o.first_seen, ..Default::default() });
                for (dst, src) in [
                    (&mut m.decode, o.decode),
                    (&mut m.prefill, o.prefill),
                    (&mut m.prefill_req, o.prefill_req),
                    (&mut m.running, o.running),
                    (&mut m.kv, o.kv),
                    (&mut m.accept_len, o.accept_len),
                ] {
                    dst.offer(src.v, src.at);
                }
                if o.first_seen > 0.0 && (m.first_seen == 0.0 || o.first_seen < m.first_seen) {
                    m.first_seen = o.first_seen;
                }
            }
            let after = serde_json::to_string(&mine).unwrap_or_default();
            if after != before
                && let Ok(txt) = serde_json::to_string_pretty(&mine)
                    && std::fs::write(&new, txt).is_ok() {
                        notes.push("merged records".into());
                    }
        }
    }
    notes
}

/// Every color theme `a` can draw, mapped to the same role in theme `b`. Tokens come first so they win
/// over gradient samples that round to the same value.
fn theme_map(spec: &str) -> Result<()> {
    let (a, b) = spec.split_once(':').ok_or_else(|| anyhow::anyhow!("--theme-map takes FROM:TO"))?;
    let find = |n: &str| theme::find(n).map(|i| &THEMES[i]).ok_or_else(|| anyhow::anyhow!("unknown theme {n}"));
    let (ta, tb) = (find(a)?, find(b)?);
    let mut m = serde_json::Map::new();
    let mut put = |x: ratatui::style::Color, y: ratatui::style::Color| {
        m.entry(color_hex(x)).or_insert_with(|| color_hex(y).into());
    };
    for (x, y) in [
        (ta.bg, tb.bg), (ta.text, tb.text), (ta.text2, tb.text2), (ta.muted, tb.muted), (ta.faint, tb.faint),
        (ta.border, tb.border), (ta.grid, tb.grid), (ta.track, tb.track), (ta.tab_bg, tb.tab_bg),
        (ta.good, tb.good), (ta.warn, tb.warn), (ta.serious, tb.serious), (ta.crit, tb.crit),
        (ta.mem_anon, tb.mem_anon), (ta.mem_shmem, tb.mem_shmem), (ta.mem_cache, tb.mem_cache), (ta.mem_other, tb.mem_other),
    ] {
        put(x, y);
    }
    let ramps = |t: &'static theme::Theme| [&t.decode, &t.prefill, &t.mtp, &t.kv, &t.gpu, &t.req, &t.cpu, &t.mem];
    for (ra, rb) in ramps(ta).into_iter().zip(ramps(tb)) {
        put(ra.key, rb.key);
        put(ra.band, rb.band);
        for i in 0..=4000 {
            let f = i as f64 / 4000.0;
            put(ra.at(f), rb.at(f));
        }
    }
    println!("{}", serde_json::Value::Object(m));
    Ok(())
}

fn list_themes(current: &str) {
    let fg = |c: ratatui::style::Color| match c {
        ratatui::style::Color::Rgb(r, g, b) => format!("\x1b[38;2;{r};{g};{b}m"),
        _ => String::new(),
    };
    let bgc = |c: ratatui::style::Color| match c {
        ratatui::style::Color::Rgb(r, g, b) => format!("\x1b[48;2;{r};{g};{b}m"),
        _ => String::new(),
    };
    println!("Themes (set [ui] theme in the config, pass --theme NAME, or press t inside lilmon):\n");
    for t in THEMES.iter() {
        let mark = if t.name == current { "*" } else { " " };
        let mut sw = format!("{} ", bgc(t.bg));
        for r in [&t.decode, &t.prefill, &t.mtp, &t.kv, &t.gpu, &t.req, &t.cpu, &t.mem] {
            sw.push_str(&format!("{}██", fg(r.key)));
        }
        sw.push_str(&format!(" {}Aa \x1b[0m", fg(t.text)));
        let light = if t.light { " [light]" } else { "" };
        println!("{mark} {:<12} {sw}  {}{light}", t.name, t.description);
    }
    println!("\nSwatches: decode, prefill, MTP, KV, GPU, requests, CPU, memory, then text on the theme's background.");
}

fn start_sources(app: &App, tx: &Sender<Msg>) {
    for (i, e) in app.eps.iter().enumerate() {
        let key = e.cfg.resolve_api_key(app.cli_api_key.as_deref());
        spawn_poller(i, e.cfg.url.clone(), key, app.interval_ms.clone(), tx.clone());
    }
    if app.gpu.enabled {
        gpu::spawn(tx.clone(), Duration::from_millis(app.cfg.gpu.interval_ms.max(200)));
    }
    if app.host.enabled {
        host::spawn(tx.clone(), Duration::from_millis(app.cfg.host.interval_ms.max(250)));
    }
    if app.litellm_on
        && let Some(l) = &app.cfg.litellm {
            let targets = app
                .eps
                .iter()
                .enumerate()
                .filter_map(|(i, e)| e.cfg.litellm_model_group.clone().map(|p| (i, p)))
                .collect();
            if let Some(url) = l.url.clone() {
                // resolved here, at spawn time, so the key never enters the running config
                litellm::spawn(tx.clone(), url, l.resolve_api_key(), Duration::from_secs_f64(l.poll_s.max(1.0)), targets);
            }
        }
}

fn color_hex(c: ratatui::style::Color) -> String {
    match c {
        ratatui::style::Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        _ => String::new(),
    }
}

/// One rendered frame as text lines, or as JSON arrays of [symbol, fg, bg, bold] per row.
fn frame_lines(app: &App, w: u16, h: u16, cells: bool) -> Result<Vec<String>> {
    use ratatui::{Terminal, backend::TestBackend};
    let mut term = Terminal::new(TestBackend::new(w, h))?;
    term.draw(|f| ui::draw(f, app))?;
    let buf = term.backend().buffer();
    Ok((0..h)
        .map(|y| {
            if cells {
                let row: Vec<serde_json::Value> = (0..w)
                    .map(|x| {
                        let c = &buf[(x, y)];
                        let bold = c.modifier.contains(ratatui::style::Modifier::BOLD);
                        serde_json::json!([c.symbol(), color_hex(c.fg), color_hex(c.bg), bold])
                    })
                    .collect();
                serde_json::Value::Array(row).to_string()
            } else {
                (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>().trim_end().to_string()
            }
        })
        .collect())
}

fn snapshot(
    mut app: App,
    secs: f64,
    size: &str,
    cells: bool,
    out: Option<&PathBuf>,
    themes: &[String],
    every: Option<f64>,
) -> Result<()> {
    let (w, h) = size.split_once('x').and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?))).unwrap_or((200u16, 56u16));
    let (tx, rx) = mpsc::channel();
    start_sources(&app, &tx);
    let end = Instant::now() + Duration::from_secs_f64(secs);
    let step = every.filter(|_| out.is_some()).map(Duration::from_secs_f64);
    let mut next_frame = step.map(|d| Instant::now() + d);
    let mut frame_no = 0u32;
    if let (Some(dir), Some(_)) = (out, step) {
        std::fs::create_dir_all(dir)?;
    }
    while let Some(left) = end.checked_duration_since(Instant::now()) {
        let wait = next_frame.map_or(left, |nf| left.min(nf.saturating_duration_since(Instant::now())));
        match rx.recv_timeout(wait) {
            Ok(m) => {
                app.handle(m);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if let (Some(nf), Some(d), Some(dir)) = (next_frame, step, out)
            && Instant::now() >= nf {
                let lines = frame_lines(&app, w, h, true)?;
                std::fs::write(dir.join(format!("frame_{frame_no:04}.jsonl")), lines.join("\n") + "\n")?;
                frame_no += 1;
                next_frame = Some(nf + d);
            }
    }
    use std::io::Write;
    if let Some(dir) = out {
        std::fs::create_dir_all(dir)?;
        let names: Vec<String> = if themes.is_empty() { vec![theme::th().name.to_string()] } else { themes.to_vec() };
        for name in names {
            let i = theme::find(&name).ok_or_else(|| anyhow::anyhow!("unknown theme {name}"))?;
            theme::set_index(i);
            for (suffix, ov) in [("", Overlay::None), ("-settings", Overlay::Settings)] {
                app.overlay = ov;
                let lines = frame_lines(&app, w, h, true)?;
                std::fs::write(dir.join(format!("{}{suffix}.jsonl", THEMES[i].name)), lines.join("\n") + "\n")?;
            }
        }
        return Ok(());
    }
    // Write through a lock and stop quietly if the reader goes away (e.g. piped into head).
    let mut stdout = std::io::stdout().lock();
    for line in frame_lines(&app, w, h, cells)? {
        if writeln!(stdout, "{line}").is_err() {
            break;
        }
    }
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let migrated = if cli.config.is_none() && !cli.no_history { migrate_legacy() } else { vec![] };
    let (file_cfg, cfg, path, exists) = effective_config(&cli)?;
    if let Some(spec) = &cli.theme_map {
        return theme_map(spec);
    }
    if cli.list_themes {
        list_themes(&cfg.ui.theme);
        return Ok(());
    }
    if cli.print_config {
        print!("{}", config::render_redacted(&cfg));
        return Ok(());
    }
    if cli.init_config {
        if exists && !cli.force {
            anyhow::bail!("{} already exists; add --force to replace it (the old file is kept as .bak)", path.display());
        }
        config::write(&path, &cfg)?;
        println!("wrote {}", path.display());
        return Ok(());
    }
    // First run with the default location: leave a fully commented file behind to edit.
    let created = !exists && cli.config.is_none() && cli.snapshot.is_none() && config::write(&path, &file_cfg).is_ok();
    let mut app = build_app(&cli, &file_cfg, cfg, path)?;
    if created {
        app.toast(format!("created {} with the defaults; press s for settings", app.config_path.display()));
    } else if !migrated.is_empty() {
        app.toast(format!("from vllmtop: {}", migrated.join(", ")));
    }
    if let Some(secs) = cli.snapshot {
        app.overlay = match cli.overlay.as_deref() {
            Some("help") => Overlay::Help,
            Some("records") => Overlay::Records,
            Some("settings") => Overlay::Settings,
            _ => Overlay::None,
        };
        return snapshot(app, secs, &cli.size, cli.cells, cli.snapshot_out.as_ref(), &cli.snapshot_themes, cli.snapshot_every);
    }
    let records_path = config::state_dir().join("records.json");

    let (tx, rx) = mpsc::channel();
    start_sources(&app, &tx);
    {
        let tx = tx.clone();
        std::thread::Builder::new().name("input".into()).spawn(move || {
            while let Ok(ev) = event::read() {
                if tx.send(Msg::Input(ev)).is_err() {
                    break;
                }
            }
        })?;
    }

    let mut terminal = ratatui::init();
    let frame = Duration::from_millis(1000 / app.cfg.ui.max_fps.clamp(1, 120) as u64);
    let mut last_draw = Instant::now() - Duration::from_secs(1);
    let mut last_persist = Instant::now();
    let mut dirty = true;
    let res: Result<()> = (|| {
        loop {
            let wait = if dirty { frame.saturating_sub(last_draw.elapsed()) } else { Duration::from_millis(250) };
            match rx.recv_timeout(wait) {
                Ok(m) => {
                    let is_input = matches!(m, Msg::Input(_));
                    if app.handle(m) {
                        return Ok(());
                    }
                    if is_input || !app.paused {
                        dirty = true;
                    }
                    while let Ok(m) = rx.try_recv() {
                        if app.handle(m) {
                            return Ok(());
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
            }
            if !app.paused && last_draw.elapsed() >= Duration::from_secs(1) {
                dirty = true;
            }
            if app.toast.as_ref().is_some_and(|(_, t)| t.elapsed() > Duration::from_secs(5)) {
                app.toast = None;
                dirty = true;
            }
            if dirty && last_draw.elapsed() >= frame {
                terminal.draw(|f| ui::draw(f, &app))?;
                last_draw = Instant::now();
                dirty = false;
            }
            if last_persist.elapsed() >= Duration::from_secs(30) {
                for e in &mut app.eps {
                    e.st.persist(false);
                }
                if !cli.no_history {
                    app.save_records(&records_path);
                }
                last_persist = Instant::now();
            }
        }
    })();
    ratatui::restore();
    for e in &mut app.eps {
        e.st.persist(true);
    }
    if !cli.no_history {
        app.save_records(&records_path);
    }
    res
}
