// Copyright 2026 Local Inference Lab, Inc.
// SPDX-License-Identifier: Apache-2.0

//! Local GPU telemetry through NVML (loaded at runtime; absent NVML just disables the panel).

use nvml_wrapper::Nvml;
use nvml_wrapper::enum_wrappers::device::{Clock, TemperatureSensor};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default)]
pub struct GpuSample {
    pub index: u32,
    pub name: String,
    pub util: Option<u32>,
    pub mem_util: Option<u32>,
    pub mem_used: Option<u64>,
    pub mem_total: Option<u64>,
    pub sm_clock: Option<u32>,
    pub sm_clock_max: Option<u32>,
    pub temp: Option<u32>,
    pub power_w: Option<f64>,
    pub power_limit_w: Option<f64>,
}

pub fn short_name(n: &str) -> String {
    let mut s = n.replace("NVIDIA ", "");
    for cut in [" Workstation Edition", " Server Edition", " Blackwell", " Generation"] {
        s = s.replace(cut, "");
    }
    s.trim().to_string()
}

fn open() -> Option<Nvml> {
    Nvml::init().ok().or_else(|| Nvml::builder().lib_path("libnvidia-ml.so.1".as_ref()).init().ok())
}

pub fn spawn(tx: Sender<crate::Msg>, interval: Duration) {
    std::thread::Builder::new()
        .name("nvml".into())
        .spawn(move || {
            let Some(nvml) = open() else {
                let _ = tx.send(crate::Msg::GpuUnavailable);
                return;
            };
            let count = nvml.device_count().unwrap_or(0);
            let mut names = Vec::new();
            let mut maxclk = Vec::new();
            for i in 0..count {
                let d = nvml.device_by_index(i).ok();
                names.push(d.as_ref().and_then(|d| d.name().ok()).map(|n| short_name(&n)).unwrap_or_else(|| format!("GPU{i}")));
                maxclk.push(d.as_ref().and_then(|d| d.max_clock_info(Clock::SM).ok()));
            }
            loop {
                let t0 = Instant::now();
                let mut out = Vec::with_capacity(count as usize);
                for i in 0..count {
                    let Ok(d) = nvml.device_by_index(i) else { continue };
                    let u = d.utilization_rates().ok();
                    let m = d.memory_info().ok();
                    // On Ampere and newer this is already the driver's 1 s average (nvidia-smi's
                    // power.draw). The energy counter would also work but costs ~4 ms of driver time
                    // per call per GPU, versus microseconds here.
                    let power_w = d.power_usage().ok().map(|mw| mw as f64 / 1000.0);
                    out.push(GpuSample {
                        index: i,
                        name: names[i as usize].clone(),
                        util: u.as_ref().map(|u| u.gpu),
                        mem_util: u.as_ref().map(|u| u.memory),
                        mem_used: m.as_ref().map(|m| m.used),
                        mem_total: m.as_ref().map(|m| m.total),
                        sm_clock: d.clock_info(Clock::SM).ok(),
                        sm_clock_max: maxclk[i as usize],
                        temp: d.temperature(TemperatureSensor::Gpu).ok(),
                        power_w,
                        power_limit_w: d.enforced_power_limit().ok().map(|mw| mw as f64 / 1000.0),
                    });
                }
                if tx.send(crate::Msg::Gpu(out, crate::state::now())).is_err() {
                    return;
                }
                std::thread::sleep(interval.saturating_sub(t0.elapsed()));
            }
        })
        .expect("spawn nvml thread");
}
