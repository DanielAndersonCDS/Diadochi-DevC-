//! Three themes: Bloomberg terminal (dark), Catppuccin Mocha (dark), Windows 11 Fluent (light).

use eframe::egui::{self, Color32, Rounding, Stroke, Visuals};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub enum ThemeKind {
    Bloomberg,
    Mocha,
    Fluent,
}

#[derive(Clone, Copy)]
pub struct Palette {
    pub dark: bool,
    pub bg: Color32,      // editor background
    pub panel: Color32,   // toolbars and panels
    pub surface: Color32, // buttons, cards
    pub border: Color32,
    pub text: Color32,
    pub subtext: Color32,
    pub accent: Color32,
    pub on_accent: Color32,
    pub error: Color32,
    pub warning: Color32,
    pub ok: Color32,
    pub info: Color32,
    pub keyword: Color32,
    pub typ: Color32,
    pub string: Color32,
    pub number: Color32,
    pub comment: Color32,
    pub preproc: Color32,
    pub func: Color32,
    pub gutter: Color32,
    pub selection: Color32,
    pub radius: f32,
}

const fn hex(h: u32) -> Color32 {
    Color32::from_rgb((h >> 16) as u8, (h >> 8) as u8, h as u8)
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let m = |x: u8, y: u8| (x as f32 * (1.0 - t) + y as f32 * t).round() as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

impl ThemeKind {
    pub const ALL: [ThemeKind; 3] = [ThemeKind::Bloomberg, ThemeKind::Mocha, ThemeKind::Fluent];

    pub fn name(self) -> &'static str {
        match self {
            ThemeKind::Bloomberg => "Bloomberg (dark)",
            ThemeKind::Mocha => "Catppuccin Mocha (dark)",
            ThemeKind::Fluent => "Windows 11 (light)",
        }
    }

    pub fn palette(self) -> Palette {
        match self {
            ThemeKind::Bloomberg => Palette {
                dark: true,
                bg: hex(0x000000),
                panel: hex(0x0A0A0A),
                surface: hex(0x161616),
                border: hex(0x2E2E2E),
                text: hex(0xE6E6E6),
                subtext: hex(0x8C8C8C),
                accent: hex(0xFB8B1E),
                on_accent: hex(0x000000),
                error: hex(0xFF433D),
                warning: hex(0xFFD000),
                ok: hex(0x4AF6C3),
                info: hex(0x2E9BFF),
                keyword: hex(0xFB8B1E),
                typ: hex(0x2E9BFF),
                string: hex(0x4AF6C3),
                number: hex(0xFFD000),
                comment: hex(0x6B6B6B),
                preproc: hex(0xA0A0A0),
                func: hex(0xFFFFFF),
                gutter: hex(0x555555),
                selection: hex(0x5A3A0E),
                radius: 0.0,
            },
            ThemeKind::Mocha => Palette {
                dark: true,
                bg: hex(0x1E1E2E),
                panel: hex(0x181825),
                surface: hex(0x313244),
                border: hex(0x45475A),
                text: hex(0xCDD6F4),
                subtext: hex(0xA6ADC8),
                accent: hex(0xCBA6F7),
                on_accent: hex(0x11111B),
                error: hex(0xF38BA8),
                warning: hex(0xF9E2AF),
                ok: hex(0xA6E3A1),
                info: hex(0x89B4FA),
                keyword: hex(0xCBA6F7),
                typ: hex(0xF9E2AF),
                string: hex(0xA6E3A1),
                number: hex(0xFAB387),
                comment: hex(0x6C7086),
                preproc: hex(0xF5C2E7),
                func: hex(0x89B4FA),
                gutter: hex(0x6C7086),
                selection: hex(0x45475A),
                radius: 8.0,
            },
            ThemeKind::Fluent => Palette {
                dark: false,
                bg: hex(0xFFFFFF),
                panel: hex(0xF3F3F3),
                surface: hex(0xFBFBFB),
                border: hex(0xDCDCDC),
                text: hex(0x1B1B1B),
                subtext: hex(0x616161),
                accent: hex(0x0067C0),
                on_accent: hex(0xFFFFFF),
                error: hex(0xC42B1C),
                warning: hex(0x9D5D00),
                ok: hex(0x0F7B0F),
                info: hex(0x0067C0),
                keyword: hex(0x0000FF),
                typ: hex(0x267F99),
                string: hex(0xA31515),
                number: hex(0x098658),
                comment: hex(0x008000),
                preproc: hex(0xAF00DB),
                func: hex(0x795E26),
                gutter: hex(0x9A9A9A),
                selection: hex(0xCCE4F7),
                radius: 6.0,
            },
        }
    }
}

pub fn apply(ctx: &egui::Context, p: &Palette) {
    let mut v = if p.dark { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = p.panel;
    v.window_fill = p.panel;
    v.extreme_bg_color = p.bg;
    v.faint_bg_color = p.surface;
    v.code_bg_color = p.surface;
    v.hyperlink_color = p.accent;
    v.warn_fg_color = p.warning;
    v.error_fg_color = p.error;
    v.selection.bg_fill = p.selection;
    v.selection.stroke = Stroke::new(1.0_f32, p.accent);
    v.window_stroke = Stroke::new(1.0_f32, p.border);

    let r = Rounding::same(p.radius);
    v.widgets.noninteractive.rounding = r;
    v.widgets.inactive.rounding = r;
    v.widgets.hovered.rounding = r;
    v.widgets.active.rounding = r;
    v.widgets.open.rounding = r;

    v.widgets.noninteractive.bg_fill = p.panel;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, p.border);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, p.text);

    v.widgets.inactive.bg_fill = p.surface;
    v.widgets.inactive.weak_bg_fill = p.surface;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, p.border);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, p.text);

    let hover = mix(p.surface, p.accent, 0.18);
    v.widgets.hovered.bg_fill = hover;
    v.widgets.hovered.weak_bg_fill = hover;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, p.accent);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, p.text);

    let press = mix(p.surface, p.accent, 0.35);
    v.widgets.active.bg_fill = press;
    v.widgets.active.weak_bg_fill = press;
    v.widgets.active.bg_stroke = Stroke::new(1.0_f32, p.accent);
    v.widgets.active.fg_stroke = Stroke::new(1.0_f32, p.text);

    v.widgets.open.bg_fill = hover;
    v.widgets.open.weak_bg_fill = hover;

    ctx.set_visuals(v);
    ctx.style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0_f32, 6.0_f32);
        s.spacing.button_padding = egui::vec2(10.0_f32, 5.0_f32);
    });
}
