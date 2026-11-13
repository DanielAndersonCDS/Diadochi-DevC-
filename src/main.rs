#![cfg_attr(all(not(debug_assertions), windows), windows_subsystem = "windows")]

mod autoedit;
mod compiler;
mod format;
mod highlight;
mod safety;
mod theme;

use compiler::{BuildOptions, BuildResult, Toolchain};
use eframe::egui::{self, Color32, FontId, RichText};
use safety::{Diagnostic, Lang, Severity};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use theme::{Palette, ThemeKind};

// ───────────────────────── settings ─────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum Mode {
    Strict,  // errors block the build, like rustc
    Relaxed, // everything is shown, nothing blocks
    Off,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::Strict => "Strict",
            Mode::Relaxed => "Relaxed",
            Mode::Off => "Off",
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    theme: ThemeKind,
    mode: Mode,
    cpp_std: String,
    c_std: String,
    deep_analysis: bool,
    stack_protector: bool,
    font_size: f32,
    toolchain_dir: String,
    auto_edit: bool,
    format_on_save: bool,
    power10: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemeKind::Mocha,
            mode: Mode::Strict,
            cpp_std: "c++11".into(),
            c_std: "c11".into(),
            deep_analysis: true,
            stack_protector: false,
            font_size: 15.0,
            toolchain_dir: String::new(),
            auto_edit: true,
            format_on_save: false,
            power10: false,
        }
    }
}

// ───────────────────────── sample code ─────────────────────────

const TEMPLATE_CPP: &str = r#"#include <iostream>
#include <string>

int main() {
    std::string name = "world";
    std::cout << "Hello, " << name << "!\n";
    return 0;
}
"#;

const TEMPLATE_C: &str = r#"#include <stdio.h>

int main(void) {
    printf("Hello, world!\n");
    return 0;
}
"#;

const DEMO_C: &str = r#"#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* Diadochi demo: every problem below is deliberate. */
char *make_greeting(const char *name) {
    char buf[16];
    strcpy(buf, name);
    return buf;
}

int main(void) {
    char name[8];
    int scores[5];
    int total;
    int *data = malloc(5 * sizeof(int));
    scanf("%s", name);
    for (int i = 0; i <= 5; i++) {
        scores[i] = i;
    }
    printf("%d\n", total);
    free(data);
    printf("%d\n", data[0]);
    return 0;
}
"#;

// ───────────────────────── app state ─────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Problems,
    Build,
    Run,
}

#[derive(Clone, Copy)]
enum Action {
    NewC,
    NewCpp,
    Demo,
    Open,
    Save,
    SaveAs,
    Format,
    Check,
    Build,
    BuildRun,
    Settings,
    Rules,
}

struct JobMsg {
    result: BuildResult,
    run_after: bool,
}

struct App {
    settings: Settings,
    applied_theme: Option<ThemeKind>,
    text: String,
    path: Option<PathBuf>,
    scratch_lang: Lang,
    dirty: bool,
    toolchain: Option<Toolchain>,
    safety: Vec<Diagnostic>,
    gcc_diags: Vec<Diagnostic>,
    build_log: String,
    run_log: String,
    tab: Tab,
    status: String,
    rx: Option<Receiver<JobMsg>>,
    busy: bool,
    jump_to: Option<usize>,
    cursor: (usize, usize),
    show_settings: bool,
    show_rules: bool,
    last_title: String,
    p10_build: Option<bool>,
}

fn first_readable(paths: &[PathBuf]) -> Option<Vec<u8>> {
    paths.iter().find_map(|p| std::fs::read(p).ok())
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    let mut mono: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            mono.push(dir.join("assets").join("JetBrainsMono-Regular.ttf"));
        }
    }
    mono.push(PathBuf::from(format!("{local}\\Microsoft\\Windows\\Fonts\\JetBrainsMono-Regular.ttf")));
    mono.push(PathBuf::from("C:\\Windows\\Fonts\\JetBrainsMono-Regular.ttf"));
    mono.push(PathBuf::from("C:\\Windows\\Fonts\\consola.ttf"));
    if let Some(bytes) = first_readable(&mono) {
        fonts.font_data.insert("diadochi-mono".into(), egui::FontData::from_owned(bytes).into());
        fonts.families.entry(egui::FontFamily::Monospace).or_default().insert(0, "diadochi-mono".into());
    }
    if let Some(bytes) = first_readable(&[PathBuf::from("C:\\Windows\\Fonts\\segoeui.ttf")]) {
        fonts.font_data.insert("diadochi-ui".into(), egui::FontData::from_owned(bytes).into());
        fonts.families.entry(egui::FontFamily::Proportional).or_default().insert(0, "diadochi-ui".into());
    }
    ctx.set_fonts(fonts);
}

fn char_index_of_line(text: &str, line_1based: usize) -> usize {
    let mut idx = 0;
    for (n, l) in text.split('\n').enumerate() {
        if n + 1 == line_1based {
            return idx;
        }
        idx += l.chars().count() + 1;
    }
    idx.saturating_sub(1)
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let settings: Settings = cc
            .storage
            .and_then(|s| eframe::get_value(s, "settings"))
            .unwrap_or_default();
        install_fonts(&cc.egui_ctx);
        cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false); // we zoom the editor font ourselves
        let toolchain = compiler::detect(&settings.toolchain_dir);
        let mut app = Self {
            settings,
            applied_theme: None,
            text: TEMPLATE_CPP.to_string(),
            path: None,
            scratch_lang: Lang::Cpp,
            dirty: false,
            toolchain,
            safety: Vec::new(),
            gcc_diags: Vec::new(),
            build_log: String::new(),
            run_log: String::new(),
            tab: Tab::Problems,
            status: "Ready".into(),
            rx: None,
            busy: false,
            jump_to: None,
            cursor: (1, 1),
            show_settings: false,
            show_rules: false,
            last_title: String::new(),
            p10_build: None,
        };
        app.recheck();
        app
    }

    fn lang(&self) -> Lang {
        match &self.path {
            Some(p) => {
                let is_c = p.extension().map_or(false, |e| e.to_string_lossy().eq_ignore_ascii_case("c"));
                if is_c { Lang::C } else { Lang::Cpp }
            }
            None => self.scratch_lang,
        }
    }

    fn display_name(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "untitled".into())
    }

    fn format_now(&mut self) {
        let formatted = format::format_source(&self.text);
        if formatted != self.text {
            self.text = formatted;
            self.dirty = true;
            self.gcc_diags.clear();
            self.recheck();
        }
        self.jump_to = Some(self.cursor.0);
        self.status = "Formatted".into();
    }

    fn recheck(&mut self) {
        self.safety = if self.settings.mode == Mode::Off {
            Vec::new()
        } else {
            safety::analyze_opts(&self.text, self.lang(), self.settings.power10)
        };
    }

    fn load_text(&mut self, text: &str, path: Option<PathBuf>, lang: Lang) {
        self.text = text.to_string();
        self.path = path;
        self.scratch_lang = lang;
        self.dirty = false;
        self.gcc_diags.clear();
        self.recheck();
    }

    fn open_file(&mut self) {
        let Some(p) = rfd::FileDialog::new()
            .add_filter("C / C++", &["c", "cpp", "cc", "cxx", "h", "hpp"])
            .pick_file()
        else {
            return;
        };
        match std::fs::read_to_string(&p) {
            Ok(t) => {
                self.load_text(&t.replace("\r\n", "\n"), Some(p.clone()), Lang::Cpp);
                self.status = format!("Opened {}", p.display());
            }
            Err(e) => self.status = format!("Could not open file: {e}"),
        }
    }

    fn save_as(&mut self) {
        let ext = if self.lang() == Lang::C { "c" } else { "cpp" };
        let Some(p) = rfd::FileDialog::new().set_file_name(format!("main.{ext}")).save_file() else { return };
        self.path = Some(p);
        self.save();
        self.recheck();
    }

    fn save(&mut self) {
        let Some(p) = self.path.clone() else {
            self.save_as();
            return;
        };
        if self.settings.format_on_save {
            self.format_now();
        }
        match std::fs::write(&p, &self.text) {
            Ok(_) => {
                self.dirty = false;
                self.status = format!("Saved {}", p.display());
            }
            Err(e) => self.status = format!("Save failed: {e}"),
        }
    }

    /// The compiler needs a real file. Unsaved buffers go to a scratch file in the temp dir.
    fn ensure_on_disk(&mut self) -> Option<PathBuf> {
        if let Some(p) = self.path.clone() {
            if self.dirty {
                self.save();
            }
            return Some(p);
        }
        let ext = if self.scratch_lang == Lang::C { "c" } else { "cpp" };
        let p = std::env::temp_dir().join(format!("diadochi_scratch.{ext}"));
        match std::fs::write(&p, &self.text) {
            Ok(_) => Some(p),
            Err(e) => {
                self.status = format!("Could not write scratch file: {e}");
                None
            }
        }
    }

    fn start_build(&mut self, ctx: &egui::Context, run_after: bool) {
        if self.busy {
            return;
        }
        self.recheck();
        let errors = self.safety.iter().filter(|d| d.severity == Severity::Error).count();
        if self.settings.mode == Mode::Strict && errors > 0 {
            self.build_log = format!(
                "error: could not compile `{}` due to {} previous safety error{}\n\n\
                 Strict mode refuses to build code that breaks a memory-safety rule.\n\
                 Fix the problems, or waive one on purpose, with a reason:\n\n    \
                 // diadochi:allow(DR002) name is capped at 7 chars by the caller\n",
                self.display_name(),
                errors,
                if errors == 1 { "" } else { "s" }
            );
            self.tab = Tab::Problems;
            self.status = format!("Blocked by safety rules: {errors} error(s)");
            return;
        }
        let Some(tc) = self.toolchain.clone() else {
            self.status = "No Dev-C++ compiler found. Open Settings and point to the Dev-Cpp folder.".into();
            self.tab = Tab::Build;
            return;
        };
        let Some(path) = self.ensure_on_disk() else { return };
        let lang = self.lang();
        let opts = BuildOptions {
            std_flag: if lang == Lang::C { self.settings.c_std.clone() } else { self.settings.cpp_std.clone() },
            deep: self.settings.deep_analysis,
            stack_protector: self.settings.stack_protector,
            strict: self.settings.mode == Mode::Strict,
            p10: self.settings.power10,
        };
        let (tx, rx) = channel();
        self.rx = Some(rx);
        self.busy = true;
        self.status = "Compiling...".into();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = compiler::build(&tc, &path, lang, &opts);
            let _ = tx.send(JobMsg { result, run_after });
            ctx.request_repaint();
        });
    }

    fn on_built(&mut self, m: JobMsg) {
        self.busy = false;
        let r = m.result;
        self.build_log = format!(
            "$ {}\n\n{}\n{}",
            r.command,
            r.raw,
            if r.ok { "Build succeeded." } else { "Build failed." }
        );
        if self.settings.power10 {
            self.p10_build = Some(r.ok);
        }
        self.gcc_diags = r.diagnostics;
        self.tab = if r.ok { Tab::Build } else { Tab::Problems };
        self.status = if r.ok { "Build succeeded".into() } else { "Build failed".into() };
        if r.ok && m.run_after {
            self.launch(&r.exe);
        }
    }

    fn launch(&mut self, exe: &std::path::Path) {
        let dir = exe.parent().map(|d| d.to_path_buf());
        #[cfg(windows)]
        let spawned = {
            use std::os::windows::process::CommandExt;
            let mut c = std::process::Command::new("cmd");
            // new console window that stays open after the program ends, like Dev-C++'s console pauser
            c.arg("/C").raw_arg(format!("\"\"{}\" & echo. & echo [Diadochi] program finished & pause\"", exe.display()));
            c.creation_flags(0x0000_0010); // CREATE_NEW_CONSOLE
            if let Some(d) = &dir {
                c.current_dir(d);
            }
            c.spawn()
        };
        #[cfg(not(windows))]
        let spawned = {
            let mut c = std::process::Command::new(exe);
            if let Some(d) = &dir {
                c.current_dir(d);
            }
            c.spawn()
        };
        match spawned {
            Ok(_) => {
                self.run_log = format!("Launched {}\n", exe.display());
                self.status = "Running".into();
            }
            Err(e) => {
                self.run_log = format!("Could not launch {}: {e}\n", exe.display());
                self.tab = Tab::Run;
            }
        }
    }

    fn counts(&self) -> (usize, usize, usize) {
        let all = self.safety.iter().chain(self.gcc_diags.iter());
        let (mut e, mut w, mut v) = (0, 0, 0);
        for d in all {
            match d.severity {
                Severity::Error => e += 1,
                Severity::Warning => w += 1,
                Severity::Waived => v += 1,
                Severity::Note => {}
            }
        }
        (e, w, v)
    }

    // ───────────── UI pieces ─────────────

    fn toolbar(&mut self, ui: &mut egui::Ui, pal: &Palette) -> Option<Action> {
        let mut act = None;
        ui.horizontal(|ui| {
            ui.label(RichText::new("DIADOCHI").strong().color(pal.accent).size(16.0));
            ui.label(RichText::new("Dev C++").color(pal.subtext).size(16.0));
            ui.separator();
            ui.menu_button("New", |ui| {
                if ui.button("C++ file").clicked() {
                    act = Some(Action::NewCpp);
                    ui.close_menu();
                }
                if ui.button("C file").clicked() {
                    act = Some(Action::NewC);
                    ui.close_menu();
                }
                if ui.button("Unsafe C demo").clicked() {
                    act = Some(Action::Demo);
                    ui.close_menu();
                }
            });
            if ui.button("Open").clicked() {
                act = Some(Action::Open);
            }
            if ui.button("Save").clicked() {
                act = Some(Action::Save);
            }
            if ui.button("Save as").clicked() {
                act = Some(Action::SaveAs);
            }
            ui.separator();
            if ui.button("Format  Ctrl+Shift+F").clicked() {
                act = Some(Action::Format);
            }
            if ui.button("Check").clicked() {
                act = Some(Action::Check);
            }
            if ui.add_enabled(!self.busy, egui::Button::new("Build  F9")).clicked() {
                act = Some(Action::Build);
            }
            let run = egui::Button::new(RichText::new("Build & Run  F11").strong().color(pal.on_accent)).fill(pal.accent);
            if ui.add_enabled(!self.busy, run).clicked() {
                act = Some(Action::BuildRun);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Settings").clicked() {
                    act = Some(Action::Settings);
                }
                if ui.button("Rules").clicked() {
                    act = Some(Action::Rules);
                }
                egui::ComboBox::from_id_source("theme_combo")
                    .selected_text(self.settings.theme.name())
                    .show_ui(ui, |ui| {
                        for t in ThemeKind::ALL {
                            ui.selectable_value(&mut self.settings.theme, t, t.name());
                        }
                    });
                let p10_before = self.settings.power10;
                ui.checkbox(&mut self.settings.power10, "Power of 10");
                if p10_before != self.settings.power10 {
                    if self.settings.power10 {
                        if self.settings.mode == Mode::Off {
                            self.settings.mode = Mode::Strict;
                        }
                        self.settings.theme = ThemeKind::Space;
                    }
                    self.p10_build = None;
                    act = Some(Action::Check);
                }
                let before = self.settings.mode;
                egui::ComboBox::from_id_source("mode_combo")
                    .selected_text(format!("Safety: {}", self.settings.mode.name()))
                    .show_ui(ui, |ui| {
                        for m in [Mode::Strict, Mode::Relaxed, Mode::Off] {
                            ui.selectable_value(&mut self.settings.mode, m, m.name());
                        }
                    });
                if before != self.settings.mode {
                    act = Some(Action::Check);
                }
            });
        });
        act
    }

    fn editor_ui(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        let font = FontId::monospace(self.settings.font_size);
        let row_h = ui.fonts(|f| f.row_height(&font));
        let id = egui::Id::new("diadochi_editor");

        let mut scroll_y: Option<f32> = None;
        if let Some(line) = self.jump_to.take() {
            let idx = char_index_of_line(&self.text, line);
            let mut st = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
            st.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(idx))));
            st.store(ui.ctx(), id);
            ui.memory_mut(|m| m.request_focus(id));
            scroll_y = Some(((line as f32 - 1.0) * row_h - 80.0).max(0.0));
        }

        // gutter colors: red for errors, amber for warnings
        let mut marks: HashMap<usize, Color32> = HashMap::new();
        for d in self.safety.iter().chain(self.gcc_diags.iter()) {
            let c = match d.severity {
                Severity::Error => pal.error,
                Severity::Warning => pal.warning,
                _ => continue,
            };
            let slot = marks.entry(d.line).or_insert(c);
            if d.severity == Severity::Error {
                *slot = pal.error;
            }
        }
        let n_lines = self.text.split('\n').count().max(1);
        let width = n_lines.to_string().len();
        let mut gutter = egui::text::LayoutJob::default();
        for ln in 1..=n_lines {
            let color = marks.get(&ln).copied().unwrap_or(pal.gutter);
            let s = if ln < n_lines { format!("{:>w$}\n", ln, w = width) } else { format!("{:>w$}", ln, w = width) };
            gutter.append(&s, 0.0, egui::TextFormat::simple(font.clone(), color));
        }

        let mut pre_changed = false;
        if self.settings.auto_edit {
            pre_changed = autoedit::process(ui.ctx(), id, &mut self.text);
        }

        let avail = ui.available_size();
        let mut area = egui::ScrollArea::both().auto_shrink([false, false]);
        if let Some(y) = scroll_y {
            area = area.vertical_scroll_offset(y);
        }
        let mut changed = false;
        let mut cursor: Option<(usize, usize)> = None;
        let text = &mut self.text;
        area.show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.add_space(2.0);
                    ui.add(egui::Label::new(gutter).selectable(false));
                });
                ui.add_space(6.0);
                let mut layouter = |ui: &egui::Ui, s: &str, _wrap: f32| {
                    ui.fonts(|f| f.layout_job(highlight::layout(s, pal, font.clone())))
                };
                let out = egui::TextEdit::multiline(text)
                    .id(id)
                    .font(font.clone())
                    .lock_focus(true)
                    .frame(false)
                    .desired_width((avail.x - 70.0).max(200.0))
                    .desired_rows(((avail.y / row_h) as usize).max(10))
                    .layouter(&mut layouter)
                    .show(ui);
                changed = out.response.changed();
                if let Some(cr) = out.cursor_range {
                    cursor = Some((cr.primary.rcursor.row + 1, cr.primary.rcursor.column + 1));
                }
            });
        });
        if let Some(c) = cursor {
            self.cursor = c;
        }
        if changed || pre_changed {
            self.dirty = true;
            self.p10_build = None;
            self.gcc_diags.clear();
            self.recheck();
        }
    }


    fn checklist_ui(&self, ui: &mut egui::Ui, pal: &Palette) -> Option<usize> {
        let mut counts = [0usize; 10];
        let mut first_line: [Option<usize>; 10] = [None; 10];
        for d in self.safety.iter() {
            if !matches!(d.severity, Severity::Error | Severity::Warning) || !d.code.starts_with("P10-") {
                continue;
            }
            if let Ok(n) = d.code[4..].parse::<usize>() {
                if (1..=10).contains(&n) {
                    counts[n - 1] += 1;
                    first_line[n - 1].get_or_insert(d.line);
                }
            }
        }
        let checked = [true, true, true, true, true, false, false, true, true, true];
        let mut jump = None;

        ui.label(RichText::new("PRE-LAUNCH CHECKLIST").strong().size(15.0).color(pal.accent));
        ui.label(RichText::new("NASA/JPL Power of 10 (heuristic check)").small().color(pal.subtext));
        ui.add_space(8.0);

        let mut total = 0usize;
        for n in 1..=10usize {
            let i = n - 1;
            let (color, status) = if n == 10 {
                match self.p10_build {
                    Some(true) => (pal.ok, "GO".to_string()),
                    Some(false) => (pal.error, "NO-GO".to_string()),
                    None => (pal.warning, "BUILD".to_string()),
                }
            } else if !checked[i] {
                (pal.gutter, "N/A".to_string())
            } else if counts[i] == 0 {
                (pal.ok, "GO".to_string())
            } else {
                total += counts[i];
                (pal.error, format!("NO-GO x{}", counts[i]))
            };
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 26.0), egui::Sense::click());
            if resp.hovered() {
                ui.painter().rect_filled(rect, 2.0, pal.surface);
            }
            let p = ui.painter();
            p.circle_filled(rect.left_center() + egui::vec2(10.0, 0.0), 6.0, color);
            p.text(
                rect.left_center() + egui::vec2(26.0, 0.0),
                egui::Align2::LEFT_CENTER,
                format!("{:>2}  {}", n, safety::P10_TITLES[i]),
                FontId::monospace(12.5),
                pal.text,
            );
            p.text(rect.right_center() - egui::vec2(4.0, 0.0), egui::Align2::RIGHT_CENTER, status, FontId::monospace(12.0), color);
            if resp.clicked() {
                jump = first_line[i];
            }
        }
        ui.add_space(10.0);
        ui.separator();
        let go = total == 0 && self.p10_build != Some(false);
        let (txt, col) = if go {
            ("ALL CHECKED RULES: GO".to_string(), pal.ok)
        } else {
            (format!("NO-GO: {} violation(s)", total), pal.error)
        };
        ui.label(RichText::new(txt).strong().size(14.0).color(col));
        ui.add_space(4.0);
        ui.label(RichText::new("Rules 6 and 7 are not checked yet. Rule 10 needs a build (-Werror).\nA clean result is not a certification.").small().color(pal.subtext));
        jump
    }

    fn problems_ui(&self, ui: &mut egui::Ui, pal: &Palette) -> Option<usize> {
        let mut all: Vec<&Diagnostic> = self.safety.iter().chain(self.gcc_diags.iter()).collect();
        all.sort_by_key(|d| (d.severity, d.line));
        let src: Vec<&str> = self.text.split('\n').collect();
        let (e, w, v) = self.counts();
        let mut jump = None;

        if all.is_empty() {
            ui.label(RichText::new("No problems found.").color(pal.ok).strong());
            if self.settings.mode == Mode::Off {
                ui.label(RichText::new("Safety rules are switched off.").color(pal.subtext));
            }
            return None;
        }
        let summary = format!("{e} error(s), {w} warning(s), {v} waived");
        ui.label(RichText::new(summary).color(if e > 0 { pal.error } else { pal.warning }).strong());
        ui.separator();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for d in all {
                if problem_card(ui, d, &src, pal) {
                    jump = Some(d.line);
                }
                ui.separator();
            }
        });
        jump
    }
}

fn problem_card(ui: &mut egui::Ui, d: &Diagnostic, src: &[&str], pal: &Palette) -> bool {
    let (color, label) = match d.severity {
        Severity::Error => (pal.error, "error"),
        Severity::Warning => (pal.warning, "warning"),
        Severity::Note => (pal.info, "note"),
        Severity::Waived => (pal.subtext, "waived"),
    };
    let mut clicked = false;
    ui.vertical(|ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(format!("{}[{}]", label, d.code)).color(color).strong().monospace());
            ui.label(RichText::new(&d.message).color(pal.text));
        });
        if ui.link(RichText::new(format!("--> line {}:{}", d.line, d.col_start + 1)).monospace()).clicked() {
            clicked = true;
        }
        if let Some(l) = src.get(d.line.saturating_sub(1)) {
            let text = l.trim_end_matches('\r').replace('\t', " ");
            ui.label(RichText::new(format!("{:>5} | {}", d.line, text)).monospace().color(pal.text));
            let width = d.col_end.saturating_sub(d.col_start).max(1);
            ui.label(
                RichText::new(format!("      | {}{}", " ".repeat(d.col_start), "^".repeat(width)))
                    .monospace()
                    .color(color),
            );
        }
        if let Some(h) = &d.help {
            ui.label(RichText::new(format!("      = help: {h}")).monospace().color(pal.info));
        }
        if let Some(r) = &d.waived_reason {
            ui.label(RichText::new(format!("      = waived: {r}")).monospace().color(pal.subtext));
        }
    });
    clicked
}

impl eframe::App for App {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, "settings", &self.settings);
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // finished background builds
        let msgs: Vec<JobMsg> = self.rx.as_ref().map(|rx| rx.try_iter().collect()).unwrap_or_default();
        for m in msgs {
            self.on_built(m);
        }

        // theme
        if self.applied_theme != Some(self.settings.theme) {
            theme::apply(ctx, &self.settings.theme.palette());
            self.applied_theme = Some(self.settings.theme);
        }
        let pal = self.settings.theme.palette();

        // window title
        let title = format!("{}{} - Diadochi Dev C++", self.display_name(), if self.dirty { " *" } else { "" });
        if title != self.last_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }

        // shortcuts (same keys as Dev-C++)
        let mut act: Option<Action> = None;
        ctx.input(|i| {
            if i.key_pressed(egui::Key::F9) {
                act = Some(Action::Build);
            }
            if i.key_pressed(egui::Key::F11) {
                act = Some(Action::BuildRun);
            }
            if i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::F) {
                act = Some(Action::Format);
            } else if i.modifiers.command && i.key_pressed(egui::Key::S) {
                act = Some(Action::Save);
            }
            // zoom: Ctrl+wheel, Ctrl +/-, Ctrl+0
            let mut size = self.settings.font_size;
            if (i.zoom_delta() - 1.0).abs() > f32::EPSILON {
                size *= i.zoom_delta();
            }
            if i.modifiers.command {
                if i.key_pressed(egui::Key::Plus) || i.key_pressed(egui::Key::Equals) {
                    size += 1.0;
                }
                if i.key_pressed(egui::Key::Minus) {
                    size -= 1.0;
                }
                if i.key_pressed(egui::Key::Num0) {
                    size = 15.0;
                }
            }
            self.settings.font_size = size.clamp(8.0, 48.0);
        });

        egui::TopBottomPanel::top("toolbar")
            .frame(egui::Frame::none().fill(pal.panel).inner_margin(egui::Margin::symmetric(12.0_f32, 8.0_f32)).stroke(egui::Stroke::new(1.0_f32, pal.border)))
            .show(ctx, |ui| {
                if let Some(a) = self.toolbar(ui, &pal) {
                    act = Some(a);
                }
            });

        egui::TopBottomPanel::bottom("status")
            .frame(egui::Frame::none().fill(pal.panel).inner_margin(egui::Margin::symmetric(12.0_f32, 4.0_f32)).stroke(egui::Stroke::new(1.0_f32, pal.border)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    match &self.toolchain {
                        Some(tc) => ui.label(RichText::new(&tc.label).color(pal.subtext).small()),
                        None => ui.label(RichText::new("Dev-C++ compiler not found (Settings)").color(pal.error).small()),
                    };
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (e, w, v) = self.counts();
                        let lang = if self.lang() == Lang::C { "C" } else { "C++" };
                        ui.label(RichText::new(format!("Ln {}, Col {}   {}", self.cursor.0, self.cursor.1, lang)).small().color(pal.subtext));
                        ui.label(RichText::new(format!("{e} err  {w} warn  {v} waived")).small().color(if e > 0 { pal.error } else { pal.subtext }));
                        ui.label(RichText::new(&self.status).small().color(pal.text));
                    });
                });
            });

        let mut jump: Option<usize> = None;
        egui::TopBottomPanel::bottom("output")
            .resizable(true)
            .default_height(230.0)
            .min_height(90.0)
            .frame(egui::Frame::none().fill(pal.panel).inner_margin(egui::Margin::same(8.0_f32)).stroke(egui::Stroke::new(1.0_f32, pal.border)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let (e, w, _) = self.counts();
                    ui.selectable_value(&mut self.tab, Tab::Problems, format!("Problems ({})", e + w));
                    ui.selectable_value(&mut self.tab, Tab::Build, "Build output");
                    ui.selectable_value(&mut self.tab, Tab::Run, "Run");
                });
                ui.separator();
                match self.tab {
                    Tab::Problems => jump = self.problems_ui(ui, &pal),
                    Tab::Build => {
                        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                            let log = if self.build_log.is_empty() { "Nothing built yet." } else { &self.build_log };
                            ui.add(egui::Label::new(RichText::new(log).monospace().color(pal.text)).selectable(true));
                        });
                    }
                    Tab::Run => {
                        let log = if self.run_log.is_empty() { "Use Build & Run (F11). Programs open in their own console window." } else { &self.run_log };
                        ui.label(RichText::new(log).monospace().color(pal.text));
                    }
                }
            });
        if let Some(l) = jump {
            self.jump_to = Some(l);
        }

        if self.settings.power10 && self.settings.mode != Mode::Off {
            let mut jump2: Option<usize> = None;
            egui::SidePanel::right("p10_checklist")
                .resizable(false)
                .exact_width(290.0)
                .frame(egui::Frame::none().fill(pal.panel).inner_margin(egui::Margin::same(12.0_f32)).stroke(egui::Stroke::new(1.0_f32, pal.border)))
                .show(ctx, |ui| {
                    jump2 = self.checklist_ui(ui, &pal);
                });
            if let Some(l) = jump2 {
                self.jump_to = Some(l);
            }
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(pal.bg).inner_margin(egui::Margin::symmetric(8.0_f32, 6.0_f32)))
            .show(ctx, |ui| self.editor_ui(ui, &pal));

        // windows
        let mut open = self.show_settings;
        egui::Window::new("Settings").open(&mut open).resizable(false).show(ctx, |ui| {
            ui.label("Dev-C++ folder (install folder or its compiler bin folder)");
            ui.horizontal(|ui| {
                ui.text_edit_singleline(&mut self.settings.toolchain_dir);
                if ui.button("Browse").clicked() {
                    if let Some(p) = rfd::FileDialog::new().pick_folder() {
                        self.settings.toolchain_dir = p.display().to_string();
                    }
                }
                if ui.button("Detect").clicked() {
                    self.toolchain = compiler::detect(&self.settings.toolchain_dir);
                    self.status = if self.toolchain.is_some() { "Compiler found".into() } else { "Compiler not found".into() };
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("C++ standard");
                egui::ComboBox::from_id_source("cpp_std").selected_text(self.settings.cpp_std.clone()).show_ui(ui, |ui| {
                    for s in ["c++11", "c++14", "c++17", "gnu++11", "gnu++14"] {
                        ui.selectable_value(&mut self.settings.cpp_std, s.to_string(), s);
                    }
                });
                ui.label("C standard");
                egui::ComboBox::from_id_source("c_std").selected_text(self.settings.c_std.clone()).show_ui(ui, |ui| {
                    for s in ["c99", "c11", "gnu99", "gnu11"] {
                        ui.selectable_value(&mut self.settings.c_std, s.to_string(), s);
                    }
                });
            });
            ui.checkbox(&mut self.settings.deep_analysis, "Deep analysis (-O2 so GCC's flow warnings fire)");
            ui.checkbox(&mut self.settings.stack_protector, "Stack protector (-fstack-protector-strong; needs libssp)");
            ui.checkbox(&mut self.settings.auto_edit, "Auto-close brackets/quotes, smart Enter, Backspace and Tab");
            ui.checkbox(&mut self.settings.format_on_save, "Format on save");
            ui.add(egui::Slider::new(&mut self.settings.font_size, 11.0..=24.0).text("Editor font size"));
            ui.separator();
            ui.label(RichText::new("Strict: safety errors block the build (like rustc).\nRelaxed: shown, never blocking.   Off: no checks.").color(pal.subtext));
        });
        self.show_settings = open;

        let mut open = self.show_rules;
        egui::Window::new("Safety rules").open(&mut open).default_width(560.0).show(ctx, |ui| {
            egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                for (code, desc) in safety::RULE_DOCS {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(*code).monospace().strong().color(pal.accent));
                        ui.label(*desc);
                    });
                }
                ui.separator();
                ui.label(RichText::new("Waive one finding on purpose (like unsafe, but you must say why):\n// diadochi:allow(DR002) reason").monospace().color(pal.subtext));
            });
        });
        self.show_rules = open;

        // run the requested action
        match act {
            Some(Action::NewC) => self.load_text(TEMPLATE_C, None, Lang::C),
            Some(Action::NewCpp) => self.load_text(TEMPLATE_CPP, None, Lang::Cpp),
            Some(Action::Demo) => {
                self.load_text(DEMO_C, None, Lang::C);
                self.tab = Tab::Problems;
            }
            Some(Action::Open) => self.open_file(),
            Some(Action::Save) => self.save(),
            Some(Action::SaveAs) => self.save_as(),
            Some(Action::Format) => self.format_now(),
            Some(Action::Check) => {
                self.recheck();
                self.tab = Tab::Problems;
            }
            Some(Action::Build) => self.start_build(ctx, false),
            Some(Action::BuildRun) => self.start_build(ctx, true),
            Some(Action::Settings) => self.show_settings = true,
            Some(Action::Rules) => self.show_rules = true,
            None => {}
        }
    }
}

fn load_icon() -> Option<egui::IconData> {
    let img = image::load_from_memory(include_bytes!("../assets/icon.png")).ok()?.into_rgba8();
    let (width, height) = img.dimensions();
    Some(egui::IconData { rgba: img.into_raw(), width, height })
}

fn main() -> eframe::Result<()> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1280.0_f32, 820.0_f32])
        .with_min_inner_size([820.0_f32, 520.0_f32])
        .with_title("Diadochi Dev C++");
    if let Some(icon) = load_icon() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native("Diadochi Dev C++", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}
