// Copyright 2026 Local Inference Lab, Inc.
// SPDX-License-Identifier: Apache-2.0

//! Local host telemetry from /proc: CPU (total and per core), memory composition, load average and
//! the busiest processes. Describes the machine lilmon runs on, not necessarily the endpoint's.

use std::collections::HashMap;
use std::fs;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default)]
pub struct ProcInfo {
    pub pid: i32,
    pub name: String,
    /// Percent of one core, like top.
    pub cpu: f32,
    pub rss: u64,
    pub vllm: bool,
}

#[derive(Clone, Debug, Default)]
pub struct HostSample {
    pub hostname: String,
    /// Busy fraction 0..1 across all cores.
    pub cpu: f32,
    pub iowait: f32,
    pub cores: Vec<f32>,
    pub mem_total: u64,
    pub mem_avail: u64,
    pub mem_free: u64,
    pub anon: u64,
    pub shmem: u64,
    /// Page cache minus shmem (reclaimable).
    pub cache: u64,
    pub swap_total: u64,
    pub swap_used: u64,
    pub load: [f32; 3],
    pub top: Vec<ProcInfo>,
    pub nprocs: usize,
}

impl HostSample {
    pub fn mem_used(&self) -> u64 {
        self.mem_total.saturating_sub(self.mem_avail)
    }
}

/// (busy, iowait, total) jiffies.
type Jiffies = (u64, u64, u64);

/// Jiffies for the aggregate line and for each core.
fn read_stat() -> Option<(Vec<Jiffies>, Vec<Jiffies>)> {
    let txt = fs::read_to_string("/proc/stat").ok()?;
    let mut all = vec![];
    let mut cores = vec![];
    for line in txt.lines() {
        let Some(rest) = line.strip_prefix("cpu") else { break };
        // "cpu  123 ..." is the aggregate line; "cpu7 123 ..." is a core.
        let aggregate = rest.starts_with(' ');
        let it = rest.split_whitespace().skip(usize::from(!aggregate));
        let v: Vec<u64> = it.filter_map(|x| x.parse().ok()).collect();
        if v.len() < 5 {
            continue;
        }
        // user nice system idle iowait irq softirq steal (guest is already in user)
        let total: u64 = v.iter().take(8).sum();
        let idle = v[3];
        let iow = v[4];
        let rec = (total - idle - iow, iow, total);
        if aggregate {
            all.push(rec);
        } else {
            cores.push(rec);
        }
    }
    Some((all, cores))
}

fn frac(now: Jiffies, prev: Jiffies) -> (f32, f32) {
    let dt = now.2.saturating_sub(prev.2) as f32;
    if dt <= 0.0 {
        return (0.0, 0.0);
    }
    (now.0.saturating_sub(prev.0) as f32 / dt, now.1.saturating_sub(prev.1) as f32 / dt)
}

fn meminfo(s: &mut HostSample) {
    let Ok(txt) = fs::read_to_string("/proc/meminfo") else { return };
    let mut m: HashMap<&str, u64> = HashMap::new();
    for line in txt.lines() {
        let mut it = line.split_whitespace();
        if let (Some(k), Some(v)) = (it.next(), it.next())
            && let Ok(v) = v.parse::<u64>() {
                m.insert(k.trim_end_matches(':'), v * 1024);
            }
    }
    let g = |k: &str| m.get(k).copied().unwrap_or(0);
    s.mem_total = g("MemTotal");
    s.mem_avail = g("MemAvailable");
    s.mem_free = g("MemFree");
    s.anon = g("AnonPages");
    s.shmem = g("Shmem");
    s.cache = (g("Cached") + g("Buffers")).saturating_sub(g("Shmem"));
    s.swap_total = g("SwapTotal");
    s.swap_used = g("SwapTotal").saturating_sub(g("SwapFree"));
}

fn getconf(name: &str, default: u64) -> u64 {
    std::process::Command::new("getconf")
        .arg(name)
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(default)
}

/// (comm, ppid, utime+stime ticks, starttime) from /proc/<pid>/stat.
fn proc_stat(pid: i32) -> Option<(String, u64, u64)> {
    let txt = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let l = txt.find('(')?;
    let r = txt.rfind(')')?;
    let comm = txt[l + 1..r].to_string();
    let f: Vec<&str> = txt[r + 2..].split(' ').collect();
    // fields after ")": state(0) ppid(1) ... utime(11) stime(12) ... starttime(19)
    let ticks = f.get(11)?.parse::<u64>().ok()? + f.get(12)?.parse::<u64>().ok()?;
    let start = f.get(19)?.parse().ok()?;
    Some((comm, ticks, start))
}

/// Human name: process titles (VLLM::EngineCore), `vllm serve :port`, python script names.
fn proc_name(pid: i32, comm: &str) -> (String, bool) {
    let raw = fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
    let args: Vec<String> = raw.split(|&b| b == 0).filter(|a| !a.is_empty()).map(|a| String::from_utf8_lossy(a).into_owned()).collect();
    let joined = args.join(" ").to_ascii_lowercase();
    let vllm = joined.contains("vllm");
    if let Some(a0) = args.first()
        && let Some(i) = a0.find("::") {
            // setproctitle-style titles: VLLM::EngineCore, vLLM-Omni::DiffusionWorker
            let title = a0.split_whitespace().next().unwrap_or(a0);
            return (title[i + 2..].to_string().chars().take(24).collect(), true);
        }
    if vllm && args.iter().any(|a| a == "serve") {
        let port = args.iter().position(|a| a == "--port").and_then(|i| args.get(i + 1)).map(|p| format!(":{p}")).unwrap_or_default();
        return (format!("vllm-api{port}"), true);
    }
    if comm.starts_with("python") {
        if let Some(script) = args.iter().skip(1).find(|a| a.ends_with(".py")) {
            let base = script.rsplit('/').next().unwrap_or(script);
            return (base.trim_end_matches(".py").to_string(), vllm);
        }
        if let Some(i) = args.iter().position(|a| a == "-m")
            && let Some(m) = args.get(i + 1) {
                return (m.clone(), vllm);
            }
    }
    (comm.to_string(), vllm)
}

fn rss_bytes(pid: i32) -> u64 {
    fs::read_to_string(format!("/proc/{pid}/status"))
        .ok()
        .and_then(|t| {
            t.lines()
                .find(|l| l.starts_with("VmRSS:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse::<u64>().ok())
        })
        .map(|kb| kb * 1024)
        .unwrap_or(0)
}

pub fn spawn(tx: Sender<crate::Msg>, interval: Duration) {
    std::thread::Builder::new()
        .name("host".into())
        .spawn(move || {
            if read_stat().is_none() {
                let _ = tx.send(crate::Msg::HostUnavailable);
                return;
            }
            let hz = getconf("CLK_TCK", 100) as f32;
            let hostname = fs::read_to_string("/proc/sys/kernel/hostname").map(|s| s.trim().to_string()).unwrap_or_default();
            let mut prev_stat = read_stat();
            // pid -> (ticks, starttime) from the previous process scan
            let mut prev_procs: HashMap<i32, (u64, u64)> = HashMap::new();
            let mut prev_scan = Instant::now();
            let mut names: HashMap<(i32, u64), (String, bool)> = HashMap::new();
            let mut top: Vec<ProcInfo> = vec![];
            let mut nprocs = 0usize;
            let mut tick = 0u64;
            loop {
                std::thread::sleep(interval);
                let mut s = HostSample { hostname: hostname.clone(), ..Default::default() };
                if let (Some((a, c)), Some((pa, pc))) = (read_stat(), prev_stat.as_ref()) {
                    if let (Some(&n), Some(&p)) = (a.first(), pa.first()) {
                        (s.cpu, s.iowait) = frac(n, p);
                    }
                    s.cores = c.iter().zip(pc).map(|(&n, &p)| frac(n, p).0).collect();
                    prev_stat = Some((a, c));
                }
                meminfo(&mut s);
                if let Ok(l) = fs::read_to_string("/proc/loadavg") {
                    for (i, v) in l.split_whitespace().take(3).enumerate() {
                        s.load[i] = v.parse().unwrap_or(0.0);
                    }
                }
                // Process scan every other tick: ~1000 small reads, a few ms.
                if tick.is_multiple_of(2) {
                    let dt = prev_scan.elapsed().as_secs_f32().max(0.1);
                    prev_scan = Instant::now();
                    let mut cur: HashMap<i32, (u64, u64)> = HashMap::with_capacity(prev_procs.len() + 16);
                    let mut busy: Vec<(i32, String, u64, f32)> = vec![];
                    if let Ok(rd) = fs::read_dir("/proc") {
                        for e in rd.flatten() {
                            let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<i32>().ok()) else { continue };
                            let Some((comm, ticks, start)) = proc_stat(pid) else { continue };
                            cur.insert(pid, (ticks, start));
                            if let Some(&(pt, ps)) = prev_procs.get(&pid)
                                && ps == start && ticks > pt {
                                    let pct = (ticks - pt) as f32 / hz / dt * 100.0;
                                    busy.push((pid, comm, start, pct));
                                }
                        }
                    }
                    nprocs = cur.len();
                    busy.sort_by(|a, b| b.3.total_cmp(&a.3));
                    names.retain(|(pid, st), _| cur.get(pid).is_some_and(|c| c.1 == *st));
                    top = busy
                        .into_iter()
                        .take(6)
                        .map(|(pid, comm, start, cpu)| {
                            let (name, vllm) = names.entry((pid, start)).or_insert_with(|| proc_name(pid, &comm)).clone();
                            ProcInfo { pid, name, cpu, rss: rss_bytes(pid), vllm }
                        })
                        .collect();
                    prev_procs = cur;
                }
                tick += 1;
                s.top = top.clone();
                s.nprocs = nprocs;
                if tx.send(crate::Msg::Host(Box::new(s), crate::state::now())).is_err() {
                    return;
                }
            }
        })
        .expect("spawn host thread");
}
