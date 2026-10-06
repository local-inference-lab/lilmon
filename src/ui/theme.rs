// Copyright 2026 Local Inference Lab, Inc.
// SPDX-License-Identifier: Apache-2.0

//! Color themes. Each theme is built from a compact spec: surface and ink tones, four status
//! colors, and one key color per data series. Chart ramps (baseline → top gradient) and the dim
//! envelope tone are derived from the key and the background, so every theme gets the same
//! visual grammar. The active theme is a process-wide index so drawing code can call `th()`.

use ratatui::style::{Color, Modifier, Style};
use std::sync::atomic::{AtomicUsize, Ordering};

pub const fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

const fn t(hex: u32) -> (u8, u8, u8) {
    ((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

const fn chan(a: u32, b: u32, shift: u32, pct_b: u32) -> u32 {
    let x = (a >> shift) & 0xff;
    let y = (b >> shift) & 0xff;
    ((x * (100 - pct_b) + y * pct_b + 50) / 100) << shift
}

/// Blend `a` toward `b` by `pct_b` percent.
const fn mix(a: u32, b: u32, pct_b: u32) -> u32 {
    chan(a, b, 16, pct_b) | chan(a, b, 8, pct_b) | chan(a, b, 0, pct_b)
}

/// A single-hue ramp from the baseline tone (`lo`) to the top tone (`hi`), plus a dim tone for
/// the mean-to-max envelope and the key color used for swatches and bars.
pub struct Ramp {
    pub lo: (u8, u8, u8),
    pub hi: (u8, u8, u8),
    pub band: Color,
    pub key: Color,
}

impl Ramp {
    pub fn at(&self, f: f64) -> Color {
        let f = f.clamp(0.0, 1.0);
        let l = |a: u8, b: u8| (a as f64 + (b as f64 - a as f64) * f).round() as u8;
        Color::Rgb(l(self.lo.0, self.hi.0), l(self.lo.1, self.hi.1), l(self.lo.2, self.hi.2))
    }
}

const fn ramp(key: u32, bg: u32, light: bool) -> Ramp {
    if light {
        // On light surfaces height means more ink: tint near the baseline, deep at the top.
        Ramp { lo: t(mix(key, bg, 50)), hi: t(mix(key, 0x000000, 22)), band: rgb(mix(key, bg, 80)), key: rgb(key) }
    } else {
        Ramp { lo: t(mix(key, bg, 45)), hi: t(mix(key, 0xffffff, 40)), band: rgb(mix(key, bg, 78)), key: rgb(key) }
    }
}

pub struct Theme {
    pub name: &'static str,
    pub description: &'static str,
    pub light: bool,
    pub bg: Color,
    pub text: Color,
    pub text2: Color,
    pub muted: Color,
    pub faint: Color,
    pub border: Color,
    pub grid: Color,
    pub track: Color,
    pub tab_bg: Color,
    pub good: Color,
    pub warn: Color,
    pub serious: Color,
    pub crit: Color,
    pub decode: Ramp,
    pub prefill: Ramp,
    pub mtp: Ramp,
    pub kv: Ramp,
    pub gpu: Ramp,
    pub req: Ramp,
    pub cpu: Ramp,
    pub mem: Ramp,
    pub mem_anon: Color,
    pub mem_shmem: Color,
    pub mem_cache: Color,
    pub mem_other: Color,
}

struct Spec {
    name: &'static str,
    description: &'static str,
    light: bool,
    bg: u32,
    text: u32,
    text2: u32,
    muted: u32,
    faint: u32,
    border: u32,
    grid: u32,
    track: u32,
    tab: u32,
    /// good, warning, serious, critical
    status: [u32; 4],
    /// decode, prefill, MTP, KV, GPU, requests, CPU, memory
    series: [u32; 8],
}

const fn build(s: Spec) -> Theme {
    let [decode, prefill, mtp, kv, gpu, req, cpu, mem] = s.series;
    Theme {
        name: s.name,
        description: s.description,
        light: s.light,
        bg: rgb(s.bg),
        text: rgb(s.text),
        text2: rgb(s.text2),
        muted: rgb(s.muted),
        faint: rgb(s.faint),
        border: rgb(s.border),
        grid: rgb(s.grid),
        track: rgb(s.track),
        tab_bg: rgb(s.tab),
        good: rgb(s.status[0]),
        warn: rgb(s.status[1]),
        serious: rgb(s.status[2]),
        crit: rgb(s.status[3]),
        decode: ramp(decode, s.bg, s.light),
        prefill: ramp(prefill, s.bg, s.light),
        mtp: ramp(mtp, s.bg, s.light),
        kv: ramp(kv, s.bg, s.light),
        gpu: ramp(gpu, s.bg, s.light),
        req: ramp(req, s.bg, s.light),
        cpu: ramp(cpu, s.bg, s.light),
        mem: ramp(mem, s.bg, s.light),
        mem_anon: rgb(mem),
        mem_shmem: rgb(prefill),
        mem_cache: rgb(mix(mem, s.bg, 55)),
        mem_other: rgb(s.muted),
    }
}

pub static THEMES: [Theme; 13] = [
    build(Spec {
        name: "graphite",
        description: "Default. Warm graphite surface with a colorblind-checked palette: blue decode, violet prefill, aqua MTP, amber KV.",
        light: false,
        bg: 0x1a1a19, text: 0xecece8, text2: 0xc3c2b7, muted: 0x85847c, faint: 0x55544f,
        border: 0x4a4a46, grid: 0x2e2e2c, track: 0x2a2a28, tab: 0x2f2f2d,
        status: [0x0ca30c, 0xfab219, 0xec835a, 0xd03b3b],
        series: [0x3987e5, 0x9085e9, 0x199e70, 0xc98500, 0xd95926, 0xd55181, 0x3fa653, 0x6f8fb3],
    }),
    build(Spec {
        name: "midnight",
        description: "Deep navy night sky with crisp electric accents. Calm and high-clarity for dark rooms.",
        light: false,
        bg: 0x0b1020, text: 0xe6edf7, text2: 0xb4c0d3, muted: 0x7686a0, faint: 0x46536b,
        border: 0x2a3550, grid: 0x1a2238, track: 0x18203a, tab: 0x1f2a44,
        status: [0x2fbf71, 0xf2c14e, 0xf08a5d, 0xef4f5f],
        series: [0x4ea1ff, 0xa78bfa, 0x2dd4bf, 0xfbbf24, 0xfb923c, 0xf472b6, 0x4ade80, 0x94a3b8],
    }),
    build(Spec {
        name: "nord",
        description: "Arctic and north-bluish: frosted cyans over polar-night slate, with muted aurora accents (Nord palette).",
        light: false,
        bg: 0x2e3440, text: 0xeceff4, text2: 0xd8dee9, muted: 0x8b95a7, faint: 0x5b6578,
        border: 0x4c566a, grid: 0x3b4252, track: 0x3b4252, tab: 0x434c5e,
        status: [0xa3be8c, 0xebcb8b, 0xd08770, 0xbf616a],
        series: [0x88c0d0, 0xb48ead, 0xa3be8c, 0xebcb8b, 0xd08770, 0x5e81ac, 0x8fbcbb, 0x81a1c1],
    }),
    build(Spec {
        name: "dracula",
        description: "Vivid neon pastels (cyan, purple, pink) on a dusky violet-gray (Dracula palette).",
        light: false,
        bg: 0x282a36, text: 0xf8f8f2, text2: 0xd6d6e0, muted: 0x9aa0c3, faint: 0x5a5f80,
        border: 0x44475a, grid: 0x343746, track: 0x343746, tab: 0x44475a,
        status: [0x50fa7b, 0xf1fa8c, 0xffb86c, 0xff5555],
        series: [0x8be9fd, 0xbd93f9, 0x50fa7b, 0xf1fa8c, 0xffb86c, 0xff79c6, 0x5af78e, 0x7b88c4],
    }),
    build(Spec {
        name: "gruvbox",
        description: "Warm retro groove: cream text, toasted orange and mossy green on a dark-roast brown (Gruvbox palette).",
        light: false,
        bg: 0x282828, text: 0xebdbb2, text2: 0xd5c4a1, muted: 0xa89984, faint: 0x665c54,
        border: 0x504945, grid: 0x3c3836, track: 0x3c3836, tab: 0x504945,
        status: [0xb8bb26, 0xfabd2f, 0xfe8019, 0xfb4934],
        series: [0x83a598, 0xd3869b, 0x8ec07c, 0xfabd2f, 0xfe8019, 0xb16286, 0xb8bb26, 0x928374],
    }),
    build(Spec {
        name: "solarized",
        description: "Precision low-glare palette: balanced accents on a deep teal-ink background (Solarized Dark).",
        light: false,
        bg: 0x002b36, text: 0xeee8d5, text2: 0x93a1a1, muted: 0x6c8890, faint: 0x456068,
        border: 0x2a5560, grid: 0x073642, track: 0x073642, tab: 0x0d4552,
        status: [0x859900, 0xb58900, 0xcb4b16, 0xdc322f],
        series: [0x268bd2, 0x6c71c4, 0x2aa198, 0xb58900, 0xcb4b16, 0xd33682, 0x859900, 0x839496],
    }),
    build(Spec {
        name: "tokyo-night",
        description: "City lights after rain: soft blue and lavender glow over inky indigo (Tokyo Night palette).",
        light: false,
        bg: 0x1a1b26, text: 0xc0caf5, text2: 0xa9b1d6, muted: 0x737aa2, faint: 0x4a5072,
        border: 0x3b4261, grid: 0x24283b, track: 0x24283b, tab: 0x292e42,
        status: [0x9ece6a, 0xe0af68, 0xff9e64, 0xdb4b4b],
        series: [0x7aa2f7, 0xbb9af7, 0x73daca, 0xe0af68, 0xff9e64, 0xf7768e, 0x9ece6a, 0x7dcfff],
    }),
    build(Spec {
        name: "catppuccin",
        description: "Soothing pastels (lavender, sky, peach) on a soft espresso base (Catppuccin Mocha).",
        light: false,
        bg: 0x1e1e2e, text: 0xcdd6f4, text2: 0xbac2de, muted: 0x9399b2, faint: 0x585b70,
        border: 0x45475a, grid: 0x313244, track: 0x313244, tab: 0x45475a,
        status: [0xa6e3a1, 0xf9e2af, 0xfab387, 0xf38ba8],
        series: [0x89b4fa, 0xcba6f7, 0x94e2d5, 0xf9e2af, 0xfab387, 0xf5c2e7, 0xa6e3a1, 0x74c7ec],
    }),
    build(Spec {
        name: "synthwave",
        description: "Outrun neon: hot pink, laser cyan and sunset orange glowing over a midnight-purple grid.",
        light: false,
        bg: 0x1a1027, text: 0xf5e9ff, text2: 0xd4c2ec, muted: 0x9a86b8, faint: 0x5e4c7a,
        border: 0x4a3566, grid: 0x2a1c3f, track: 0x2a1c3f, tab: 0x3a2753,
        status: [0x3cf2a6, 0xffd84d, 0xff9a4d, 0xff4d6d],
        series: [0x36f9f6, 0xff7edb, 0x72f1b8, 0xfede5d, 0xff8b39, 0xfe4450, 0x9df46a, 0xb893ce],
    }),
    build(Spec {
        name: "phosphor",
        description: "Green-phosphor CRT: monochrome glow on black where only alerts break the green. Panel titles identify each series.",
        light: false,
        bg: 0x050a05, text: 0xa8ffa8, text2: 0x74dc74, muted: 0x48a048, faint: 0x2a5f2a,
        border: 0x1f4a1f, grid: 0x0f2a0f, track: 0x0d220d, tab: 0x133413,
        status: [0x39ff14, 0xffcc33, 0xff9933, 0xff4444],
        series: [0x33ff66, 0x7dffb0, 0xa6ff4d, 0xd4ff66, 0x5cff8a, 0x33cc99, 0x66ff66, 0x4ccc4c],
    }),
    build(Spec {
        name: "amber",
        description: "Amber monochrome: the warm glow of a 1980s VT220 on black, with red reserved for alarms.",
        light: false,
        bg: 0x0a0700, text: 0xffcc66, text2: 0xe0a845, muted: 0xa87a2a, faint: 0x5c4314,
        border: 0x4a3510, grid: 0x241a06, track: 0x1f1605, tab: 0x33250a,
        status: [0xc8e05a, 0xffe066, 0xff8040, 0xff3b3b],
        series: [0xffb000, 0xffd46a, 0xff9f1a, 0xffe099, 0xff8c00, 0xffbf40, 0xffa733, 0xcc8f33],
    }),
    build(Spec {
        name: "paper",
        description: "Light theme for light terminals: dark ink on off-white, using the light-mode steps of the default palette.",
        light: true,
        bg: 0xfcfcfb, text: 0x1d1d1b, text2: 0x52514e, muted: 0x7a7973, faint: 0xa9a8a2,
        border: 0xcfcec8, grid: 0xe6e5e0, track: 0xebeae5, tab: 0xe3e2dc,
        status: [0x0b8a0b, 0xb07800, 0xc85a2a, 0xc62828],
        series: [0x2a78d6, 0x4a3aa7, 0x128a5e, 0xa86f00, 0xc9541f, 0xc23c74, 0x008300, 0x5a7896],
    }),
    build(Spec {
        name: "contrast",
        description: "High contrast: pure black, bright white text and fully saturated series for projectors, glare or low vision.",
        light: false,
        bg: 0x000000, text: 0xffffff, text2: 0xe6e6e6, muted: 0xb3b3b3, faint: 0x808080,
        border: 0x9a9a9a, grid: 0x3a3a3a, track: 0x262626, tab: 0x333333,
        status: [0x00ff00, 0xffff00, 0xff8800, 0xff3030],
        series: [0x00b3ff, 0xc58cff, 0x00ffcc, 0xffd000, 0xff7a00, 0xff4fd8, 0x4dff4d, 0xbfbfbf],
    }),
];

static CURRENT: AtomicUsize = AtomicUsize::new(0);

/// The active theme.
pub fn th() -> &'static Theme {
    &THEMES[CURRENT.load(Ordering::Relaxed) % THEMES.len()]
}

pub fn current_index() -> usize {
    CURRENT.load(Ordering::Relaxed) % THEMES.len()
}

pub fn set_index(i: usize) {
    CURRENT.store(i % THEMES.len(), Ordering::Relaxed);
}

pub fn find(name: &str) -> Option<usize> {
    let n = name.trim().to_ascii_lowercase().replace([' ', '_'], "-");
    THEMES.iter().position(|t| t.name == n)
}

/// Background painted behind the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Background {
    /// The theme's own surface.
    Theme,
    /// Leave cells at the terminal default (keeps transparency).
    None,
    Color(Color),
}

impl Background {
    pub fn parse(s: &str) -> Option<Background> {
        let v = s.trim().to_ascii_lowercase();
        Some(match v.as_str() {
            "" | "theme" | "auto" => Background::Theme,
            "none" | "transparent" | "terminal" | "default" => Background::None,
            "black" => Background::Color(rgb(0x000000)),
            _ => {
                let hex = v.strip_prefix('#')?;
                if hex.len() != 6 {
                    return None;
                }
                Background::Color(rgb(u32::from_str_radix(hex, 16).ok()?))
            }
        })
    }

    pub fn label(&self) -> String {
        match self {
            Background::Theme => "theme".into(),
            Background::None => "none".into(),
            Background::Color(Color::Rgb(0, 0, 0)) => "black".into(),
            Background::Color(Color::Rgb(r, g, b)) => format!("#{r:02x}{g:02x}{b:02x}"),
            Background::Color(_) => "theme".into(),
        }
    }

    pub fn color(&self) -> Option<Color> {
        match self {
            Background::Theme => Some(th().bg),
            Background::None => None,
            Background::Color(c) => Some(*c),
        }
    }
}

pub fn title() -> Style {
    Style::new().fg(th().text).add_modifier(Modifier::BOLD)
}
pub fn text() -> Style {
    Style::new().fg(th().text)
}
pub fn text2() -> Style {
    Style::new().fg(th().text2)
}
pub fn muted() -> Style {
    Style::new().fg(th().muted)
}
pub fn faint() -> Style {
    Style::new().fg(th().faint)
}
pub fn bold() -> Style {
    Style::new().fg(th().text).add_modifier(Modifier::BOLD)
}

/// Status color for a 0..1 fill level (KV cache, memory).
pub fn level(f: f64) -> Color {
    let t = th();
    if f >= 0.95 {
        t.crit
    } else if f >= 0.85 {
        t.serious
    } else if f >= 0.70 {
        t.warn
    } else {
        t.good
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_unique_and_findable() {
        for (i, t) in THEMES.iter().enumerate() {
            assert_eq!(find(t.name), Some(i));
            assert!(!t.description.is_empty());
        }
        assert_eq!(find("Tokyo Night"), find("tokyo-night"));
    }

    #[test]
    fn background_parse() {
        assert_eq!(Background::parse("none"), Some(Background::None));
        assert_eq!(Background::parse("#102030"), Some(Background::Color(rgb(0x102030))));
        assert_eq!(Background::parse("black").unwrap().label(), "black");
        assert_eq!(Background::parse("nope"), None);
    }

    #[test]
    fn graphite_ramps_stay_close_to_hand_tuned_originals() {
        let d = &THEMES[0].decode;
        let close = |a: (u8, u8, u8), b: (u8, u8, u8)| {
            (a.0 as i32 - b.0 as i32).abs() <= 24 && (a.1 as i32 - b.1 as i32).abs() <= 24 && (a.2 as i32 - b.2 as i32).abs() <= 24
        };
        assert!(close(d.hi, (0x86, 0xb6, 0xef)));
        assert!(close(d.lo, (0x18, 0x4f, 0x95)));
    }
}
