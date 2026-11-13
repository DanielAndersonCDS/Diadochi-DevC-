//! Finds the compiler bundled with Dev-C++ (MinGW / TDM-GCC) and drives it.

use crate::safety::{Diagnostic, Lang, Severity};
use regex::Regex;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone)]
pub struct Toolchain {
    pub gcc: PathBuf,
    pub gxx: PathBuf,
    pub bin_dir: Option<PathBuf>,
    pub label: String,
    pub major: u32,
}

pub struct BuildOptions {
    pub std_flag: String,
    pub deep: bool,
    pub stack_protector: bool,
    pub strict: bool,
    pub p10: bool,
}

pub struct BuildResult {
    pub ok: bool,
    pub command: String,
    pub raw: String,
    pub diagnostics: Vec<Diagnostic>,
    pub exe: PathBuf,
}

const EXE: &str = if cfg!(windows) { ".exe" } else { "" };

fn quiet(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    #[cfg(not(windows))]
    {
        let _ = cmd;
    }
}

fn version_line(exe: &Path) -> Option<String> {
    let mut c = Command::new(exe);
    c.arg("--version");
    quiet(&mut c);
    let out = c.output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).lines().next().map(|s| s.trim().to_string())
}

fn gcc_major(version: &str) -> u32 {
    Regex::new(r"(\d+)\.\d+\.\d+")
        .unwrap()
        .captures(version)
        .and_then(|c| c[1].parse().ok())
        .unwrap_or(0)
}

fn tool_in(dir: &Path, name: &str) -> Option<PathBuf> {
    let p = dir.join(format!("{name}{EXE}"));
    p.is_file().then_some(p)
}

fn from_bin(dir: &Path) -> Option<Toolchain> {
    let gxx = tool_in(dir, "g++")?;
    let gcc = tool_in(dir, "gcc")?;
    let ver = version_line(&gxx).unwrap_or_else(|| "g++".into());
    Some(Toolchain {
        gcc,
        gxx,
        bin_dir: Some(dir.to_path_buf()),
        label: format!("{}  [{}]", ver, dir.display()),
        major: gcc_major(&ver),
    })
}

/// Accepts either the Dev-C++ install folder or the compiler's bin folder.
pub fn from_root(root: &Path) -> Option<Toolchain> {
    for sub in ["", "bin", "MinGW64/bin", "MinGW32/bin", "MinGW/bin"] {
        let d = if sub.is_empty() { root.to_path_buf() } else { root.join(sub) };
        if let Some(t) = from_bin(&d) {
            return Some(t);
        }
    }
    None
}

pub fn detect(user_dir: &str) -> Option<Toolchain> {
    if !user_dir.trim().is_empty() {
        if let Some(t) = from_root(Path::new(user_dir.trim())) {
            return Some(t);
        }
    }
    if let Ok(v) = std::env::var("DIADOCHI_DEVCPP") {
        if let Some(t) = from_root(Path::new(&v)) {
            return Some(t);
        }
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    for var in ["ProgramFiles(x86)", "ProgramFiles", "ProgramW6432"] {
        if let Ok(base) = std::env::var(var) {
            roots.push(PathBuf::from(base).join("Dev-Cpp"));
        }
    }
    roots.push(PathBuf::from(r"C:\Dev-Cpp"));
    roots.push(PathBuf::from(r"D:\Dev-Cpp"));
    for r in roots {
        if let Some(t) = from_root(&r) {
            return Some(t);
        }
    }
    // Fallback so the app is still usable (and testable) without Dev-C++.
    let gxx = PathBuf::from("g++");
    version_line(&gxx).map(|v| Toolchain {
        gcc: PathBuf::from("gcc"),
        gxx,
        bin_dir: None,
        major: gcc_major(&v),
        label: format!("{}  [from PATH - Dev-C++ not found]", v),
    })
}

pub fn build(tc: &Toolchain, src: &Path, lang: Lang, o: &BuildOptions) -> BuildResult {
    let exe = src.with_extension(if cfg!(windows) { "exe" } else { "out" });
    let compiler = if lang == Lang::C { &tc.gcc } else { &tc.gxx };

    let mut args: Vec<String> = ["-Wall", "-Wextra", "-Wpedantic", "-Wshadow", "-Wformat=2", "-Wcast-align"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    if tc.major >= 6 {
        args.push("-Wnull-dereference".into()); // GCC 6+; Dev-C++ 5.11 ships GCC 4.9.2
    }
    if o.deep {
        // flow-based warnings (-Wnull-dereference, -Warray-bounds, -Wmaybe-uninitialized) only fire with optimization
        args.push("-O2".into());
        args.push("-D_FORTIFY_SOURCE=2".into());
    }
    if o.strict {
        // only flags that exist in the TDM-GCC 4.9 shipped with Dev-C++ 5.11
        args.push("-Werror=return-type".into());
        args.push("-Werror=format-security".into());
        if lang == Lang::C {
            args.push("-Werror=implicit-function-declaration".into());
        }
    }
    if o.p10 {
        // Power of 10 rule 10: most pedantic warnings, and zero warnings allowed
        for f in ["-Werror", "-Wconversion", "-Wundef", "-Wswitch-default"] {
            args.push(f.into());
        }
    }
    if o.stack_protector {
        args.push("-fstack-protector-strong".into());
    }
    args.push(format!("-std={}", o.std_flag));
    args.push("-static-libgcc".into());
    if lang == Lang::Cpp {
        args.push("-static-libstdc++".into());
    }
    let src_name = src.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let exe_name = exe.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    args.push(src_name.clone());
    args.push("-o".into());
    args.push(exe_name);

    let command = format!("{} {}", compiler.display(), args.join(" "));
    let mut cmd = Command::new(compiler);
    cmd.args(&args);
    if let Some(dir) = src.parent() {
        cmd.current_dir(dir);
    }
    if let Some(bin) = &tc.bin_dir {
        // cc1plus and its DLLs live next to g++, so the folder must be on PATH
        let mut paths = vec![bin.clone()];
        if let Some(existing) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&existing));
        }
        if let Ok(joined) = std::env::join_paths(paths) {
            cmd.env("PATH", joined);
        }
    }
    quiet(&mut cmd);

    match cmd.output() {
        Ok(out) => {
            let raw = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            let diagnostics = parse_gcc(&raw, &src_name);
            BuildResult { ok: out.status.success(), command, raw, diagnostics, exe }
        }
        Err(e) => BuildResult {
            ok: false,
            command,
            raw: format!("could not start the compiler: {e}"),
            diagnostics: Vec::new(),
            exe,
        },
    }
}

fn parse_gcc(raw: &str, src_name: &str) -> Vec<Diagnostic> {
    let re = Regex::new(r"^(.*?):(\d+):(?:(\d+):)?\s*(fatal error|error|warning|note):\s*(.*)$").unwrap();
    let mut out = Vec::new();
    for line in raw.lines() {
        let Some(c) = re.captures(line) else { continue };
        let file = Path::new(&c[1]).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
        if file != src_name {
            continue; // diagnostics inside headers have no meaningful line in this editor
        }
        let severity = match &c[4] {
            "warning" => Severity::Warning,
            "note" => Severity::Note,
            _ => Severity::Error,
        };
        let line_no: usize = c[2].parse().unwrap_or(1);
        let col: usize = c.get(3).and_then(|m| m.as_str().parse().ok()).unwrap_or(1);
        out.push(Diagnostic {
            severity,
            code: "GCC".into(),
            message: c[5].to_string(),
            line: line_no.max(1),
            col_start: col.saturating_sub(1),
            col_end: col,
            help: None,
            waived_reason: None,
        });
    }
    out
}
