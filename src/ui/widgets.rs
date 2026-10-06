// Copyright 2026 Local Inference Lab, Inc.
// SPDX-License-Identifier: Apache-2.0

use super::theme::{self, Ramp};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

const LOWER: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
const LEFT: [char; 9] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];

// ---- number formatting --------------------------------------------------------------------------

/// Compact magnitude: 812, 9.4, 1.23K, 44.6K, 612K, 3.42M.
pub fn si(v: f64) -> String {
    let a = v.abs();
    if !v.is_finite() {
        "—".into()
    } else if a >= 1e9 {
        format!("{:.2}G", v / 1e9)
    } else if a >= 1e8 {
        format!("{:.0}M", v / 1e6)
    } else if a >= 1e7 {
        format!("{:.1}M", v / 1e6)
    } else if a >= 1e6 {
        format!("{:.2}M", v / 1e6)
    } else if a >= 1e5 {
        format!("{:.0}K", v / 1e3)
    } else if a >= 1e4 {
        format!("{:.1}K", v / 1e3)
    } else if a >= 1e3 {
        format!("{:.2}K", v / 1e3)
    } else if a >= 100.0 || a == 0.0 {
        format!("{v:.0}")
    } else if a >= 10.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.2}")
    }
}

/// `si` without trailing zeros, for axis labels: 2.00K -> 2K, 2.50K -> 2.5K, 10.0 -> 10.
pub fn si_axis(v: f64) -> String {
    let s = si(v);
    let split = s.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(s.len());
    let (num, suf) = s.split_at(split);
    let num = if num.contains('.') { num.trim_end_matches('0').trim_end_matches('.') } else { num };
    format!("{num}{suf}")
}

/// Like `si` but integers below 10 stay integers (counts).
pub fn count(v: f64) -> String {
    if v.abs() < 1000.0 { format!("{v:.0}") } else { si(v) }
}

pub fn secs(s: f64) -> String {
    if !s.is_finite() {
        "—".into()
    } else if s < 0.001 {
        format!("{:.0}µs", s * 1e6)
    } else if s < 0.01 {
        format!("{:.1}ms", s * 1e3)
    } else if s < 1.0 {
        format!("{:.0}ms", s * 1e3)
    } else if s < 10.0 {
        format!("{s:.2}s")
    } else if s < 60.0 {
        format!("{s:.1}s")
    } else if s < 3600.0 {
        format!("{}m{:02}s", (s / 60.0) as u64, (s % 60.0) as u64)
    } else if s < 86400.0 {
        format!("{}h{:02}m", (s / 3600.0) as u64, ((s % 3600.0) / 60.0) as u64)
    } else {
        format!("{}d{}h", (s / 86400.0) as u64, ((s % 86400.0) / 3600.0) as u64)
    }
}

/// Context lengths are usually powers of two: 1048576 -> 1M, 262144 -> 256K.
pub fn ctx(n: u64) -> String {
    if n >= 1 << 20 && n.is_multiple_of(1 << 20) {
        format!("{}M", n >> 20)
    } else if n >= 1 << 10 && n.is_multiple_of(1 << 10) {
        format!("{}K", n >> 10)
    } else {
        si(n as f64)
    }
}

pub fn ago(s: f64) -> String {
    if s < 60.0 {
        format!("{:.0}s", s.max(0.0))
    } else if s < 3600.0 {
        format!("{:.0}m", s / 60.0)
    } else if s < 86400.0 {
        format!("{:.0}h", s / 3600.0)
    } else {
        format!("{:.0}d", s / 86400.0)
    }
}

pub fn pct(f: f64) -> String {
    format!("{:.1}%", f * 100.0)
}

/// Local wall-clock HH:MM:SS for a unix time.
pub fn clock(t: f64) -> String {
    let off = local_offset();
    let s = (t as i64 + off).rem_euclid(86400);
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

fn local_offset() -> i64 {
    use std::sync::OnceLock;
    static OFF: OnceLock<i64> = OnceLock::new();
    *OFF.get_or_init(|| {
        std::process::Command::new("date")
            .arg("+%z")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| {
                let s = s.trim();
                let sign = if s.starts_with('-') { -1 } else { 1 };
                let d = s.trim_start_matches(['+', '-']);
                let h: i64 = d.get(0..2)?.parse().ok()?;
                let m: i64 = d.get(2..4)?.parse().ok()?;
                Some(sign * (h * 3600 + m * 60))
            })
            .unwrap_or(0)
    })
}

/// Round up to 1, 2, 2.5, 5 x 10^k.
pub fn nice_ceil(v: f64) -> f64 {
    if v <= 0.0 || !v.is_finite() {
        return 1.0;
    }
    let e = 10f64.powf(v.log10().floor());
    for m in [1.0, 2.0, 2.5, 5.0, 10.0] {
        if v <= m * e * 1.0000001 {
            return m * e;
        }
    }
    10.0 * e
}

// ---- big digits ---------------------------------------------------------------------------------

fn glyph(c: char) -> [&'static str; 3] {
    match c {
        '0' => ["█▀█", "█ █", "▀▀▀"],
        '1' => ["▀█ ", " █ ", "▀▀▀"],
        '2' => ["▀▀█", "█▀▀", "▀▀▀"],
        '3' => ["▀▀█", " ▀█", "▀▀▀"],
        '4' => ["█ █", "▀▀█", "  ▀"],
        '5' => ["█▀▀", "▀▀█", "▀▀▀"],
        '6' => ["█▀▀", "█▀█", "▀▀▀"],
        '7' => ["▀▀█", "  █", "  ▀"],
        '8' => ["█▀█", "█▀█", "▀▀▀"],
        '9' => ["█▀█", "▀▀█", "▀▀▀"],
        '.' => [" ", " ", "▀"],
        '-' | '—' => ["   ", "▀▀▀", "   "],
        _ => [" ", " ", " "],
    }
}

/// Draw `s` in 3-row digits at (x, y). Returns the width used.
pub fn big(buf: &mut Buffer, x: u16, y: u16, s: &str, style: Style, max_x: u16) -> u16 {
    let mut cx = x;
    for c in s.chars() {
        let g = glyph(c);
        let w = g[0].chars().count() as u16;
        if cx + w > max_x {
            break;
        }
        for (r, row) in g.iter().enumerate() {
            buf.set_string(cx, y + r as u16, row, style);
        }
        cx += w + 1;
    }
    cx.saturating_sub(x + 1)
}

/// Split a number into big-font digits and a small suffix: 44612 -> ("44.6", "K").
pub fn big_parts(v: f64) -> (String, String) {
    let s = si(v);
    let split = s.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(s.len());
    (s[..split].to_string(), s[split..].to_string())
}

// ---- area chart ---------------------------------------------------------------------------------

#[derive(Clone, Copy, Default)]
pub struct Col {
    pub mean: f32,
    pub max: f32,
    pub seen: bool,
}

/// Filled area chart with a vertical gradient. When a column aggregates several seconds, the
/// span between its mean and max is drawn in the ramp's dim band tone.
pub fn area(buf: &mut Buffer, r: Rect, cols: &[Col], ymax: f64, ramp: &Ramp) {
    if r.width == 0 || r.height == 0 {
        return;
    }
    let h = r.height as f64;
    let eighths = |v: f32| -> i64 {
        if v <= 0.0 {
            0
        } else {
            ((v as f64 / ymax) * h * 8.0).round().max(1.0) as i64
        }
    };
    for (x, c) in cols.iter().enumerate().take(r.width as usize) {
        let cx = r.x + x as u16;
        if !c.seen {
            continue;
        }
        let hm = eighths(c.mean);
        let hx = eighths(c.max).max(hm);
        for row in 0..r.height {
            let lo = row as i64 * 8;
            let hi = lo + 8;
            let cy = r.y + r.height - 1 - row;
            let fg = ramp.at((row as f64 + 0.5) / h);
            let cell = &mut buf[(cx, cy)];
            if hm >= hi {
                cell.set_char('█').set_fg(fg);
            } else if hm > lo {
                cell.set_char(LOWER[(hm - lo) as usize]).set_fg(fg);
                if hx >= lo + 6 {
                    cell.set_bg(ramp.band);
                }
            } else if hx > lo {
                let n = (hx - lo).min(8) as usize;
                cell.set_char(LOWER[n]).set_fg(ramp.band);
            }
        }
    }
}

/// Dotted horizontal guide line (drawn before the area so bars cover it).
pub fn guide(buf: &mut Buffer, x: u16, y: u16, w: u16) {
    for i in 0..w {
        if i % 2 == 0 {
            buf[(x + i, y)].set_char('┈').set_fg(theme::th().grid);
        }
    }
}

// ---- small pieces -------------------------------------------------------------------------------

/// One-row sparkline of `vals` scaled to `ymax`, newest at the right.
pub fn spark(buf: &mut Buffer, x: u16, y: u16, w: u16, vals: &[Option<f32>], ymax: f32, ramp: &Ramp) {
    let n = vals.len();
    for i in 0..w as usize {
        if i + n < w as usize {
            continue;
        }
        let Some(v) = vals[i + n - w as usize] else { continue };
        let f = if ymax > 0.0 { (v / ymax).clamp(0.0, 1.0) } else { 0.0 };
        let k = if v > 0.0 { ((f * 8.0).round() as usize).max(1) } else { 0 };
        if k == 0 {
            buf[(x + i as u16, y)].set_char('▁').set_fg(theme::th().grid);
        } else {
            buf[(x + i as u16, y)].set_char(LOWER[k]).set_fg(ramp.at(f as f64));
        }
    }
}

/// Horizontal bar with 1/8-cell resolution on a dim track.
pub fn hbar(buf: &mut Buffer, x: u16, y: u16, w: u16, frac: f64, fg: Color) {
    let f = frac.clamp(0.0, 1.0);
    let total = (f * w as f64 * 8.0).round() as u32;
    for i in 0..w {
        let filled = total.saturating_sub(i as u32 * 8).min(8) as usize;
        let cell = &mut buf[(x + i, y)];
        cell.set_bg(theme::th().track);
        if filled > 0 {
            cell.set_char(LEFT[filled]).set_fg(fg);
        } else {
            cell.set_char(' ');
        }
    }
}
