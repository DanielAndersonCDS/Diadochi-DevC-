//! Diadochi safety rules: a Rust-style "the compiler says no" layer for C and C++.
//!
//! This is a heuristic, line- and block-aware analyzer, not a real borrow checker.
//! It masks comments/strings, finds function bodies, and applies rules for the
//! classic memory-safety mistakes (overflows, leaks, use-after-free, ...).
//! Like Rust's `unsafe`, any finding can be waived ON PURPOSE with a reason:
//!     // diadochi:allow(DR002) name is capped at 7 chars by the caller

use regex::Regex;
use std::collections::HashMap;
use std::sync::OnceLock;

macro_rules! rx {
    ($pat:expr) => {{
        static CELL: OnceLock<Regex> = OnceLock::new();
        CELL.get_or_init(|| Regex::new($pat).expect("valid regex"))
    }};
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    C,
    Cpp,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Severity {
    Error,
    Warning,
    Note,
    Waived,
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: String,
    pub message: String,
    pub line: usize,      // 1-based
    pub col_start: usize, // 0-based, in chars
    pub col_end: usize,
    pub help: Option<String>,
    pub waived_reason: Option<String>,
}

pub const RULE_DOCS: &[(&str, &str)] = &[
    ("DR000", "A waiver comment must give a reason (like a SAFETY comment on unsafe)"),
    ("DR001", "gets(): can never be used safely"),
    ("DR002", "strcpy/strcat: destination size unknown, buffer overflow"),
    ("DR003", "sprintf/vsprintf: unbounded write"),
    ("DR004", "scanf %s / %[ without a width: buffer overflow"),
    ("DR005", "atoi/atof: cannot report bad input"),
    ("DR006", "system(): shell injection, non-portable"),
    ("DR007", "goto: skips initialization and cleanup"),
    ("DR008", "printf(variable): format-string vulnerability"),
    ("DR009", "void main: non-standard"),
    ("DR010", "fflush(stdin): undefined behaviour"),
    ("DR011", "string literal assigned to non-const char*"),
    ("DR012", "C++: raw new without an owner"),
    ("DR013", "C++: manual delete"),
    ("DR014", "C++: C-style cast"),
    ("DR015", "mutable global variable"),
    ("DR016", "alloca(): unchecked stack allocation"),
    ("DR017", "strncpy(): may leave the string unterminated"),
    ("DR101", "malloc/calloc/fopen result used without a NULL check"),
    ("DR102", "p = realloc(p, n): leaks the block if realloc fails"),
    ("DR103", "allocation never released and never handed off: memory leak"),
    ("DR104", "allocated with one API, released with another (malloc/free vs new/delete vs new[]/delete[])"),
    ("DR105", "double free"),
    ("DR106", "use after free"),
    ("DR107", "returning a pointer to a local variable"),
    ("DR108", "local variable read before it is assigned"),
    ("DR109", "constant array index out of bounds"),
    ("DR110", "off-by-one loop bound: i <= SIZE indexing an array of SIZE"),
    ("P10-1", "Power of 10 #1: no goto/setjmp/longjmp, no recursion"),
    ("P10-2", "Power of 10 #2: every loop needs a provable fixed upper bound"),
    ("P10-3", "Power of 10 #3: no dynamic allocation after initialization"),
    ("P10-4", "Power of 10 #4: functions of at most 60 lines"),
    ("P10-5", "Power of 10 #5: at least 2 assertions per function"),
    ("P10-8", "Power of 10 #8: limited preprocessor (no ##, no variadic macros)"),
    ("P10-9", "Power of 10 #9: at most one level of dereference, no function pointers"),
];

// ───────────────────────── helpers ─────────────────────────

fn col_of(line: &str, byte: usize) -> usize {
    line.get(..byte).map_or(0, |s| s.chars().count())
}

#[allow(clippy::too_many_arguments)]
fn push(
    out: &mut Vec<Diagnostic>,
    sev: Severity,
    code: &str,
    msg: String,
    line0: usize,
    cs: usize,
    ce: usize,
    help: Option<String>,
) {
    out.push(Diagnostic {
        severity: sev,
        code: code.to_string(),
        message: msg,
        line: line0 + 1,
        col_start: cs,
        col_end: ce.max(cs + 1),
        help,
        waived_reason: None,
    });
}

/// Regex matching `name` as a whole identifier that is not a member access (`a.name`, `a->name`).
/// Group 1 is the name itself.
fn word_use(name: &str) -> Regex {
    Regex::new(&format!(r"(?:^|[^\w.>])({})\b", regex::escape(name))).unwrap()
}

/// Replace comments and string/char literal contents with spaces (keeping quotes,
/// newlines and the char count) so rules never match inside them.
pub fn mask(src: &str) -> Vec<String> {
    #[derive(Clone, Copy)]
    enum S {
        Code,
        Line,
        Block,
        Str,
        Chr,
    }
    let ch: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len());
    let mut s = S::Code;
    let mut i = 0;
    while i < ch.len() {
        let c = ch[i];
        let n = ch.get(i + 1).copied().unwrap_or('\0');
        let cur = s;
        match cur {
            S::Code => {
                if c == '/' && n == '/' {
                    s = S::Line;
                    out.push_str("  ");
                    i += 2;
                    continue;
                }
                if c == '/' && n == '*' {
                    s = S::Block;
                    out.push_str("  ");
                    i += 2;
                    continue;
                }
                if c == '"' {
                    s = S::Str;
                }
                if c == '\'' {
                    s = S::Chr;
                }
                out.push(c);
            }
            S::Line => {
                if c == '\n' {
                    s = S::Code;
                    out.push('\n');
                } else {
                    out.push(' ');
                }
            }
            S::Block => {
                if c == '*' && n == '/' {
                    s = S::Code;
                    out.push_str("  ");
                    i += 2;
                    continue;
                }
                out.push(if c == '\n' { '\n' } else { ' ' });
            }
            S::Str | S::Chr => {
                let quote = if matches!(cur, S::Str) { '"' } else { '\'' };
                if c == '\\' {
                    out.push(' ');
                    if n == '\n' {
                        out.push('\n');
                    } else if n != '\0' {
                        out.push(' ');
                    }
                    i += 2;
                    continue;
                }
                if c == quote {
                    s = S::Code;
                    out.push(c);
                } else if c == '\n' {
                    s = S::Code;
                    out.push('\n');
                } else {
                    out.push(' ');
                }
            }
        }
        i += 1;
    }
    out.split('\n').map(str::to_string).collect()
}

// ───────────────────────── layout: depth + function spans ─────────────────────────

#[derive(Clone, Copy)]
enum Kind {
    Fn,
    Ns,
    Class,
    Block,
}

struct FnSpan {
    start: usize,
    end: usize,
    header: String,
}

struct Layout {
    depth_start: Vec<usize>,
    top_level: Vec<bool>,
    functions: Vec<FnSpan>,
}

fn classify(header: &str, stack: &[Kind]) -> Kind {
    if stack.iter().any(|k| matches!(k, Kind::Fn)) {
        return Kind::Block;
    }
    let h = header.trim();
    let words: Vec<&str> = h
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|t| !t.is_empty())
        .collect();
    let has = |w: &str| words.iter().any(|t| *t == w);
    if has("namespace") || (has("extern") && !h.contains(')')) {
        return Kind::Ns;
    }
    if (has("struct") || has("class") || has("enum") || has("union")) && !h.contains(')') {
        return Kind::Class;
    }
    let first = words.first().copied().unwrap_or("");
    let control = matches!(first, "if" | "for" | "while" | "switch" | "else" | "do" | "catch" | "try");
    if h.contains(')') && !control {
        return Kind::Fn;
    }
    Kind::Block
}

fn scan_layout(clean: &[String]) -> Layout {
    let mut stack: Vec<Kind> = Vec::new();
    let mut open_fns: Vec<(usize, String)> = Vec::new();
    let mut header = String::new();
    let mut lay = Layout { depth_start: Vec::new(), top_level: Vec::new(), functions: Vec::new() };

    for (i, line) in clean.iter().enumerate() {
        lay.depth_start.push(stack.len());
        lay.top_level.push(stack.iter().all(|k| matches!(k, Kind::Ns)));
        if line.trim_start().starts_with('#') {
            header.clear();
            continue;
        }
        for c in line.chars() {
            match c {
                ';' => header.clear(),
                '{' => {
                    let kind = classify(&header, &stack);
                    if matches!(kind, Kind::Fn) {
                        open_fns.push((i, header.clone()));
                    }
                    stack.push(kind);
                    header.clear();
                }
                '}' => {
                    if let Some(k) = stack.pop() {
                        if matches!(k, Kind::Fn) {
                            if let Some((start, hdr)) = open_fns.pop() {
                                lay.functions.push(FnSpan { start, end: i, header: hdr });
                            }
                        }
                    }
                    header.clear();
                }
                _ => header.push(c),
            }
        }
        header.push(' ');
    }
    // A function still open at end of file (the user is mid-typing): analyze it anyway.
    while let Some((start, hdr)) = open_fns.pop() {
        lay.functions.push(FnSpan { start, end: clean.len().saturating_sub(1), header: hdr });
    }
    lay
}

// ───────────────────────── simple per-line rules ─────────────────────────

struct LineRule {
    code: &'static str,
    sev: Severity,
    re: Regex,
    msg: &'static str,
    help: &'static str,
    cpp_only: bool,
}

fn line_rule_table() -> &'static Vec<LineRule> {
    static T: OnceLock<Vec<LineRule>> = OnceLock::new();
    T.get_or_init(|| {
        use Severity::*;
        let defs: &[(&str, Severity, &str, &str, &str, bool)] = &[
            ("DR001", Error, r"\b(gets)\s*\(",
             "gets() can never be used safely: it has no way to limit how much it reads",
             "use fgets(buf, sizeof buf, stdin)", false),
            ("DR002", Error, r"\b(strcpy|strcat|wcscpy|wcscat|stpcpy)\s*\(",
             "{}() does not know how big the destination buffer is, so a long source overflows it",
             "use snprintf(dst, sizeof dst, \"%s\", src), or std::string in C++", false),
            ("DR003", Error, r"\b(sprintf|vsprintf)\s*\(",
             "{}() can write past the end of the buffer",
             "use snprintf / vsnprintf with sizeof the buffer", false),
            ("DR005", Warning, r"\b(atoi|atol|atoll|atof)\s*\(",
             "{}() cannot report bad input: it silently returns 0",
             "use strtol / strtod and check endptr and errno (std::stoi in C++)", false),
            ("DR006", Warning, r"\b(system)\s*\(",
             "system() hands a string to the shell: injection risk and non-portable",
             "call the OS API or the program directly", false),
            ("DR007", Warning, r"\b(goto)\b",
             "goto jumps over initialization and cleanup, a classic source of leaks",
             "use structured control flow, or one cleanup block at the end of the function", false),
            ("DR008", Error, r"\bprintf\s*\(\s*([A-Za-z_]\w*)\s*\)",
             "'{}' is used as the format string: a % in it is a format-string vulnerability",
             "write printf(\"%s\", {})", false),
            ("DR009", Warning, r"\bvoid\s+(main)\s*\(",
             "main must return int in standard C and C++",
             "write int main(void) in C, int main() in C++", false),
            ("DR010", Warning, r"\bfflush\s*\(\s*(stdin)\s*\)",
             "fflush(stdin) is undefined behaviour",
             "read and discard input up to the newline instead", false),
            ("DR011", Warning, r#"\bchar\s*\*\s*\w+\s*=\s*(")"#,
             "string literals are read-only: writing through this pointer is undefined behaviour",
             "declare it const char *, or use an array: char s[] = \"...\"", false),
            ("DR012", Warning, r"(?:^|[^\w:])(new)\s+[A-Za-z_]",
             "raw 'new': nothing guarantees a matching delete on every path",
             "use std::make_unique<T>(), std::vector or std::string (ownership, like Rust's Box)", true),
            ("DR013", Warning, r"\b(delete)\b\s*(?:\[\s*\]\s*)?[A-Za-z_*(]",
             "manual 'delete' is easy to forget, repeat, or use after",
             "let a smart pointer or container own the memory", true),
            ("DR014", Warning,
             r"(?:^|[^A-Za-z0-9_])\(\s*((?:const\s+)?(?:(?:unsigned|signed)\s+)?(?:int|char|long|short|float|double|size_t|u?int\d+_t)\s*\**|void\s*\*+)\s*\)\s*[A-Za-z_(&*]",
             "C-style cast silently picks the most dangerous conversion",
             "use static_cast<T>(x); reinterpret_cast makes unsafe intent greppable", true),
            ("DR016", Warning, r"\b(alloca)\s*\(",
             "alloca() has no failure path: a big size just crashes the stack",
             "use a fixed-size array, or heap allocation with a check", false),
            ("DR017", Warning, r"\b(strncpy)\s*\(",
             "strncpy() does not NUL-terminate when the source fills the buffer",
             "terminate by hand (dst[n-1] = '\\0') or use snprintf", false),
        ];
        defs.iter()
            .map(|&(code, sev, pat, msg, help, cpp_only)| LineRule {
                code,
                sev,
                re: Regex::new(pat).unwrap(),
                msg,
                help,
                cpp_only,
            })
            .collect()
    })
}

fn line_rules(clean: &[String], lang: Lang, out: &mut Vec<Diagnostic>) {
    for (i, line) in clean.iter().enumerate() {
        if line.trim_start().starts_with('#') {
            continue;
        }
        for r in line_rule_table() {
            if r.cpp_only && lang != Lang::Cpp {
                continue;
            }
            for caps in r.re.captures_iter(line) {
                let whole = caps.get(0).unwrap();
                let m = caps.get(1).unwrap_or(whole);
                if r.code == "DR011" && line.contains("const") {
                    continue;
                }
                if r.code == "DR012" && line.contains("operator") {
                    continue;
                }
                if r.code == "DR014" && line[..whole.start()].trim_end().ends_with("sizeof") {
                    continue;
                }
                push(
                    out,
                    r.sev,
                    r.code,
                    r.msg.replace("{}", m.as_str()),
                    i,
                    col_of(line, m.start()),
                    col_of(line, m.end()),
                    Some(r.help.replace("{}", m.as_str())),
                );
            }
        }
    }
}

/// scanf("%s") without a width: needs the raw line because masking blanks the format string.
fn scanf_rule(raw: &[&str], clean: &[String], out: &mut Vec<Diagnostic>) {
    let call = rx!(r"\b(?:f|s)?scanf\s*\(");
    let spec = rx!(r"%(\*?)(s|\[)");
    for (i, line) in clean.iter().enumerate() {
        if !call.is_match(line) {
            continue;
        }
        let Some(rawl) = raw.get(i) else { continue };
        for c in spec.captures_iter(rawl) {
            if !c[1].is_empty() {
                continue; // %*s stores nothing
            }
            let m = c.get(0).unwrap();
            push(
                out,
                Severity::Error,
                "DR004",
                "scanf conversion without a width reads until whitespace, with no limit: buffer overflow".into(),
                i,
                col_of(rawl, m.start()),
                col_of(rawl, m.end()),
                Some("give a width one less than the buffer: char name[32]; scanf(\"%31s\", name);".into()),
            );
        }
    }
}

fn globals(clean: &[String], lay: &Layout, out: &mut Vec<Diagnostic>) {
    let re = rx!(r"^\s*(?:static\s+)?(?:(?:unsigned|signed|long|short)\s+)*(?:int|long|short|char|float|double|size_t|bool)(?:\s*[\*&]+\s*|\s+)([A-Za-z_]\w*)\s*(?:\[[^\]]*\])?\s*(?:=[^;]*)?;");
    for (i, line) in clean.iter().enumerate() {
        if !lay.top_level[i] || line.contains("const") || line.contains("typedef") {
            continue;
        }
        if let Some(c) = re.captures(line) {
            let m = c.get(1).unwrap();
            push(
                out,
                Severity::Warning,
                "DR015",
                format!("mutable global '{}': any function can change it at any time", m.as_str()),
                i,
                col_of(line, m.start()),
                col_of(line, m.end()),
                Some("pass it as a parameter, or make it const; Rust only allows this behind unsafe".into()),
            );
        }
    }
}

// ───────────────────────── per-function rules ─────────────────────────

const READERS: &[&str] = &[
    "printf", "fprintf", "puts", "fputs", "putchar", "strlen", "strcmp", "strncmp", "strcpy",
    "strncpy", "strcat", "strncat", "memset", "memcpy", "memmove", "memcmp", "scanf", "fscanf",
    "sscanf", "fgets", "fread", "fwrite", "sprintf", "snprintf", "free", "fclose", "if", "while",
    "for", "switch", "sizeof", "assert", "strchr", "strstr", "fseek", "ftell", "rewind", "fgetc",
    "getc", "fputc", "putc", "feof", "ferror", "fflush", "perror", "delete", "return", "cout",
];

fn calls_with_arg(line: &str, name: &str) -> Vec<String> {
    let call = rx!(r"([A-Za-z_]\w*)\s*\(");
    let word = word_use(name);
    let mut res = Vec::new();
    for m in call.captures_iter(line) {
        let open = m.get(0).unwrap().end();
        let mut depth = 1;
        let mut end = line.len();
        for (off, ch) in line[open..].char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = open + off;
                        break;
                    }
                }
                _ => {}
            }
        }
        if word.is_match(&line[open..end]) {
            res.push(m[1].to_string());
        }
    }
    res
}

/// Does ownership of `name` visibly leave the function (returned, stored, or passed on)?
/// Passing a pointer to your own function counts as a "move", like in Rust.
fn escapes(name: &str, rest: &[&String]) -> bool {
    let n = regex::escape(name);
    let ret = Regex::new(&format!(r"\breturn\b[^;]*\b{n}\b")).unwrap();
    let store = Regex::new(&format!(r"[\w\]\)>\.\-]\s*=\s*{n}\s*;")).unwrap();
    let word = word_use(name);
    for line in rest {
        if !word.is_match(line) {
            continue;
        }
        if ret.is_match(line) || store.is_match(line) {
            return true;
        }
        if calls_with_arg(line, name).iter().any(|c| !READERS.contains(&c.as_str())) {
            return true;
        }
    }
    false
}

#[derive(Clone, Copy, PartialEq)]
enum AllocKind {
    Malloc,
    File,
    New,
    NewArr,
}

fn check_allocations(clean: &[String], s: usize, e: usize, lang: Lang, out: &mut Vec<Diagnostic>) {
    let c_alloc = rx!(r"(?:^|[^\w.>])([A-Za-z_]\w*)\s*=\s*(?:\(\s*[\w\s\*:<>]+\)\s*)?(malloc|calloc|strdup|_strdup|fopen)\s*\(");
    let realloc = rx!(r"(?:^|[^\w.>])([A-Za-z_]\w*)\s*=\s*(?:\(\s*[\w\s\*:<>]+\)\s*)?realloc\s*\(\s*([A-Za-z_]\w*)\s*,");
    let new_arr = rx!(r"(?:^|[^\w.>])([A-Za-z_]\w*)\s*=\s*new\s+[^;(]*?\[");
    let new_one = rx!(r"(?:^|[^\w.>])([A-Za-z_]\w*)\s*=\s*new\s+");
    let guard_on_line = rx!(r"\b(?:if|while)\b|\?");
    let mut seen: Vec<String> = Vec::new();

    for i in s..=e {
        let line = &clean[i];

        if let Some(c) = realloc.captures(line) {
            if c[1] == c[2] {
                let m = c.get(1).unwrap();
                push(
                    out,
                    Severity::Warning,
                    "DR102",
                    format!("'{0} = realloc({0}, ...)' leaks the old block if realloc fails", &c[1]),
                    i,
                    col_of(line, m.start()),
                    col_of(line, m.end()),
                    Some("assign to a temporary, check it, then copy it into the original pointer".into()),
                );
            }
        }

        // (name, kind, callee, start, end)
        let found: Option<(String, AllocKind, String, usize, usize)> = if let Some(c) = c_alloc.captures(line) {
            let kind = if &c[2] == "fopen" { AllocKind::File } else { AllocKind::Malloc };
            let m = c.get(1).unwrap();
            Some((m.as_str().to_string(), kind, c[2].to_string(), m.start(), m.end()))
        } else if lang == Lang::Cpp {
            if let Some(c) = new_arr.captures(line) {
                let m = c.get(1).unwrap();
                Some((m.as_str().to_string(), AllocKind::NewArr, "new[]".into(), m.start(), m.end()))
            } else if let Some(c) = new_one.captures(line) {
                let m = c.get(1).unwrap();
                Some((m.as_str().to_string(), AllocKind::New, "new".into(), m.start(), m.end()))
            } else {
                None
            }
        } else {
            None
        };
        let Some((name, kind, callee, cs, ce)) = found else { continue };
        if seen.contains(&name) {
            continue;
        }
        seen.push(name.clone());
        let n = regex::escape(&name);

        // DR101: NULL check within the next few lines
        if matches!(kind, AllocKind::Malloc | AllocKind::File) {
            let chk = Regex::new(&format!(
                r"(?:^|[^\w.>])!\s*{n}\b|\b(?:if|while|assert)\s*\(\s*!?\s*{n}\b|\b{n}\s*[!=]=\s*(?:NULL|nullptr|0)\b|\b(?:NULL|nullptr|0)\s*[!=]=\s*{n}\b"
            ))
            .unwrap();
            let lim = (i + 4).min(e);
            let checked = guard_on_line.is_match(line) || (i..=lim).any(|k| chk.is_match(&clean[k]));
            if !checked {
                push(
                    out,
                    Severity::Error,
                    "DR101",
                    format!("{}() can return NULL and '{}' is used without checking", callee, name),
                    i,
                    cs,
                    ce,
                    Some(format!("check right away: if (!{}) {{ /* handle the failure */ }}", name)),
                );
            }
        }

        // Release / mismatch / leak
        let (release, wrong, how) = match kind {
            AllocKind::Malloc => (
                format!(r"\bfree\s*\(\s*(?:\(\s*[\w\s\*]+\)\s*)?{n}\s*\)"),
                format!(r"\bdelete\b[^;]*\b{n}\b"),
                "free()",
            ),
            AllocKind::File => (
                format!(r"\bfclose\s*\(\s*{n}\s*\)"),
                format!(r"\b(?:free|delete)\b[^;]*\b{n}\b"),
                "fclose()",
            ),
            AllocKind::New => (
                format!(r"\bdelete\s+{n}\b"),
                format!(r"\bdelete\s*\[\s*\]\s*{n}\b|\bfree\s*\(\s*{n}\b"),
                "delete",
            ),
            AllocKind::NewArr => (
                format!(r"\bdelete\s*\[\s*\]\s*{n}\b"),
                format!(r"\bdelete\s+{n}\b|\bfree\s*\(\s*{n}\b"),
                "delete[]",
            ),
        };
        let release = Regex::new(&release).unwrap();
        let wrong = Regex::new(&wrong).unwrap();
        let mut released = false;
        let mut wrong_hit: Option<usize> = None;
        for k in (i + 1)..=e {
            if release.is_match(&clean[k]) {
                released = true;
            }
            if wrong_hit.is_none() && wrong.is_match(&clean[k]) {
                wrong_hit = Some(k);
            }
        }
        if let Some(k) = wrong_hit {
            push(
                out,
                Severity::Error,
                "DR104",
                format!("'{}' came from {} but is released with the wrong API: undefined behaviour", name, callee),
                k,
                0,
                clean[k].len(),
                Some(format!("memory from {} must be released with {}", callee, how)),
            );
        } else if !released {
            let rest: Vec<&String> = clean.iter().skip(i + 1).take(e - i).collect();
            if !escapes(&name, &rest) {
                let help = match kind {
                    AllocKind::Malloc => format!("call free({0}) on every path before returning, or return/store {0} to hand ownership on", name),
                    AllocKind::File => format!("call fclose({}) on every path before returning", name),
                    _ => "use std::unique_ptr / std::vector so the memory frees itself".to_string(),
                };
                push(
                    out,
                    Severity::Error,
                    "DR103",
                    format!("'{}' is never released in this function and never leaves it (not returned, stored or passed on): leak", name),
                    i,
                    cs,
                    ce,
                    Some(help),
                );
            }
        }
    }
}

fn check_free_flow(clean: &[String], lay: &Layout, s: usize, e: usize, out: &mut Vec<Diagnostic>) {
    let rel = rx!(r"\b(?:free|fclose)\s*\(\s*(?:\(\s*[\w\s\*]+\)\s*)?([A-Za-z_]\w*)\s*\)|\bdelete\s*(?:\[\s*\]\s*)?([A-Za-z_]\w*)\s*;");
    for j in s..=e {
        for caps in rel.captures_iter(&clean[j]) {
            let Some(m) = caps.get(1).or_else(|| caps.get(2)) else { continue };
            let name = m.as_str();
            if name == "NULL" {
                continue;
            }
            let d0 = lay.depth_start[j];
            let word = word_use(name);
            let assign = Regex::new(&format!(r"^\s*{}\s*=([^=].*)?$", regex::escape(name))).unwrap();

            for k in (j + 1)..=e {
                let ln = &clean[k];
                let dk = lay.depth_start[k];
                let closes = ln.trim_start().starts_with('}');
                if dk < d0 || (closes && dk <= d0) {
                    break; // left the block that did the free
                }
                if let Some(c) = assign.captures(ln) {
                    // `p = NULL;` or `p = malloc(..)` re-arms the pointer; `p = p->next` does not
                    if let Some(rhs) = c.get(1) {
                        if let Some(u) = word.captures(rhs.as_str()) {
                            let g = u.get(1).unwrap();
                            let base = rhs.start();
                            push(
                                out,
                                Severity::Error,
                                "DR106",
                                format!("'{}' is read after it was freed (line {}): the pointer dangles", name, j + 1),
                                k,
                                col_of(ln, base + g.start()),
                                col_of(ln, base + g.end()),
                                Some("read what you need before freeing, e.g. save p->next in a temporary first".into()),
                            );
                        }
                    }
                    break;
                }
                let twice = rel
                    .captures_iter(ln)
                    .any(|c2| c2.get(1).or_else(|| c2.get(2)).map_or(false, |g| g.as_str() == name));
                if twice {
                    push(
                        out,
                        Severity::Error,
                        "DR105",
                        format!("'{}' is freed twice (first at line {}): double free corrupts the heap", name, j + 1),
                        k,
                        0,
                        ln.len(),
                        Some(format!("set {} = NULL right after freeing it", name)),
                    );
                    break;
                }
                if let Some(c) = word.captures(ln) {
                    let g = c.get(1).unwrap();
                    push(
                        out,
                        Severity::Error,
                        "DR106",
                        format!("'{}' is used after it was freed (line {}): the pointer dangles", name, j + 1),
                        k,
                        col_of(ln, g.start()),
                        col_of(ln, g.end()),
                        Some(format!("don't touch {0} after the free, or re-assign it first", name)),
                    );
                    break;
                }
                if ln.contains("return") || ln.contains("break") || ln.contains("continue") || ln.contains("goto") {
                    break;
                }
            }
        }
    }
}

fn check_returns(clean: &[String], s: usize, e: usize, ret_is_ptr: bool, out: &mut Vec<Diagnostic>) {
    if !ret_is_ptr {
        return;
    }
    let decl = rx!(r"^\s*(?:const\s+)?(?:(?:unsigned|signed|long|short)\s+)*(?:int|long|short|char|float|double|bool|size_t|u?int\d+_t|std::string|string)\s+([A-Za-z_]\w*)\s*(\[[^\]]*\])?\s*(?:=|;|,|\[)");
    let ret_addr = rx!(r"\breturn\s*&\s*([A-Za-z_]\w*)\s*;");
    let ret_name = rx!(r"\breturn\s+([A-Za-z_]\w*)\s*;");
    let ret_cstr = rx!(r"\breturn\s+([A-Za-z_]\w*)\s*\.\s*(?:c_str|data)\s*\(\s*\)");

    // name -> (is_array, is_std_string)
    let mut locals: HashMap<String, (bool, bool)> = HashMap::new();
    for i in s..=e {
        let line = &clean[i];
        if line.contains("static") {
            continue;
        }
        if let Some(c) = decl.captures(line) {
            locals.insert(c[1].to_string(), (c.get(2).is_some(), line.contains("string")));
        }
    }
    let help = "return a heap copy (malloc/strdup), fill a caller-supplied buffer, or return std::string by value";
    for i in s..=e {
        let line = &clean[i];
        let mut report = |cap: regex::Captures, why: &str| {
            let m = cap.get(1).unwrap();
            push(
                out,
                Severity::Error,
                "DR107",
                format!("returns a pointer into local '{}' ({}): it no longer exists after the function returns", m.as_str(), why),
                i,
                col_of(line, m.start()),
                col_of(line, m.end()),
                Some(help.to_string()),
            );
        };
        if let Some(c) = ret_addr.captures(line) {
            if locals.contains_key(&c[1]) {
                report(c, "address of a local");
                continue;
            }
        }
        if let Some(c) = ret_name.captures(line) {
            if locals.get(&c[1]).map_or(false, |l| l.0) {
                report(c, "a local array decays to a pointer");
                continue;
            }
        }
        if let Some(c) = ret_cstr.captures(line) {
            if locals.get(&c[1]).map_or(false, |l| l.1) {
                report(c, "the buffer dies with the string");
            }
        }
    }
}

fn check_uninit(clean: &[String], s: usize, e: usize, out: &mut Vec<Diagnostic>) {
    let decl = rx!(r"^\s*(?:(?:unsigned|signed|long|short)\s+)*(?:int|long|short|char|float|double|bool|size_t)\s+([A-Za-z_]\w*)\s*;");
    for i in (s + 1)..=e {
        let Some(c) = decl.captures(&clean[i]) else { continue };
        let name = c[1].to_string();
        let n = regex::escape(&name);
        let word = word_use(&name);
        let write = Regex::new(&format!(r"(?:^|[^\w.>]){n}\s*=(?:[^=]|$)")).unwrap();
        let addr = Regex::new(&format!(r"&\s*{n}\b|>>\s*{n}\b")).unwrap();
        for k in (i + 1)..=e {
            let ln = &clean[k];
            if !word.is_match(ln) {
                continue;
            }
            // first mention decides: is it a write, or a read of garbage?
            if addr.is_match(ln) {
                break;
            }
            let is_read = match write.find(ln) {
                Some(w) => word.is_match(&ln[w.end()..]),
                None => true,
            };
            if is_read {
                let g = word.captures(ln).unwrap().get(1).unwrap();
                push(
                    out,
                    Severity::Error,
                    "DR108",
                    format!("'{}' is read before it is ever assigned (declared at line {} with no value)", name, i + 1),
                    k,
                    col_of(ln, g.start()),
                    col_of(ln, g.end()),
                    Some(format!("initialize where you declare it: int {} = 0;", name)),
                );
            }
            break;
        }
    }
}

fn collect_arrays(lines: &[String], range: std::ops::RangeInclusive<usize>, only_top: Option<&Layout>, map: &mut HashMap<String, (usize, usize)>) {
    let decl = rx!(r"\b(?:int|long|short|char|float|double|bool|size_t|\w+_t)\s+([A-Za-z_]\w*)\s*\[\s*(\d+)\s*\]");
    for i in range {
        if let Some(l) = only_top {
            if !l.top_level[i] {
                continue;
            }
        }
        for c in decl.captures_iter(&lines[i]) {
            if let Ok(size) = c[2].parse::<usize>() {
                map.insert(c[1].to_string(), (size, i));
            }
        }
    }
}

fn check_arrays(
    clean: &[String],
    global_arrays: &HashMap<String, (usize, usize)>,
    s: usize,
    e: usize,
    out: &mut Vec<Diagnostic>,
) {
    let mut sizes = global_arrays.clone();
    collect_arrays(clean, s..=e, None, &mut sizes);
    let idx = rx!(r"(?:^|[^\w.>])([A-Za-z_]\w*)\s*\[\s*(-?\d+)\s*\]");
    let forle = rx!(r"\bfor\s*\([^;]*;\s*([A-Za-z_]\w*)\s*<=\s*(\d+)\s*;");

    for i in s..=e {
        let line = &clean[i];
        for c in idx.captures_iter(line) {
            let Some(&(size, decl_line)) = sizes.get(&c[1]) else { continue };
            if i == decl_line {
                continue;
            }
            let v: i64 = c[2].parse().unwrap_or(0);
            if v < 0 || (v as usize) >= size {
                let m = c.get(0).unwrap();
                push(
                    out,
                    Severity::Error,
                    "DR109",
                    format!("index {} is out of bounds for '{}' (valid indices are 0..{})", v, &c[1], size.saturating_sub(1)),
                    i,
                    col_of(line, m.start()),
                    col_of(line, m.end()),
                    Some("C does not check this at runtime: the write lands on whatever sits next to the array".into()),
                );
            }
        }
        if let Some(c) = forle.captures(line) {
            let var = c[1].to_string();
            let limit: usize = c[2].parse().unwrap_or(usize::MAX);
            for (arr, &(size, _)) in &sizes {
                if size != limit {
                    continue;
                }
                let acc = Regex::new(&format!(r"\b{}\s*\[\s*{}\s*\]", regex::escape(arr), regex::escape(&var))).unwrap();
                if (i..=(i + 6).min(e)).any(|k| acc.is_match(&clean[k])) {
                    let m = c.get(0).unwrap();
                    push(
                        out,
                        Severity::Error,
                        "DR110",
                        format!("loop runs while {0} <= {1}, but '{2}' only has indices 0..{3}: off-by-one overflow", var, limit, arr, limit.saturating_sub(1)),
                        i,
                        col_of(line, m.start()),
                        col_of(line, m.end()),
                        Some(format!("use {} < {}", var, limit)),
                    );
                }
            }
        }
    }
}

fn check_function(
    clean: &[String],
    lay: &Layout,
    global_arrays: &HashMap<String, (usize, usize)>,
    f: &FnSpan,
    lang: Lang,
    out: &mut Vec<Diagnostic>,
) {
    let (s, e) = (f.start, f.end.min(clean.len().saturating_sub(1)));
    if s > e {
        return;
    }
    let ret_is_ptr = f.header.split('(').next().map_or(false, |h| h.contains('*'));
    check_allocations(clean, s, e, lang, out);
    check_free_flow(clean, lay, s, e, out);
    check_returns(clean, s, e, ret_is_ptr, out);
    check_uninit(clean, s, e, out);
    check_arrays(clean, global_arrays, s, e, out);
}

// ───────────────────────── waivers (the `unsafe { }` escape hatch) ─────────────────────────

fn apply_waivers(raw: &[&str], out: &mut [Diagnostic]) {
    let re = rx!(r"diadochi:allow\(\s*([A-Za-z0-9_,\s-]+?)\s*\)\s*(?::|-)?\s*(.*)$");
    for d in out.iter_mut() {
        if !matches!(d.severity, Severity::Error | Severity::Warning) {
            continue;
        }
        let mut cands = vec![d.line - 1];
        if d.line >= 2 {
            cands.push(d.line - 2);
        }
        for k in cands {
            let Some(c) = raw.get(k).and_then(|l| re.captures(l)) else { continue };
            let applies = c[1].split(',').any(|x| {
                let x = x.trim();
                x == d.code || x.eq_ignore_ascii_case("all")
            });
            if !applies {
                continue;
            }
            let reason = c[2].trim().trim_end_matches("*/").trim().to_string();
            if reason.is_empty() {
                d.help = Some("waiver ignored: state WHY it is safe, e.g. // diadochi:allow(DR002) src is capped at 7 chars".into());
            } else {
                d.severity = Severity::Waived;
                d.waived_reason = Some(reason);
            }
            break;
        }
    }
}

// ───────────────────────── Power of 10 (NASA/JPL) ─────────────────────────
// Gerard Holzmann's "Power of 10: Rules for Developing Safety-Critical Code".
// Rules 1-5, 8, 9 are checked here (heuristically); rule 10 is enforced through compiler flags;
// rules 6 and 7 are not checked yet.

pub const P10_TITLES: [&str; 10] = [
    "Simple control flow",
    "Bounded loops",
    "No allocation after init",
    "Short functions",
    "Assertions",
    "Smallest scope",
    "Check returns and params",
    "Limited preprocessor",
    "Limited pointers",
    "Pedantic warnings",
];

fn fn_name(header: &str) -> Option<String> {
    let pre = header.split('(').next()?;
    pre.rsplit(|c: char| !(c.is_alphanumeric() || c == '_'))
        .find(|t| !t.is_empty())
        .map(str::to_string)
}

fn p10_rules(clean: &[String], lay: &Layout, lang: Lang, out: &mut Vec<Diagnostic>) {
    let nfn = lay.functions.len();
    let names: Vec<Option<String>> = lay.functions.iter().map(|f| fn_name(&f.header)).collect();
    let owner = |line: usize| lay.functions.iter().position(|f| line >= f.start && line <= f.end);
    let last_line = clean.len().saturating_sub(1);

    // ── Rule 1: no goto / setjmp / longjmp, no recursion
    let ctrl = rx!(r"\b(goto|setjmp|longjmp)\b");
    for (i, line) in clean.iter().enumerate() {
        if line.trim_start().starts_with('#') {
            continue;
        }
        for c in ctrl.captures_iter(line) {
            let m = c.get(1).unwrap();
            push(out, Severity::Error, "P10-1", format!("{} breaks the simple-control-flow rule", m.as_str()),
                 i, col_of(line, m.start()), col_of(line, m.end()),
                 Some("use plain if / for / while / switch and return".into()));
        }
    }
    let call_res: Vec<Option<Regex>> = names
        .iter()
        .map(|n| n.as_ref().map(|n| Regex::new(&format!(r"\b{}\s*\(", regex::escape(n))).unwrap()))
        .collect();
    let mut calls: Vec<Vec<usize>> = vec![Vec::new(); nfn];
    for (fi, f) in lay.functions.iter().enumerate() {
        let last = f.end.min(last_line);
        for (gi, re) in call_res.iter().enumerate() {
            let Some(re) = re else { continue };
            if ((f.start + 1)..=last).any(|k| re.is_match(&clean[k])) {
                calls[fi].push(gi);
            }
        }
    }
    for fi in 0..nfn {
        let mut seen = vec![false; nfn];
        let mut stack: Vec<usize> = calls[fi].clone();
        let mut recursive = false;
        while let Some(x) = stack.pop() {
            if x == fi {
                recursive = true;
                break;
            }
            if seen[x] {
                continue;
            }
            seen[x] = true;
            stack.extend(calls[x].iter().copied());
        }
        if recursive {
            let direct = calls[fi].contains(&fi);
            let name = names[fi].clone().unwrap_or_default();
            push(out, Severity::Error, "P10-1",
                 format!("'{}' is {} recursive: its stack use cannot be bounded", name, if direct { "directly" } else { "indirectly" }),
                 lay.functions[fi].start, 0, clean[lay.functions[fi].start].len(),
                 Some("rewrite it as a loop with a fixed upper bound".into()));
        }
    }

    // ── Rule 2: every loop has a provable fixed upper bound
    let for_re = rx!(r"\b(for)\s*\(([^;]*);([^;]*);([^)]*)\)");
    let while_re = rx!(r"\b(while)\s*\(([^)]*)\)");
    let bound = rx!(r"(?:[<>]=?|!=)\s*(?:\d+|[A-Z][A-Z0-9_]*)\b|\b(?:\d+|[A-Z][A-Z0-9_]*)\s*(?:[<>]=?|!=)");
    let loop_help = "compare against a constant, e.g. for (i = 0; i < n && i < MAX_ITEMS; i++)";
    for (i, line) in clean.iter().enumerate() {
        if line.trim_start().starts_with('#') {
            continue;
        }
        for c in for_re.captures_iter(line) {
            if !bound.is_match(&c[3]) {
                let m = c.get(1).unwrap();
                push(out, Severity::Error, "P10-2", "loop bound cannot be proven statically".into(),
                     i, col_of(line, m.start()), col_of(line, m.end()), Some(loop_help.into()));
            }
        }
        for c in while_re.captures_iter(line) {
            if !bound.is_match(&c[2]) {
                let m = c.get(1).unwrap();
                push(out, Severity::Error, "P10-2", "loop bound cannot be proven statically".into(),
                     i, col_of(line, m.start()), col_of(line, m.end()), Some(loop_help.into()));
            }
        }
    }

    // ── Rule 3: no dynamic allocation after initialization (allowed in init*/setup* functions)
    let alloc_c = rx!(r"\b(malloc|calloc|realloc|free|strdup|_strdup|alloca)\s*\(");
    let alloc_cpp = rx!(r"(?:^|[^\w:])(new|delete)\b\s*(?:\[\s*\]\s*)?[A-Za-z_*(]");
    let mut alloc_res: Vec<&Regex> = vec![alloc_c];
    if lang == Lang::Cpp {
        alloc_res.push(alloc_cpp);
    }
    for (i, line) in clean.iter().enumerate() {
        if line.trim_start().starts_with('#') || line.contains("operator") {
            continue;
        }
        let Some(fi) = owner(i) else { continue };
        let nm = names[fi].clone().unwrap_or_default().to_lowercase();
        if nm.starts_with("init") || nm.starts_with("setup") {
            continue;
        }
        for re in &alloc_res {
            for c in re.captures_iter(line) {
                let m = c.get(1).unwrap();
                push(out, Severity::Error, "P10-3",
                     format!("{} allocates or frees memory outside an init function", m.as_str()),
                     i, col_of(line, m.start()), col_of(line, m.end()),
                     Some("allocate once in an init*/setup* function, or use static buffers".into()));
            }
        }
    }

    // ── Rules 4 and 5: function length and assertion density
    let assert_re = rx!(r"\b(?:assert|ASSERT)\s*\(");
    for (fi, f) in lay.functions.iter().enumerate() {
        let last = f.end.min(last_line);
        let code_lines = (f.start..=last).filter(|&k| !clean[k].trim().is_empty()).count();
        let name = names[fi].clone().unwrap_or_default();
        if code_lines > 60 {
            push(out, Severity::Error, "P10-4",
                 format!("'{}' is {} lines long (limit 60)", name, code_lines),
                 f.start, 0, clean[f.start].len(),
                 Some("split it into smaller functions that each do one thing".into()));
        }
        if code_lines > 3 {
            let n = (f.start..=last).map(|k| assert_re.find_iter(&clean[k]).count()).sum::<usize>();
            if n < 2 {
                push(out, Severity::Error, "P10-5",
                     format!("'{}' has {} assertion(s); at least 2 are required", name, n),
                     f.start, 0, clean[f.start].len(),
                     Some("assert(cond) things that must never happen, and return an error code when they do".into()));
            }
        }
    }

    // ── Rule 8: limited preprocessor
    let cond = rx!(r"^#\s*(?:if|ifdef|ifndef)\b");
    let mut conds = 0;
    for (i, line) in clean.iter().enumerate() {
        let t = line.trim_start();
        if !t.starts_with('#') {
            continue;
        }
        if t.starts_with("#define") || t.starts_with("# define") {
            if let Some(p) = t.find("##") {
                push(out, Severity::Error, "P10-8", "token pasting (##) is not allowed".into(), i, p, p + 2, None);
            }
            if let Some(p) = t.find("...").or_else(|| t.find("__VA_ARGS__")) {
                push(out, Severity::Error, "P10-8", "variadic macros are not allowed".into(), i, p, p + 3, None);
            }
        }
        if cond.is_match(t) {
            conds += 1;
            if conds == 4 {
                push(out, Severity::Error, "P10-8", "more than 3 conditional-compilation directives".into(),
                     i, 0, t.len(), Some("keep #if / #ifdef to a minimum".into()));
            }
        }
    }

    // ── Rule 9: limited pointers
    let fnptr = rx!(r"\(\s*\*\s*[A-Za-z_]\w*\s*\)\s*\(");
    let dstar = rx!(r"\*\s*\*\s*[A-Za-z_(]");
    let arrows = rx!(r"\w+(?:\[[^\]]*\])?\s*->\s*\w+(?:\[[^\]]*\])?\s*->");
    let macro_deref = rx!(r"->|(?:^|[(=,])\s*\*\s*\w");
    for (i, line) in clean.iter().enumerate() {
        let t = line.trim_start();
        if t.starts_with("#define") || t.starts_with("# define") {
            let body = t.splitn(3, char::is_whitespace).nth(2).unwrap_or("");
            if macro_deref.is_match(body) {
                push(out, Severity::Error, "P10-9", "pointer dereference hidden in a macro".into(), i, 0, t.len(), None);
            }
            continue;
        }
        if t.starts_with('#') {
            continue;
        }
        if let Some(m) = fnptr.find(line) {
            push(out, Severity::Error, "P10-9", "function pointers are not permitted".into(),
                 i, col_of(line, m.start()), col_of(line, m.end()), Some("call the function directly, or use a switch".into()));
        }
        if !line.contains("main(") {
            if let Some(m) = dstar.find(line) {
                push(out, Severity::Error, "P10-9", "more than one level of pointer dereference".into(),
                     i, col_of(line, m.start()), col_of(line, m.end()), Some("use a struct or an index instead of a pointer to a pointer".into()));
            }
        }
        if let Some(m) = arrows.find(line) {
            push(out, Severity::Error, "P10-9", "chained -> is more than one level of dereference".into(),
                 i, col_of(line, m.start()), col_of(line, m.end()), Some("copy the inner pointer into a local first, then use it".into()));
        }
    }
}

// ───────────────────────── entry point ─────────────────────────

pub fn analyze(src: &str, lang: Lang) -> Vec<Diagnostic> {
    analyze_opts(src, lang, false)
}

/// `p10` adds the NASA/JPL "Power of 10" rules on top of the normal ones.
pub fn analyze_opts(src: &str, lang: Lang, p10: bool) -> Vec<Diagnostic> {
    let raw: Vec<&str> = src.split('\n').collect();
    let clean = mask(src);
    let lay = scan_layout(&clean);
    let mut out = Vec::new();

    line_rules(&clean, lang, &mut out);
    scanf_rule(&raw, &clean, &mut out);
    globals(&clean, &lay, &mut out);

    let mut global_arrays = HashMap::new();
    if !clean.is_empty() {
        collect_arrays(&clean, 0..=clean.len() - 1, Some(&lay), &mut global_arrays);
    }
    for f in &lay.functions {
        check_function(&clean, &lay, &global_arrays, f, lang, &mut out);
    }

    if p10 {
        p10_rules(&clean, &lay, lang, &mut out);
    }

    apply_waivers(&raw, &mut out);
    out.sort_by_key(|d| (d.line, d.col_start));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes(src: &str, lang: Lang) -> Vec<String> {
        analyze(src, lang)
            .into_iter()
            .filter(|d| d.severity != Severity::Waived)
            .map(|d| d.code)
            .collect()
    }

    #[test]
    fn flags_strcpy() {
        let c = codes("int main(void){ char b[4]; strcpy(b, x); return 0; }", Lang::C);
        assert!(c.contains(&"DR002".to_string()));
    }

    #[test]
    fn flags_use_after_free() {
        let src = "int main(void) {\n    int *p = malloc(4);\n    if (!p) return 1;\n    free(p);\n    printf(\"%d\", *p);\n    return 0;\n}\n";
        assert!(codes(src, Lang::C).contains(&"DR106".to_string()));
    }

    #[test]
    fn p10_flags_recursion() {
        let src = "int f(int n)\n{\n    return f(n - 1);\n}\n";
        let all = analyze_opts(src, Lang::C, true);
        assert!(all.iter().any(|d| d.code == "P10-1"));
    }

    #[test]
    fn waiver_with_reason_downgrades() {
        let src = "void f(char *s){ char b[8];\n// diadochi:allow(DR002) s is capped at 7 chars by the caller\nstrcpy(b, s); }\n";
        let all = analyze(src, Lang::C);
        assert!(all.iter().any(|d| d.code == "DR002" && d.severity == Severity::Waived));
    }
}
