// Copyright 2026 Local Inference Lab, Inc.
// SPDX-License-Identifier: Apache-2.0

pub mod theme;
pub mod widgets;

use crate::prom::H;
use crate::state::{Field, now};
use crate::{App, Endpoint, Overlay, SETTINGS, Setting, WINDOWS};
use std::sync::atomic::Ordering;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Wrap};
use theme::*;
use widgets::*;

fn put(buf: &mut Buffer, x: u16, y: u16, w: u16, spans: Vec<Span<'_>>) -> u16 {
    if w == 0 {
        return x;
    }
    buf.set_line(x, y, &Line::from(spans), w).0
}

fn panel(f: &mut Frame, r: Rect, title: &str, sub: &str, key: Color, right: Vec<Span<'_>>) -> Rect {
    let mut left = vec![Span::styled("▍", Style::new().fg(key)), Span::styled(title.to_string(), theme::title())];
    if !sub.is_empty() {
        left.push(Span::styled(format!(" {sub}"), theme::muted()));
    }
    left.push(Span::raw(" "));
    let mut b = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(th().border))
        .title_top(Line::from(left));
    if !right.is_empty() {
        let mut rs = vec![Span::raw(" ")];
        rs.extend(right);
        rs.push(Span::raw(" "));
        b = b.title_top(Line::from(rs).right_aligned());
    }
    let inner = b.inner(r);
    f.render_widget(b, r);
    inner
}

/// Clears an overlay's area to the configured background (or the terminal default).
fn clear(f: &mut Frame, r: Rect, app: &App) {
    f.render_widget(Clear, r);
    if let Some(bg) = app.bg.color() {
        f.buffer_mut().set_style(r, Style::new().bg(bg).fg(th().text));
    }
}

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    // Paint every cell so transparent terminals don't show patches where cells set their own bg.
    if let Some(bg) = app.bg.color() {
        f.buffer_mut().set_style(area, Style::new().bg(bg).fg(th().text));
    }
    if area.width < 80 || area.height < 24 {
        let p = Paragraph::new(format!("lilmon needs at least 80×24 (now {}×{})", area.width, area.height))
            .style(theme::text2());
        f.render_widget(p, area);
        return;
    }
    let [header, body, footer] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0), Constraint::Length(1)]).areas(area);
    draw_header(f, header, app);
    draw_footer(f, footer, app);

    let wide = body.width >= 150;
    let gpu_on = app.show_gpu && app.gpu.enabled && (!app.gpu.samples.is_empty() || !app.gpu.unavailable);
    let host_on = app.show_host && app.host.enabled && !app.host.unavailable;
    let req_on = app.show_req && app.litellm_on;
    let ngpu = app.gpu.samples.len().max(1) as u16;
    let mid_h: u16 = 13;
    let sys_on = gpu_on || host_on;
    let want_bottom = if wide {
        match (req_on, gpu_on, host_on) {
            (true, true, true) => (body.height * 36 / 100).clamp(12, 24),
            (true, _, _) => (body.height * 30 / 100).clamp(8, 16),
            (false, _, true) => (body.height * 30 / 100).clamp(9, 16),
            (false, true, false) => 2 + 2 * ngpu,
            _ => 0,
        }
    } else {
        12
    };
    let mut bottom_h = want_bottom;
    if body.height < mid_h + bottom_h + 10 {
        bottom_h = body.height.saturating_sub(mid_h + 10);
        if bottom_h < 6 {
            bottom_h = 0;
        }
    }
    let [charts, mid, bottom] =
        Layout::vertical([Constraint::Min(8), Constraint::Length(mid_h), Constraint::Length(bottom_h)]).areas(body);

    let [c1, c2] = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(charts);
    throughput(f, c1, app, Field::Decode);
    throughput(f, c2, app, Field::Prefill);

    if wide {
        let [a, b, c] =
            Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(36), Constraint::Percentage(34)])
                .areas(mid);
        sessions(f, a, app);
        mtp(f, b, app);
        latency(f, c, app);
        if bottom_h > 0 {
            if req_on && sys_on {
                let [l, r] = Layout::horizontal([Constraint::Percentage(44), Constraint::Percentage(56)]).areas(bottom);
                system_stack(f, l, app, gpu_on, host_on, ngpu);
                requests(f, r, app);
            } else if req_on {
                requests(f, bottom, app);
            } else if gpu_on && host_on {
                let [l, r] = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(bottom);
                gpu(f, l, app);
                host(f, r, app);
            } else if gpu_on {
                gpu(f, bottom, app);
            } else if host_on {
                host(f, bottom, app);
            }
        }
    } else {
        let [a, b] = Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(mid);
        sessions(f, a, app);
        mtp(f, b, app);
        if bottom_h > 0 {
            let [l, r] = Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(bottom);
            latency(f, l, app);
            if req_on {
                requests(f, r, app);
            } else if sys_on {
                system_stack(f, r, app, gpu_on, host_on, ngpu);
            }
        }
    }

    match app.overlay {
        Overlay::Help => help(f, area, app),
        Overlay::Records => records(f, area, app),
        Overlay::Settings => settings(f, area, app),
        Overlay::None => {}
    }
}

fn status(ep: &Endpoint, interval_s: f64) -> (String, Color) {
    let t = now();
    let age = ep.last_ok.map(|l| t - l);
    match (&ep.err, age) {
        (Some((e, _)), a) if a.is_none_or(|a| a > 3.0 * interval_s) => {
            // auth problems read better without the "down" prefix and need room for the hint
            if e.starts_with("401 ") || e.starts_with("403 ") {
                (format!("✕ {}", e.chars().take(72).collect::<String>()), th().crit)
            } else {
                (format!("✕ down · {}", e.chars().take(48).collect::<String>()), th().crit)
            }
        }
        (_, Some(a)) if a > 3.0 * interval_s => (format!("◌ stale {}", ago(a)), th().warn),
        (_, Some(_)) => (format!("● live {:.0}ms", ep.scrape_ms), th().good),
        _ => ("◌ connecting".into(), th().muted),
    }
}

fn draw_header(f: &mut Frame, r: Rect, app: &App) {
    let buf = f.buffer_mut();
    let ep = app.ep();
    let mut x = r.x;
    let w = r.width;
    // "lilmon [L|L]": the Local Inference Lab mark, brackets muted, the bar in the accent color.
    let bracket = theme::muted();
    x = put(buf, x, r.y, w, vec![
        Span::styled(" ▍", Style::new().fg(th().decode.key)),
        Span::styled("lilmon ", theme::bold()),
        Span::styled("[", bracket),
        Span::styled("L", theme::bold()),
        Span::styled("|", Style::new().fg(th().decode.key).add_modifier(Modifier::BOLD)),
        Span::styled("L", theme::bold()),
        Span::styled("] ", bracket),
    ]);
    for (i, e) in app.eps.iter().enumerate() {
        let label = format!(" {} {} ", i + 1, e.cfg.name);
        let st = if i == app.sel {
            Style::new().fg(th().text).bg(th().tab_bg).add_modifier(Modifier::BOLD)
        } else {
            theme::muted()
        };
        x = put(buf, x, r.y, r.right().saturating_sub(x), vec![Span::styled(label, st)]);
        x += 1;
    }
    let interval_s = app.cfg.interval_ms as f64 / 1000.0;
    let (stxt, scol) = status(ep, interval_s);
    let mut spans = vec![Span::styled(" ", theme::text()), Span::styled(ep.model(), theme::bold())];
    let last = ep.st.last();
    if let Some(ml) = ep.info.as_ref().and_then(|i| i.max_model_len) {
        spans.push(Span::styled(format!(" · {} ctx", ctx(ml)), theme::text2()));
    }
    if let Some(s) = last {
        if let Some(d) = s.cache_info.get("cache_dtype") {
            spans.push(Span::styled(format!(" · {d} KV"), theme::text2()));
        }
        if let Some(c) = s.kv_capacity_tokens() {
            spans.push(Span::styled(format!(" · {} tok", si(c)), theme::text2()));
        }
        if let Some(ps) = s.process_start {
            spans.push(Span::styled(format!(" · up {}", secs(now() - ps)), theme::muted()));
        }
        if s.asleep {
            spans.push(Span::styled("  ☾ sleeping", Style::new().fg(th().warn)));
        }
    }
    if ep.st.restarts > 0 {
        spans.push(Span::styled(format!("  ↻ restarted ×{}", ep.st.restarts), Style::new().fg(th().warn)));
    }
    spans.push(Span::styled(format!("   {stxt}"), Style::new().fg(scol)));

    let (_, wl) = app.window();
    let right = vec![
        Span::styled(if app.paused { "⏸ paused  " } else { "" }, Style::new().fg(th().warn).add_modifier(Modifier::BOLD)),
        Span::styled("window ", theme::muted()),
        Span::styled(wl, theme::bold()),
        Span::styled(format!("  {} ", clock(now())), theme::text2()),
    ];
    let rw: u16 = right.iter().map(|s| s.width() as u16).sum();
    let left_w: u16 = spans.iter().map(|s| s.width() as u16).sum();
    if x + left_w + rw < r.right() {
        put(buf, x, r.y, r.right().saturating_sub(x), spans);
        put(buf, r.right() - rw, r.y, rw, right);
    } else {
        put(buf, x, r.y, r.right().saturating_sub(x), spans);
    }
}

fn draw_footer(f: &mut Frame, r: Rect, app: &App) {
    let buf = f.buffer_mut();
    let k = |s: &'static str| Span::styled(s, Style::new().fg(th().text).bg(th().tab_bg));
    let d = |s: &'static str| Span::styled(s, theme::muted());
    let mut spans = vec![Span::raw(" "), k(" q "), d(" quit  "), k(" ←→ "), d(" window  ")];
    if app.eps.len() > 1 {
        spans.extend([k(" tab "), d(" endpoint  ")]);
    }
    spans.extend([k(" s "), d(" settings  "), k(" t "), d(" theme  "), k(" r "), d(" records  ")]);
    spans.extend([k(" g "), d(" gpu  "), k(" c "), d(" cpu/mem  ")]);
    if app.litellm_on {
        spans.extend([k(" l "), d(" requests  ")]);
    }
    spans.extend([k(" ? "), d(" help")]);
    let end = put(buf, r.x, r.y, r.width, spans);
    if let Some((msg, _)) = &app.toast {
        let avail = r.right().saturating_sub(end + 3) as usize;
        if avail > 12 {
            let txt: String = if msg.chars().count() > avail { msg.chars().take(avail - 1).chain(['…']).collect() } else { msg.clone() };
            let w = txt.chars().count() as u16 + 2;
            put(buf, r.right() - w, r.y, w, vec![Span::styled(format!(" {txt} "), Style::new().fg(th().bg).bg(th().decode.key))]);
        }
    }
}

// ---- throughput charts --------------------------------------------------------------------------

/// Maps whole seconds to chart columns. Bins hold a whole number of seconds and are anchored to
/// absolute time (bin = sec / spc), and each bin spans a whole number of columns, so as time advances
/// the data scrolls left by whole bins instead of being re-binned (which made shapes shimmer).
/// The drawn span is therefore the window rounded to fit the width, not exactly the window.
pub(crate) struct Timeline {
    spc: i64,
    cpb: u16,
    nbins: usize,
    width: u16,
    last_bin: i64,
    end_sec: i64,
}

impl Timeline {
    pub(crate) fn new(win: i64, width: u16, end_sec: i64) -> Timeline {
        let w = width.max(1) as i64;
        let (spc, cpb) = if win >= w {
            (((win as f64 / w as f64).round() as i64).max(1), 1)
        } else {
            (1, ((w as f64 / win.max(1) as f64).round() as i64).clamp(1, w))
        };
        Timeline { spc, cpb: cpb as u16, nbins: (w / cpb) as usize, width: width.max(1), last_bin: end_sec.div_euclid(spc), end_sec }
    }

    fn span(&self) -> i64 {
        self.nbins as i64 * self.spc
    }

    /// Seconds [s0, s1) of bin `i` (0 = oldest shown); the newest bin is cut at `end_sec`.
    fn bin_secs(&self, i: usize) -> (i64, i64) {
        let s0 = (self.last_bin - (self.nbins - 1 - i) as i64) * self.spc;
        (s0, (s0 + self.spc).min(self.end_sec + 1))
    }

    /// Unused columns sit at the left edge.
    fn x_offset(&self) -> u16 {
        self.width - self.nbins as u16 * self.cpb
    }

    pub(crate) fn columns<T: Copy + Default>(&self, mut f: impl FnMut(i64, i64) -> T) -> Vec<T> {
        let mut out = vec![T::default(); self.x_offset() as usize];
        for i in 0..self.nbins {
            let (a, b) = self.bin_secs(i);
            let v = f(a, b);
            out.extend(std::iter::repeat_n(v, self.cpb as usize));
        }
        out
    }

    /// Column (from the plot's left edge) of the time `ago` seconds before the leading edge.
    fn col_for_ago(&self, ago: i64) -> Option<u16> {
        let bins_back = ago / self.spc;
        if bins_back as usize >= self.nbins {
            return None;
        }
        let right = self.width as i64 - 1;
        Some((right - bins_back * self.cpb as i64).max(0) as u16)
    }
}

/// Mean and max of a bucket field per bin.
pub(crate) fn series_columns(st: &crate::state::EndpointState, field: Field, tl: &Timeline) -> Vec<Col> {
    tl.columns(|a, b| {
        let mut c = Col::default();
        let (mut sum, mut n) = (0.0f64, 0u32);
        for s in a..b {
            if let Some(bk) = st.series.get(s).filter(|b| b.seen) {
                let v = bk.get(field);
                sum += v as f64;
                n += 1;
                c.max = c.max.max(v);
            }
        }
        if n > 0 {
            c.seen = true;
            c.mean = (sum / n as f64) as f32;
        }
        c
    })
}

fn tick_step(win: i64) -> i64 {
    [(60, 15), (300, 60), (900, 180), (3600, 900), (21600, 3600)]
        .iter()
        .find(|&&(w, _)| win <= w)
        .map_or(6 * 3600, |&(_, step)| step)
}

fn rel_label(s: i64) -> String {
    if s == 0 {
        "now".into()
    } else if s < 60 {
        format!("-{s}s")
    } else if s < 3600 {
        format!("-{}m", s / 60)
    } else {
        format!("-{}h", s / 3600)
    }
}

fn chart(buf: &mut Buffer, r: Rect, app: &App, field: Field, ramp: &Ramp, min_ymax: f64) {
    let (win, _) = app.window();
    let gutter = 6u16;
    if r.width <= gutter + 4 || r.height < 3 {
        return;
    }
    let plot = Rect::new(r.x + gutter, r.y, r.width - gutter - 1, r.height - 1);
    let tl = Timeline::new(win, plot.width, app.ep().st.complete_sec());
    let cols = series_columns(&app.ep().st, field, &tl);
    let peak = cols.iter().map(|c| c.max).fold(0.0f32, f32::max);
    let ymax = nice_ceil((peak as f64 * 1.08).max(min_ymax));

    // axes and guides
    let label_style = theme::faint();
    buf.set_stringn(r.x, plot.y, format!("{:>5}", si_axis(ymax)), 5, label_style);
    guide(buf, plot.x, plot.y, plot.width);
    if plot.height >= 4 {
        // The row containing ymax/2 (its top edge is exactly ymax/2 when the height is even).
        let my = plot.y + plot.height / 2;
        buf.set_stringn(r.x, my, format!("{:>5}", si_axis(ymax / 2.0)), 5, label_style);
        guide(buf, plot.x, my, plot.width);
    }
    buf.set_stringn(r.x, plot.bottom() - 1, format!("{:>5}", "0"), 5, label_style);
    area(buf, plot, &cols, ymax, ramp);

    // baseline + time ticks
    let ly = plot.bottom();
    for i in 0..plot.width {
        buf[(plot.x + i, ly)].set_char('─').set_fg(th().grid);
    }
    let span = tl.span();
    let step = tick_step(span);
    let mut last_end = plot.x;
    let mut k = span / step;
    while k >= 0 {
        let ago_s = k * step;
        k -= 1;
        let Some(col) = tl.col_for_ago(ago_s) else { continue };
        let cx = plot.x + col;
        let lab = rel_label(ago_s);
        let lw = lab.chars().count() as u16;
        let lx = if ago_s == 0 { plot.right().saturating_sub(lw) } else { cx.saturating_sub(lw / 2).max(plot.x) };
        if lx >= last_end && lx + lw <= plot.right() {
            buf[(cx.min(plot.right() - 1), ly)].set_char('┴').set_fg(th().grid);
            buf.set_string(lx, ly, &lab, theme::faint());
            last_end = lx + lw + 1;
        }
    }

    // label the window's peak column
    if peak > 0.0
        && let Some((ix, _)) = cols.iter().enumerate().max_by(|a, b| a.1.max.total_cmp(&b.1.max)) {
            let top_rows = ((peak as f64 / ymax) * plot.height as f64).ceil() as u16;
            let lab = si(peak as f64);
            let lw = lab.len() as u16;
            let y = (plot.bottom() - top_rows.min(plot.height)).saturating_sub(1).max(plot.y);
            let x = (plot.x + ix as u16).saturating_sub(lw / 2).clamp(plot.x, plot.right().saturating_sub(lw));
            if top_rows < plot.height {
                buf.set_string(x, y, &lab, theme::text2());
            }
        }
}

fn throughput(f: &mut Frame, r: Rect, app: &App, field: Field) {
    let ep = app.ep();
    let st = &ep.st;
    let (win, wl) = app.window();
    let decode = field == Field::Decode;
    let ramp = if decode { &th().decode } else { &th().prefill };
    let title = if decode { "DECODE" } else { "PREFILL" };
    let sub = if decode { "tok/s" } else { "tok/s computed" };
    let rec = app.records.get(&ep.record_key());
    let right = match rec.map(|r| if decode { r.decode } else { r.prefill }) {
        Some(rv) if rv.v > 0.0 => vec![Span::styled("record ", theme::muted()), Span::styled(si(rv.v), theme::text2())],
        _ => vec![],
    };
    let inner = panel(f, r, title, sub, ramp.key, right);
    let buf = f.buffer_mut();
    if inner.height < 4 {
        return;
    }
    let now_s = if decode { app.cfg.ui.decode_now_s } else { app.cfg.ui.prefill_now_s }.max(1) as i64;
    let nowv = st.mean(field, now_s);
    let x0 = inner.x + 1;
    let mut top = inner.y;
    let big_mode = inner.height >= 10;
    
    let stat_x = if big_mode {
        let (digits, suffix) = match nowv {
            Some(v) => big_parts(v as f64),
            None => ("—".to_string(), String::new()),
        };
        let bw = big(buf, x0, top, &digits, Style::new().fg(th().text), inner.right());
        let ux = x0 + bw + 1;
        let ux = put(buf, ux, top + 2, inner.right().saturating_sub(ux), vec![Span::styled(suffix, theme::bold())]);
        put(buf, ux + 1, top + 2, inner.right().saturating_sub(ux + 1), vec![Span::styled("tok/s", theme::muted())]);
        // Fixed column so the stats don't shift as the big number changes width.
        x0 + 24
    } else {
        let v = nowv.map(|v| si(v as f64)).unwrap_or_else(|| "—".into());
        put(buf, x0, top, inner.width, vec![Span::styled(v, theme::bold()), Span::styled(" tok/s", theme::muted())]);
        x0 + 16
    };

    // peaks column
    let colw = 17u16;
    let lines_a: Vec<(String, String)> = app
        .peaks
        .iter()
        .map(|(s, l)| {
            let v = st.peak(field, *s).map(|(v, _)| si(v as f64)).unwrap_or_else(|| "—".into());
            (format!("peak {l:<4}"), v)
        })
        .collect();
    let lat = st.latency_window((win as f64).min(3600.0));
    let mut lines_b: Vec<Vec<Span>> = vec![];
    let avg = st.mean(field, win).map(|v| si(v as f64)).unwrap_or_else(|| "—".into());
    lines_b.push(vec![Span::styled(format!("avg {wl:<5}"), theme::muted()), Span::styled(avg, theme::text())]);
    if decode {
        let pr = lat.per_req_decode.map(si).unwrap_or_else(|| "—".into());
        lines_b.push(vec![
            Span::styled("per-req   ", theme::muted()),
            Span::styled(pr, theme::text()),
            Span::styled(" tok/s", theme::muted()),
        ]);
        let s = st.last();
        let run = s.map(|s| s.running).unwrap_or(0.0);
        let per = st.mean(Field::Decode, 2).filter(|_| run > 0.0).map(|v| v as f64 / run);
        lines_b.push(vec![
            Span::styled("per-seq   ", theme::muted()),
            Span::styled(per.map(si).unwrap_or_else(|| "—".into()), theme::text()),
            Span::styled(format!(" tok/s × {run:.0}"), theme::muted()),
        ]);
    } else {
        let w = st.sum_window(win);
        let cached = w.cached as f64 / win as f64;
        let hit = (w.prefix_q > 0.0).then(|| w.prefix_h as f64 / w.prefix_q as f64);
        lines_b.push(vec![
            Span::styled("cached    ", theme::muted()),
            Span::styled(si(cached), theme::text()),
            Span::styled(hit.map(|h| format!(" · {} hit", pct(h))).unwrap_or_default(), theme::muted()),
        ]);
        if st.prefilling > 0.0 {
            let el = st.prefill_since.map(|t| now() - t).unwrap_or(0.0);
            lines_b.push(vec![
                Span::styled("▶ ", Style::new().fg(th().prefill.key)),
                Span::styled(format!("prefilling {:.0}", st.prefilling), theme::bold()),
                Span::styled(format!(" · {}", secs(el)), theme::muted()),
            ]);
        } else if let Some(e) = st.prefill_events.iter().rev().find(|e| e.tokens >= 8192.0).or(st.prefill_events.back()) {
            lines_b.push(vec![
                Span::styled("last      ", theme::muted()),
                Span::styled(si(e.tokens), theme::text()),
                Span::styled(format!(" in {} · ", secs(e.secs)), theme::muted()),
                Span::styled(si(e.tokens / e.secs), theme::text()),
                Span::styled(format!("/s · {} ago", ago(now() - e.t)), theme::muted()),
            ]);
        } else {
            lines_b.push(vec![Span::styled("last      —", theme::muted())]);
        }
    }
    if big_mode {
        for (i, (l, v)) in lines_a.iter().enumerate() {
            put(buf, stat_x, top + i as u16, colw, vec![Span::styled(l.clone(), theme::muted()), Span::styled(v.clone(), theme::text())]);
        }
        let bx = stat_x + colw + 2;
        if bx + 12 < inner.right() {
            for (i, l) in lines_b.into_iter().enumerate() {
                put(buf, bx, top + i as u16, inner.right() - bx, l);
            }
        }
        top += 4;
    } else {
        let mut x = stat_x;
        for (l, v) in &lines_a {
            x = put(buf, x, top, inner.right().saturating_sub(x), vec![Span::styled(l.clone(), theme::muted()), Span::styled(v.clone(), theme::text())]);
            x += 2;
        }
        top += 1;
    }
    let cr = Rect::new(inner.x, top, inner.width, inner.bottom().saturating_sub(top));
    chart(buf, cr, app, field, ramp, if decode { 10.0 } else { 1000.0 });
}

// ---- sessions -----------------------------------------------------------------------------------

fn sessions(f: &mut Frame, r: Rect, app: &App) {
    let ep = app.ep();
    let st = &ep.st;
    let (win, wl) = app.window();
    let inner = panel(f, r, "SESSIONS", "", th().kv.key, vec![]);
    let buf = f.buffer_mut();
    let Some(s) = st.last() else {
        put(buf, inner.x + 1, inner.y, inner.width, vec![Span::styled("waiting for data…", theme::muted())]);
        return;
    };
    let x0 = inner.x + 1;
    let w = inner.width.saturating_sub(2);
    let mut y = inner.y;
    let bw = big(buf, x0, y, &format!("{:.0}", s.running), Style::new().fg(th().text), inner.right());
    put(buf, x0 + bw + 1, y + 2, 10, vec![Span::styled("running", theme::muted())]);
    let sx = x0 + bw + 10;
    let sw = inner.right().saturating_sub(sx);
    let waiting_col = if s.waiting > 0.0 { th().warn } else { th().text };
    let mut wl_spans = vec![Span::styled("waiting  ", theme::muted()), Span::styled(format!("{:.0}", s.waiting), Style::new().fg(waiting_col))];
    if s.waiting_deferred > 0.0 {
        wl_spans.push(Span::styled(format!(" ({:.0} deferred)", s.waiting_deferred), theme::muted()));
    }
    put(buf, sx, y, sw, wl_spans);
    if st.prefilling > 0.0 {
        let el = st.prefill_since.map(|t| now() - t).unwrap_or(0.0);
        put(buf, sx, y + 1, sw, vec![
            Span::styled("prefill  ", theme::muted()),
            Span::styled(format!("{:.0}", st.prefilling), Style::new().fg(th().prefill.key).add_modifier(Modifier::BOLD)),
            Span::styled(format!(" ▶ {}", secs(el)), theme::muted()),
        ]);
    } else {
        put(buf, sx, y + 1, sw, vec![Span::styled("prefill  ", theme::muted()), Span::styled("0", theme::text())]);
    }
    put(buf, sx, y + 2, sw, vec![Span::styled("decode   ", theme::muted()), Span::styled(format!("{:.0}", st.decoding), theme::text())]);
    y += 4;

    // KV cache
    let kv = s.kv_usage;
    let barw = w.saturating_sub(11);
    put(buf, x0, y, 3, vec![Span::styled("KV", theme::muted())]);
    hbar(buf, x0 + 3, y, barw, kv, level(kv));
    put(buf, x0 + 4 + barw, y, 8, vec![Span::styled(format!("{:>5.1}%", kv * 100.0), theme::bold())]);
    y += 1;
    let mut cap = vec![];
    if let Some(c) = s.kv_capacity_tokens() {
        cap.push(Span::styled(format!("   of {} tok", si(c)), theme::muted()));
    }
    if let Some((pk, _)) = st.peak(Field::Kv, win) {
        cap.push(Span::styled(format!(" · peak {wl} {:.1}%", pk * 100.0), theme::muted()));
    }
    put(buf, x0, y, w, cap);
    y += 2;

    let sw_ = st.sum_window(win);
    let hit_w = (sw_.prefix_q > 0.0).then(|| sw_.prefix_h as f64 / sw_.prefix_q as f64);
    let hit_l = (s.prefix_queries > 0.0).then(|| s.prefix_hits / s.prefix_queries);
    put(buf, x0, y, w, vec![
        Span::styled("prefix hit ", theme::muted()),
        Span::styled(hit_w.map(pct).unwrap_or_else(|| "—".into()), theme::text()),
        Span::styled(format!(" {wl} · "), theme::muted()),
        Span::styled(hit_l.map(pct).unwrap_or_else(|| "—".into()), theme::text()),
        Span::styled(" life", theme::muted()),
    ]);
    y += 1;
    let per_min = sw_.finished as f64 / (win as f64 / 60.0);
    put(buf, x0, y, w, vec![
        Span::styled("finished   ", theme::muted()),
        Span::styled(count(sw_.finished as f64), theme::text()),
        Span::styled(format!(" in {wl} · "), theme::muted()),
        Span::styled(si(per_min), theme::text()),
        Span::styled("/min", theme::muted()),
    ]);
    y += 1;
    let mut reasons = vec![Span::styled("           ", theme::muted())];
    let mut fin: Vec<_> = s.finished.iter().collect();
    fin.sort_by_key(|(k, _)| (k.as_str() != "stop", k.as_str()));
    for (k, v) in fin {
        if *v == 0.0 && k != "stop" {
            continue;
        }
        let col = match k.as_str() {
            "error" => th().crit,
            "abort" => th().serious,
            "length" | "repetition" => th().warn,
            _ => th().text2,
        };
        reasons.push(Span::styled(format!("{k} "), theme::muted()));
        reasons.push(Span::styled(format!("{} ", count(*v)), Style::new().fg(col)));
    }
    if s.preemptions > 0.0 {
        reasons.push(Span::styled("preempt ", theme::muted()));
        reasons.push(Span::styled(count(s.preemptions), Style::new().fg(th().warn)));
    }
    put(buf, x0, y, w, reasons);
    y += 1;
    if y < inner.bottom() {
        let sw = w.saturating_sub(11);
        let tl = Timeline::new(win, sw, st.complete_sec());
        let cols = series_columns(&app.ep().st, Field::Running, &tl);
        let mx = cols.iter().map(|c| c.max).fold(1.0f32, f32::max);
        let vals: Vec<Option<f32>> = cols.iter().map(|c| c.seen.then_some(c.max)).collect();
        put(buf, x0, y, 11, vec![Span::styled("concurrency", theme::muted())]);
        spark(buf, x0 + 11, y, sw, &vals, mx, &th().kv);
    }
}

// ---- MTP / speculative decoding -----------------------------------------------------------------

fn mtp(f: &mut Frame, r: Rect, app: &App) {
    let ep = app.ep();
    let st = &ep.st;
    let (win, wl) = app.window();
    let Some(s) = st.last() else {
        panel(f, r, "MTP", "spec decode", th().mtp.key, vec![]);
        return;
    };
    let mut sp = st.spec_window(win);
    let mut scope = wl.to_string();
    if sp.drafts < 1.0 {
        sp = st.spec_lifetime();
        scope = "lifetime".into();
    }
    let inner = panel(f, r, "MTP", "spec decode", th().mtp.key, vec![Span::styled(scope.clone(), theme::muted())]);
    let buf = f.buffer_mut();
    if !s.has_spec {
        put(buf, inner.x + 1, inner.y, inner.width, vec![Span::styled("speculative decoding is not enabled", theme::muted())]);
        return;
    }
    let x0 = inner.x + 1;
    let w = inner.width.saturating_sub(2);
    let mut y = inner.y;
    let al = sp.accept_len().map(|v| format!("{v:.2}")).unwrap_or_else(|| "—".into());
    let bw = big(buf, x0, y, &al, Style::new().fg(th().text), inner.right());
    put(buf, x0 + bw + 1, y + 2, 12, vec![Span::styled("tok/step", theme::muted())]);
    let sx = x0 + bw + 11;
    let sw = inner.right().saturating_sub(sx);
    let secs_w = if scope == "lifetime" { 0.0 } else { win as f64 };
    let rate = |n: f64| if secs_w > 0.0 { format!("{}/s", si(n / secs_w)) } else { count(n) };
    put(buf, sx, y, sw, vec![
        Span::styled("accept  ", theme::muted()),
        Span::styled(sp.accept_rate().map(pct).unwrap_or_else(|| "—".into()), theme::bold()),
        Span::styled(" of drafted", theme::muted()),
    ]);
    put(buf, sx, y + 1, sw, vec![
        Span::styled("mean k  ", theme::muted()),
        Span::styled(sp.mean_k().map(|k| format!("{k:.2}")).unwrap_or_else(|| "—".into()), theme::text()),
        Span::styled(format!(" · {} drafts", rate(sp.drafts)), theme::muted()),
    ]);
    put(buf, sx, y + 2, sw, vec![
        Span::styled("tokens  ", theme::muted()),
        Span::styled(rate(sp.accepted), theme::text()),
        Span::styled(" acc / ", theme::muted()),
        Span::styled(rate(sp.draft_tokens), theme::text()),
        Span::styled(" drafted", theme::muted()),
    ]);
    y += 4;
    let note = if sp.pos_exact { "per-position acceptance" } else { "per-position (÷ all drafts; set spec_k_set)" };
    put(buf, x0, y, w, vec![Span::styled(note, theme::faint())]);
    y += 1;
    let barw = w.saturating_sub(4 + 8);
    for (p, rate) in sp.pos_rate.iter().enumerate() {
        if y >= inner.bottom().saturating_sub(1) && p + 1 < sp.pos_rate.len() {
            break;
        }
        if y >= inner.bottom() {
            break;
        }
        put(buf, x0, y, 4, vec![Span::styled(format!("+{}", p + 1), theme::muted())]);
        let v = rate.unwrap_or(0.0);
        hbar(buf, x0 + 4, y, barw, v, th().mtp.at(0.35 + 0.65 * v));
        let txt = rate.map(|v| format!("{:>6.1}%", v * 100.0)).unwrap_or_else(|| "     —".into());
        put(buf, x0 + 4 + barw + 1, y, 8, vec![Span::styled(txt, theme::text())]);
        y += 1;
    }
    if y < inner.bottom() {
        // acceptance-length sparkline over the window
        let sw = w.saturating_sub(11);
        let tl = Timeline::new(win, sw, st.complete_sec());
        let vals: Vec<Option<f32>> = tl.columns(|a, b| {
            let (mut d, mut acc) = (0.0f32, 0.0f32);
            for t in a..b {
                if let Some(bk) = st.series.get(t) {
                    d += bk.drafts;
                    acc += bk.accepted;
                }
            }
            (d > 0.0).then(|| 1.0 + acc / d)
        });
        let kmax = (s.num_pos as f32 + 1.0).max(2.0);
        put(buf, x0, y, 11, vec![Span::styled("accept len", theme::muted())]);
        spark(buf, x0 + 11, y, sw, &vals, kmax, &th().mtp);
    }
}

// ---- latency ------------------------------------------------------------------------------------

fn latency(f: &mut Frame, r: Rect, app: &App) {
    let ep = app.ep();
    let st = &ep.st;
    let (win, wl) = app.window();
    let wsecs = (win as f64).min(3600.0);
    let lat = st.latency_window(wsecs);
    // Until the snapshot ring covers the window, say how much it does cover.
    let scope = if lat.secs + 5.0 < wsecs { ago(lat.secs) } else if win > 3600 { "1h".to_string() } else { wl.to_string() };
    let right = vec![Span::styled(format!("{scope} · {} req", count(lat.requests)), theme::muted())];
    let inner = panel(f, r, "LATENCY", "", th().req.key, right);
    let buf = f.buffer_mut();
    let x0 = inner.x + 1;
    let w = inner.width.saturating_sub(2);
    let mut y = inner.y;
    let lw = 12u16;
    let cw = ((w.saturating_sub(lw)) / 3).clamp(7, 10);
    put(buf, x0, y, w, vec![
        Span::styled(format!("{:<lw$}", "", lw = lw as usize), theme::muted()),
        Span::styled(format!("{:>cw$}{:>cw$}{:>cw$}", "p50", "p90", "p99", cw = cw as usize), theme::muted()),
    ]);
    y += 1;
    let rows: [(&str, H); 6] = [
        ("TTFT", H::Ttft),
        ("ITL / step", H::Itl),
        ("TPOT", H::Tpot),
        ("E2E", H::E2e),
        ("queue", H::Queue),
        ("prefill", H::PrefillTime),
    ];
    for (name, h) in rows {
        if y >= inner.bottom() {
            return;
        }
        let hh = lat.hist.get(&h);
        let q = |p: f64| hh.and_then(|x| x.quantile(p)).map(secs).unwrap_or_else(|| "—".into());
        put(buf, x0, y, w, vec![
            Span::styled(format!("{name:<lw$}", lw = lw as usize), theme::text2()),
            Span::styled(format!("{:>cw$}{:>cw$}{:>cw$}", q(0.5), q(0.9), q(0.99), cw = cw as usize), theme::text()),
        ]);
        y += 1;
    }
    y += 1;
    let mut info = |label: &str, val: String, unit: &str| {
        if y < inner.bottom() {
            put(buf, x0, y, w, vec![
                Span::styled(format!("{label:<lw$}", lw = lw as usize), theme::muted()),
                Span::styled(val, theme::text()),
                Span::styled(unit.to_string(), theme::muted()),
            ]);
            y += 1;
        }
    };
    info("decode/req", lat.per_req_decode.map(si).unwrap_or_else(|| "—".into()), " tok/s");
    info("prefill/req", lat.per_req_prefill.map(si).unwrap_or_else(|| "—".into()), " tok/s (excl. cached)");
    info("mean prompt", lat.prompt_mean.map(si).unwrap_or_else(|| "—".into()), " tok");
}

// ---- GPU ----------------------------------------------------------------------------------------

fn gpu(f: &mut Frame, r: Rect, app: &App) {
    let inner = panel(f, r, "GPU", "", th().gpu.key, vec![]);
    let buf = f.buffer_mut();
    let x0 = inner.x + 1;
    let w = inner.width.saturating_sub(2);
    if app.gpu.samples.is_empty() {
        let msg = if app.gpu.unavailable { "NVML not available on this host" } else { "reading NVML…" };
        put(buf, x0, inner.y, w, vec![Span::styled(msg, theme::muted())]);
        return;
    }
    let mut gs: Vec<_> = app.gpu.samples.iter().collect();
    gs.sort_by_key(|g| std::cmp::Reverse(g.mem_total.unwrap_or(0)));
    let namew = gs.iter().map(|g| g.name.chars().count()).max().unwrap_or(4).min(18) as u16 + 3;
    let n = gs.len() as u16;
    let spare = inner.height.saturating_sub(2 * n);
    let chart_h = if spare >= 3 * n { spare / n } else { 0 };
    let (win, _) = app.window();
    let mut y = inner.y;
    for g in gs {
        if y + 1 > inner.bottom() {
            break;
        }
        let name: String = g.name.chars().take(18).collect();
        put(buf, x0, y, namew, vec![Span::styled(format!("{} ", g.index), theme::faint()), Span::styled(name, theme::bold())]);
        let rest = w.saturating_sub(namew);
        // "util " bar " 100%  " "mem " bar " 234/251G"
        let barw = ((rest.saturating_sub(32)) / 2).clamp(4, 24);
        let mut x = x0 + namew;
        let util = g.util.unwrap_or(0) as f64 / 100.0;
        x = put(buf, x, y, 5, vec![Span::styled("util ", theme::muted())]);
        hbar(buf, x, y, barw, util, th().gpu.at(0.3 + 0.7 * util));
        x += barw;
        x = put(buf, x, y, 6, vec![Span::styled(format!("{:>4.0}%", util * 100.0), theme::text())]);
        x += 2;
        if let (Some(u), Some(t)) = (g.mem_used, g.mem_total) {
            let fr = u as f64 / t as f64;
            x = put(buf, x, y, 4, vec![Span::styled("mem ", theme::muted())]);
            if x + barw + 10 <= inner.right() {
                // vLLM preallocates most of HBM, so a full bar is normal: no status colors here.
                hbar(buf, x, y, barw, fr, th().muted);
                x += barw;
            }
            let gib = |b: u64| b as f64 / (1u64 << 30) as f64;
            put(buf, x, y, inner.right().saturating_sub(x), vec![Span::styled(format!(" {:.0}/{:.0}G", gib(u), gib(t)), theme::text())]);
        }
        y += 1;
        if y >= inner.bottom() {
            break;
        }
        let mut x = x0 + namew;
        // Second row; falls back to a compact form when the panel is narrow.
        let row2 = |compact: bool| -> Vec<Span<'static>> {
            let mut sp = vec![];
            if let Some(c) = g.sm_clock {
                sp.push(Span::styled(if compact { "" } else { "sm " }, theme::muted()));
                sp.push(Span::styled(format!("{c}"), theme::text()));
                if let (Some(m), false) = (g.sm_clock_max, compact) {
                    sp.push(Span::styled(format!("/{m}"), theme::muted()));
                }
                sp.push(Span::styled(if compact { "MHz " } else { " MHz  " }, theme::muted()));
            }
            if let Some(t) = g.temp {
                sp.push(Span::styled(format!("{t}°C{}", if compact { " " } else { "  " }), theme::text()));
            }
            if let Some(m) = g.mem_util {
                sp.push(Span::styled(if compact { "bw " } else { "mem-bw " }, theme::muted()));
                sp.push(Span::styled(format!("{m}%{}", if compact { " " } else { "  " }), theme::text()));
            }
            if let Some(p) = g.power_w {
                sp.push(Span::styled(if compact { "" } else { "pwr " }, theme::muted()));
                sp.push(Span::styled(format!("{p:.0}"), theme::text()));
                if let Some(l) = g.power_limit_w {
                    sp.push(Span::styled(format!("/{l:.0}"), theme::muted()));
                }
                sp.push(Span::styled(if compact { "W" } else { " W  " }, theme::muted()));
            }
            sp
        };
        let avail = inner.right().saturating_sub(x) as usize;
        let full = row2(false);
        let sp = if full.iter().map(|s| s.width()).sum::<usize>() <= avail { full } else { row2(true) };
        x = put(buf, x, y, inner.right().saturating_sub(x), sp);
        let hist = app.gpu.util.get(&g.index);
        let gpu_end = app.gpu.t.floor() as i64;
        if chart_h == 0
            && let Some(h) = hist {
                let sw = inner.right().saturating_sub(x + 1);
                if sw >= 8 {
                    let tl = Timeline::new(sw as i64, sw, gpu_end);
                    let vals: Vec<Option<f32>> = hist_columns(h, &tl).iter().map(|c| c.seen.then_some(c.mean)).collect();
                    spark(buf, x, y, sw, &vals, 100.0, &th().gpu);
                }
            }
        y += 1;
        if chart_h > 0 && y < inner.bottom() {
            let ch = chart_h.min(inner.bottom() - y).saturating_sub(1).max(1);
            let cr = Rect::new(x0 + namew, y, w.saturating_sub(namew), ch);
            buf.set_stringn(x0 + namew - 6, y, " 100%", 5, theme::faint());
            buf.set_stringn(x0 + namew - 6, y + ch - 1, "   0%", 5, theme::faint());
            if let Some(h) = hist {
                guide(buf, cr.x, cr.y, cr.width);
                let tl = Timeline::new(win.min(3600), cr.width, gpu_end);
                area(buf, cr, &hist_columns(h, &tl), 100.0, &th().gpu);
            }
            y += chart_h;
        }
    }
}

/// Per-second (unix second, value) history in time order, binned like the throughput charts.
pub(crate) fn hist_columns(h: &std::collections::VecDeque<(i64, f32)>, tl: &Timeline) -> Vec<Col> {
    tl.columns(|a, b| {
        let lo = h.partition_point(|&(s, _)| s < a);
        let mut c = Col::default();
        let (mut sum, mut k) = (0.0f32, 0u32);
        for &(_, v) in h.range(lo..).take_while(|&&(s, _)| s < b) {
            sum += v;
            k += 1;
            c.max = c.max.max(v);
        }
        if k > 0 {
            c.seen = true;
            c.mean = sum / k as f32;
        }
        c
    })
}

/// GPU above HOST in one column. The GPU panel keeps its per-device charts only when there's room.
fn system_stack(f: &mut Frame, r: Rect, app: &App, gpu_on: bool, host_on: bool, ngpu: u16) {
    match (gpu_on, host_on) {
        (true, true) => {
            let compact = 2 + 2 * ngpu;
            let with_charts = 2 + 5 * ngpu;
            let gh = if r.height >= with_charts + 10 { with_charts } else { compact };
            let [g, h] = Layout::vertical([Constraint::Length(gh), Constraint::Min(0)]).areas(r);
            gpu(f, g, app);
            host(f, h, app);
        }
        (true, false) => gpu(f, r, app),
        (false, true) => host(f, r, app),
        _ => {}
    }
}

fn gib(b: u64) -> String {
    let g = b as f64 / (1u64 << 30) as f64;
    if g >= 100.0 { format!("{g:.0}G") } else if g >= 10.0 { format!("{g:.1}G") } else { format!("{g:.2}G") }
}

// ---- host CPU / memory --------------------------------------------------------------------------

fn host(f: &mut Frame, r: Rect, app: &App) {
    let hs = &app.host;
    let Some(h) = hs.last.as_ref() else {
        let inner = panel(f, r, "HOST", "", th().cpu.key, vec![]);
        put(f.buffer_mut(), inner.x + 1, inner.y, inner.width, vec![Span::styled("reading /proc…", theme::muted())]);
        return;
    };
    let sub = format!("{} · {} cores · {}", h.hostname, h.cores.len(), gib(h.mem_total));
    let right = vec![
        Span::styled("load ", theme::muted()),
        Span::styled(format!("{:.2} {:.2} {:.2}", h.load[0], h.load[1], h.load[2]), theme::text2()),
    ];
    let inner = panel(f, r, "HOST", &sub, th().cpu.key, right);
    let buf = f.buffer_mut();
    let x0 = inner.x + 1;
    let w = inner.width.saturating_sub(2);
    let right_edge = inner.right().saturating_sub(1);
    let mut y = inner.y;

    // row: cpu bar | mem stacked bar
    let half = w / 2;
    let barw = half.saturating_sub(4 + 8).clamp(4, 40);
    let mut x = put(buf, x0, y, 4, vec![Span::styled("cpu ", theme::muted())]);
    hbar(buf, x, y, barw, h.cpu as f64, th().cpu.at(0.3 + 0.7 * h.cpu as f64));
    x += barw;
    let mut cpu_txt = vec![Span::styled(format!("{:>6.1}%", h.cpu * 100.0), theme::bold())];
    if h.iowait >= 0.005 {
        cpu_txt.push(Span::styled(format!(" io {:.1}%", h.iowait * 100.0), Style::new().fg(th().warn)));
    }
    put(buf, x, y, (x0 + half).saturating_sub(x), cpu_txt);
    let mx = x0 + half + 1;
    let x = put(buf, mx, y, 4, vec![Span::styled("mem ", theme::muted())]);
    let mtxt = format!(" {}/{}", gib(h.mem_used()), gib(h.mem_total));
    let mbar = right_edge.saturating_sub(x + mtxt.len() as u16).min(40);
    if mbar >= 4 && h.mem_total > 0 {
        let tot = h.mem_total as f64;
        let other = h.mem_total.saturating_sub(h.anon + h.shmem + h.cache + h.mem_free);
        let segs = [(h.anon, th().mem_anon), (h.shmem, th().mem_shmem), (other, th().mem_other), (h.cache, th().mem_cache)];
        // whole cells per segment, rounding the running sum so the total stays exact
        let mut acc = 0.0;
        let mut cx = x;
        for c in 0..mbar {
            buf[(x + c, y)].set_char(' ').set_bg(th().track);
        }
        for (v, col) in segs {
            acc += v as f64;
            let end = x + ((acc / tot) * mbar as f64).round() as u16;
            while cx < end.min(x + mbar) {
                buf[(cx, y)].set_char('█').set_fg(col).set_bg(th().track);
                cx += 1;
            }
        }
        put(buf, x + mbar, y, right_edge.saturating_sub(x + mbar) + 1, vec![Span::styled(mtxt, theme::bold())]);
    }
    y += 1;

    // per-core strip | available memory
    if y < inner.bottom() {
        let cw = half.saturating_sub(6);
        put(buf, x0, y, 6, vec![Span::styled("cores ", theme::muted())]);
        let n = h.cores.len().max(1);
        let cells = (cw as usize).min(n).max(1);
        let group = n.div_ceil(cells);
        let vals: Vec<Option<f32>> = h.cores.chunks(group).map(|c| Some(c.iter().sum::<f32>() / c.len() as f32)).collect();
        spark(buf, x0 + 6, y, vals.len() as u16, &vals, 1.0, &th().cpu);
        let used_frac = 1.0 - h.mem_avail as f64 / h.mem_total.max(1) as f64;
        let mut l = vec![
            Span::styled("    avail ", theme::muted()),
            Span::styled(gib(h.mem_avail), Style::new().fg(level(used_frac)).add_modifier(Modifier::BOLD)),
        ];
        if h.swap_total > 0 {
            l.push(Span::styled(format!(" · swap {}/{}", gib(h.swap_used), gib(h.swap_total)), theme::muted()));
        }
        put(buf, mx, y, right_edge.saturating_sub(mx) + 1, l);
        y += 1;
    }

    // memory composition legend (full width, so identity never rests on color alone)
    if y < inner.bottom() {
        let other = h.mem_total.saturating_sub(h.anon + h.shmem + h.cache + h.mem_free);
        let sw = |c: Color| Span::styled("■ ", Style::new().fg(c));
        let item = |k: &'static str, v: u64| vec![Span::styled(k, theme::muted()), Span::styled(format!("{}   ", gib(v)), theme::text())];
        let mut l = vec![Span::styled("mem  ", theme::muted())];
        for (c, k, v) in [(th().mem_anon, "anon ", h.anon), (th().mem_shmem, "shmem ", h.shmem), (th().mem_other, "kernel+other ", other), (th().mem_cache, "page cache ", h.cache)] {
            l.push(sw(c));
            l.extend(item(k, v));
        }
        l.push(Span::styled("free ", theme::muted()));
        l.push(Span::styled(gib(h.mem_free), theme::text()));
        put(buf, x0, y, w, l);
        y += 1;
    }

    // busiest processes
    if y < inner.bottom() && !h.top.is_empty() {
        let mut spans = vec![Span::styled("top  ", theme::muted())];
        let mut used = 5usize;
        for p in &h.top {
            let item = format!("{} {:.0}% {}", p.name, p.cpu, gib(p.rss));
            if used + item.chars().count() + 3 > w as usize {
                break;
            }
            used += item.chars().count() + 3;
            spans.push(Span::styled(if p.vllm { "● " } else { "  " }, Style::new().fg(th().decode.key)));
            spans.push(Span::styled(p.name.clone(), if p.vllm { theme::text() } else { theme::text2() }));
            spans.push(Span::styled(format!(" {:.0}% ", p.cpu), theme::bold()));
            spans.push(Span::styled(gib(p.rss), theme::muted()));
            spans.push(Span::raw(" "));
        }
        put(buf, x0, y, w, spans);
        y += 1;
    }

    // CPU and memory history, side by side (separate axes)
    let ch = inner.bottom().saturating_sub(y + 1);
    if ch >= 2 {
        y += 1;
        let (win, _) = app.window();
        let end = hs.t.floor() as i64;
        let gut = 5u16;
        let cwid = half.saturating_sub(gut + 1);
        let lc = Rect::new(x0 + gut, y, cwid, ch);
        let rc = Rect::new(mx + gut, y, right_edge.saturating_sub(mx + gut) + 1, ch);
        buf.set_stringn(x0, y, "100%", 4, theme::faint());
        buf.set_stringn(x0, y + ch - 1, "  0%", 4, theme::faint());
        guide(buf, lc.x, lc.y, lc.width);
        let tl = Timeline::new(win.min(3600), lc.width, end);
        area(buf, lc, &hist_columns(&hs.cpu, &tl), 100.0, &th().cpu);
        buf.set_stringn(lc.x + 1, lc.y, " cpu ", 5, theme::muted());
        buf.set_stringn(mx, y, format!("{:>4}", gib(h.mem_total)), 4, theme::faint());
        buf.set_stringn(mx, y + ch - 1, "   0", 4, theme::faint());
        guide(buf, rc.x, rc.y, rc.width);
        let tl = Timeline::new(win.min(3600), rc.width, end);
        area(buf, rc, &hist_columns(&hs.mem, &tl), h.mem_total as f64, &th().mem);
        buf.set_stringn(rc.x + 1, rc.y, " mem used ", 10, theme::muted());
    }
}

// ---- LiteLLM requests ---------------------------------------------------------------------------

fn requests(f: &mut Frame, r: Rect, app: &App) {
    let ep = app.ep();
    // which LiteLLM host, and how this endpoint's rows are picked out of its spend log
    let host = app
        .cfg
        .litellm
        .as_ref()
        .and_then(|l| l.url.as_deref())
        .and_then(crate::litellm::host_port)
        .map(|(h, p)| if p == 443 || p == 80 { h } else { format!("{h}:{p}") })
        .unwrap_or_default();
    let what = match &ep.cfg.litellm_model_group {
        Some(p) => p.clone(),
        None => crate::litellm::host_port(&ep.cfg.url).map(|(_, p)| format!("api_base :{p}")).unwrap_or_default(),
    };
    let mut right = vec![Span::styled(format!("LiteLLM {host} · {what}"), theme::muted())];
    if let Some(e) = &ep.ll_err {
        let short: String = e.chars().take(40).collect();
        right.push(Span::styled(format!(" · ✕ {short}"), Style::new().fg(th().crit)));
    } else if let Some(t) = ep.ll_t {
        right.push(Span::styled(format!(" · {} in 6h · polled {} ago", ep.ll_rows.len(), ago(now() - t)), theme::muted()));
    }
    let inner = panel(f, r, "REQUESTS", "recent, ≤60 s lag", th().req.key, right);
    let buf = f.buffer_mut();
    let x0 = inner.x + 1;
    let w = inner.width.saturating_sub(2);
    if ep.ll_rows.is_empty() {
        let msg = if ep.ll_t.is_some() { "no requests in the last 6 h" } else { "querying LiteLLM…" };
        put(buf, x0, inner.y, w, vec![Span::styled(msg, theme::muted())]);
        return;
    }
    // start | client | prompt | out | ttft | tok/s | dur
    let fixed = 9 + 9 + 7 + 8 + 7 + 8 + 2;
    let cw = w.saturating_sub(fixed).clamp(6, 48) as usize;
    let hdr = format!("{:<9}{:<cw$} {:>8}{:>7}{:>8}{:>7}{:>8}", "start", "client", "prompt", "out", "ttft", "tok/s", "dur");
    put(buf, x0, inner.y, w, vec![Span::styled(hdr, theme::muted())]);
    let maxp = ep.ll_rows.iter().map(|r| r.prompt).max().unwrap_or(1).max(1) as f64;
    for (i, row) in ep.ll_rows.iter().enumerate() {
        let y = inner.y + 1 + i as u16;
        if y >= inner.bottom() {
            break;
        }
        let client: String = if row.client.is_empty() { "—".into() } else { row.client.chars().take(cw).collect() };
        let ok = row.status.is_empty() || row.status == "success";
        let st = if ok { theme::text() } else { Style::new().fg(th().crit) };
        let mut x = put(buf, x0, y, w, vec![
            Span::styled(format!("{:<9}", clock(row.start)), theme::text2()),
            Span::styled(format!("{client:<cw$} "), st),
        ]);
        // prompt size with a tiny inline magnitude bar
        let frac = row.prompt as f64 / maxp;
        let prompt = format!("{:>7}", si(row.prompt as f64));
        x = put(buf, x, y, 8, vec![Span::styled(prompt, theme::text())]);
        buf[(x, y)].set_char(['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'][((frac * 7.0).round() as usize).min(7)]).set_fg(th().req.at(0.25 + 0.6 * frac));
        x += 1;
        let ttft = row.ttft().map(secs).unwrap_or_else(|| "—".into());
        let rate = row.decode_rate().map(si).unwrap_or_else(|| "—".into());
        put(buf, x, y, inner.right().saturating_sub(x), vec![
            Span::styled(format!("{:>7}", count(row.completion as f64)), theme::text()),
            Span::styled(format!("{ttft:>8}"), theme::text()),
            Span::styled(format!("{rate:>7}"), theme::text()),
            Span::styled(format!("{:>8}", secs(row.end - row.start)), theme::text2()),
        ]);
    }
}

// ---- overlays -----------------------------------------------------------------------------------

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height.saturating_sub(2));
    Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h)
}

fn help(f: &mut Frame, area: Rect, app: &App) {
    let r = centered(area, 96, 34);
    clear(f, r, app);
    let b = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(th().text2))
        .title_top(Line::from(vec![Span::styled(" ▍", Style::new().fg(th().decode.key)), Span::styled("lilmon help ", theme::title())]));
    let k = |s: &'static str| Span::styled(format!("{s:<12}"), theme::bold());
    let d = |s: &'static str| Span::styled(s, theme::text2());
    let h = |s: &'static str| Line::from(Span::styled(s, Style::new().fg(th().text).add_modifier(Modifier::BOLD | Modifier::UNDERLINED)));
    let p = |s: &'static str| Line::from(Span::styled(s, theme::text2()));
    let lines = vec![
        h("Keys"),
        Line::from(vec![k("q / esc"), d("quit (esc closes an overlay first)")]),
        Line::from(vec![k("← → [ ] w"), d("chart window: 1m 5m 15m 1h 6h 24h (also scopes MTP and latency)")]),
        Line::from(vec![k("tab / 1-9"), d("switch endpoint")]),
        Line::from(vec![k("s"), d("settings: the running config, applied live; w there writes it to the config file")]),
        Line::from(vec![k("t / T"), d("next / previous theme (lilmon --list-themes shows them all)")]),
        Line::from(vec![k("b"), d("background: theme surface, black, or none (terminal transparency)")]),
        Line::from(vec![k("r"), d("all-time records for this endpoint's models")]),
        Line::from(vec![k("g / c / l"), d("toggle GPU / host CPU+memory / LiteLLM request panels")]),
        Line::from(vec![k("space / p"), d("pause redraws (data keeps collecting)")]),
        Line::from(""),
        h("Numbers"),
        p("Rates are per 1-second bucket. \"now\" averages the last 2 s (decode) or 3 s (prefill). Peaks 1m/5m/15m are the highest 1-second rate in that trailing window. Chart columns hold a whole number of seconds anchored to the clock, so history scrolls without reshaping; the drawn span is the window rounded to fit the width (tick labels show true times). When a column packs several seconds, the mean is solid and the span up to the max is dim."),
        Line::from(""),
        p("Prefill counts computed (non-cached) prompt tokens. vLLM credits a request's prompt only when its first token is produced, so each credited batch is spread back over the TTFT of the requests that just finished prefill. The last few seconds can be revised when a long prefill completes; while one is running the panels show ▶ prefilling with elapsed time. 'cached' is prefix-cache hits."),
        Line::from(""),
        p("MTP: accept len = 1 + accepted/drafts (tokens emitted per verify step per sequence); accept % = accepted/drafted tokens. When k varies by batch size, set spec_k_set (e.g. [3, 5]) so per-position rates divide by the drafts that actually included that position."),
        Line::from(""),
        p("Latency percentiles come from histogram deltas over the window (max 1 h). ITL is per engine step, which with speculative decoding covers several tokens. decode/req is one sequence's speed; per-seq is aggregate decode divided by running sequences."),
        Line::from(""),
        p("Requests come from LiteLLM's spend log, so they cover LiteLLM-routed traffic only and land up to ~60 s after completion. History and records persist in ~/.local/state/lilmon/."),
        Line::from(""),
        Line::from(vec![Span::styled("Config file  ", theme::muted()), Span::styled(app.config_path.display().to_string(), theme::text())]),
        Line::from(Span::styled("lilmon · © 2026 Local Inference Lab, Inc. · Apache-2.0", theme::faint())),
    ];
    f.render_widget(Paragraph::new(lines).block(b).wrap(Wrap { trim: false }), r);
}

fn settings(f: &mut Frame, area: Rect, app: &App) {
    let r = centered(area, 84, 20);
    clear(f, r, app);
    let modified = app.config_modified();
    let b = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(th().text2))
        .title_top(Line::from(vec![
            Span::styled(" ▍", Style::new().fg(th().decode.key)),
            Span::styled("running config ", theme::title()),
        ]))
        .title_top(
            Line::from(if modified {
                vec![Span::styled(" ● unsaved changes ", Style::new().fg(th().warn))]
            } else {
                vec![Span::styled(" saved ", theme::muted())]
            })
            .right_aligned(),
        );
    let inner = b.inner(r);
    f.render_widget(b, r);
    let buf = f.buffer_mut();
    let x0 = inner.x + 2;
    let w = inner.width.saturating_sub(4);
    let t = th();
    let mut y = inner.y + 1;
    for (i, s) in SETTINGS.iter().enumerate() {
        if y >= inner.bottom() {
            break;
        }
        let sel = i == app.set_sel;
        let (label, value, note): (&str, String, String) = match s {
            Setting::Theme => ("theme", t.name.to_string(), String::new()),
            Setting::Background => (
                "background",
                app.bg.label(),
                match app.bg {
                    theme::Background::Theme => "the theme's own surface".into(),
                    theme::Background::None => "terminal default; keeps transparency".into(),
                    theme::Background::Color(_) => "solid color".into(),
                },
            ),
            Setting::Window => ("chart window", WINDOWS[app.win].1.to_string(), "also scopes MTP and latency".into()),
            Setting::Interval => (
                "scrape interval",
                format!("{} ms", app.interval_ms.load(Ordering::Relaxed)),
                "how often /metrics is read".into(),
            ),
            Setting::Gpu => ("GPU panel", if app.show_gpu { "on" } else { "off" }.into(), String::new()),
            Setting::Host => ("host panel", if app.show_host { "on" } else { "off" }.into(), "CPU, memory, top processes".into()),
            Setting::Requests => ("requests panel", if app.show_req { "on" } else { "off" }.into(), "LiteLLM rows".into()),
        };
        let row_style = if sel { Style::new().bg(t.tab_bg) } else { Style::new() };
        if sel {
            buf.set_style(Rect::new(inner.x + 1, y, inner.width.saturating_sub(2), 1), row_style);
        }
        let arrow = |c: &'static str| Span::styled(c, if sel { Style::new().fg(t.decode.key) } else { theme::faint() });
        put(buf, x0, y, w, vec![
            Span::styled(if sel { "▸ " } else { "  " }, Style::new().fg(t.decode.key)),
            Span::styled(format!("{label:<17}"), if sel { theme::bold() } else { theme::text2() }),
            arrow("◂ "),
            Span::styled(format!("{value:<10}"), theme::bold()),
            arrow(" ▸"),
            Span::styled(format!("   {note}"), theme::muted()),
        ]);
        y += 1;
        if *s == Setting::Theme {
            // description (word-wrapped, two lines at most) and swatches for the active theme
            let dw = w.saturating_sub(21) as usize;
            let mut lines: Vec<String> = vec![String::new()];
            for word in t.description.split_whitespace() {
                let cur = lines.last_mut().unwrap();
                if !cur.is_empty() && cur.chars().count() + 1 + word.chars().count() > dw {
                    lines.push(word.to_string());
                } else {
                    if !cur.is_empty() {
                        cur.push(' ');
                    }
                    cur.push_str(word);
                }
            }
            for l in lines.iter().take(2) {
                put(buf, x0 + 19, y, w.saturating_sub(19), vec![Span::styled(l.clone(), theme::muted())]);
                y += 1;
            }
            let mut sw = vec![];
            for (name, rp) in [("decode", &t.decode), ("prefill", &t.prefill), ("mtp", &t.mtp), ("kv", &t.kv), ("gpu", &t.gpu), ("cpu", &t.cpu)] {
                sw.push(Span::styled("██", Style::new().fg(rp.key)));
                sw.push(Span::styled(format!(" {name}  "), theme::faint()));
            }
            put(buf, x0 + 19, y, w.saturating_sub(19), sw);
            y += 2;
        }
    }
    let fy = inner.bottom().saturating_sub(3);
    put(buf, x0, fy, w, vec![
        Span::styled("file  ", theme::muted()),
        Span::styled(app.config_path.display().to_string(), theme::text()),
    ]);
    let k = |s: &'static str| Span::styled(s, Style::new().fg(t.text).bg(t.tab_bg));
    let d = |s: &'static str| Span::styled(s, theme::muted());
    put(buf, x0, fy + 2, w, vec![
        k(" ↑↓ "), d(" select   "), k(" ←→ "), d(" change   "), k(" w "), d(" write to file (old kept as .bak)   "), k(" esc "), d(" close"),
    ]);
}

fn records(f: &mut Frame, area: Rect, app: &App) {
    let ep = app.ep();
    let prefix = format!("{}/", ep.cfg.name);
    let recs: Vec<_> = app.records.iter().filter(|(k, _)| k.starts_with(&prefix)).collect();
    let r = centered(area, 110, (recs.len() as u16 * 8 + 4).max(8));
    clear(f, r, app);
    let b = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(th().text2))
        .title_top(Line::from(vec![
            Span::styled(" ▍", Style::new().fg(th().warn)),
            Span::styled(format!("records · {} ", ep.cfg.name), theme::title()),
        ]));
    let mut lines = vec![];
    if recs.is_empty() {
        lines.push(Line::from(Span::styled("no records yet", theme::muted())));
    }
    let date = |t: f64| {
        let d = (now() - t).max(0.0);
        format!("{} ({} ago)", clock(t), ago(d))
    };
    for (k, m) in recs {
        lines.push(Line::from(vec![
            Span::styled(k.trim_start_matches(&prefix).to_string(), theme::bold()),
            Span::styled(format!("   tracked for {}", ago(now() - m.first_seen)), theme::muted()),
        ]));
        let row = |name: &str, v: String, at: f64| {
            let v = if at > 0.0 { v } else { "—".to_string() };
            Line::from(vec![
                Span::styled(format!("  {name:<26}"), theme::muted()),
                Span::styled(format!("{v:<12}"), theme::text()),
                Span::styled(if at > 0.0 { date(at) } else { String::new() }, theme::faint()),
            ])
        };
        lines.push(row("decode, best 1 s", format!("{} tok/s", si(m.decode.v)), m.decode.at));
        lines.push(row("prefill, best 1 s", format!("{} tok/s", si(m.prefill.v)), m.prefill.at));
        lines.push(row("single prefill ≥32K tok", format!("{} tok/s", si(m.prefill_req.v)), m.prefill_req.at));
        lines.push(row("concurrency", format!("{:.0}", m.running.v), m.running.at));
        lines.push(row("KV cache usage", pct(m.kv.v), m.kv.at));
        lines.push(row("accept len, best 1 min", format!("{:.2}", m.accept_len.v), m.accept_len.at));
        lines.push(Line::from(""));
    }
    f.render_widget(Paragraph::new(lines).block(b), r);
}

#[cfg(test)]
mod tests {
    use super::Timeline;

    /// As the leading edge advances, every completed bin must keep exactly the same seconds and the
    /// same column width; it may only move left by whole bins.
    #[test]
    fn timeline_bins_are_stable_while_scrolling() {
        for win in [60, 300, 900, 3600, 21600, 86400] {
            for width in [37u16, 91, 190] {
                let mut prev: Option<Vec<(i64, i64)>> = None;
                for end in 1_791_230_000i64..1_791_230_000 + 2000 {
                    let tl = Timeline::new(win, width, end);
                    assert_eq!(tl.x_offset() + tl.nbins as u16 * tl.cpb, width);
                    let bins: Vec<(i64, i64)> = (0..tl.nbins).map(|i| tl.bin_secs(i)).collect();
                    if let Some(p) = &prev {
                        // all complete bins of the previous frame that are still on screen are unchanged
                        for b in &p[..p.len() - 1] {
                            if b.0 >= bins[0].0 {
                                assert!(bins.contains(b), "win {win} width {width}: bin {b:?} changed");
                            }
                        }
                    }
                    prev = Some(bins);
                }
                let tl = Timeline::new(win, width, 0);
                let ratio = tl.span() as f64 / win as f64;
                assert!((0.6..=1.6).contains(&ratio), "win {win} width {width}: span {}", tl.span());
            }
        }
    }
}
