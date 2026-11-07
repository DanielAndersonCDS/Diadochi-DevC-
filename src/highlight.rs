//! Tiny C/C++ syntax highlighter that produces an egui LayoutJob.

use crate::theme::Palette;
use eframe::egui::{text::LayoutJob, Color32, FontId, TextFormat};

const KEYWORDS: &[&str] = &[
    "auto", "break", "case", "const", "continue", "default", "do", "else", "enum", "extern", "for",
    "goto", "if", "inline", "register", "return", "sizeof", "static", "struct", "switch", "typedef",
    "union", "volatile", "while", "class", "namespace", "template", "typename", "public", "private",
    "protected", "virtual", "override", "final", "new", "delete", "try", "catch", "throw", "using",
    "constexpr", "noexcept", "nullptr", "operator", "this", "true", "false", "explicit", "friend",
    "mutable", "static_cast", "dynamic_cast", "reinterpret_cast", "const_cast", "NULL",
];

const TYPES: &[&str] = &[
    "int", "char", "short", "long", "float", "double", "void", "signed", "unsigned", "bool",
    "size_t", "FILE", "string", "vector", "map", "unique_ptr", "shared_ptr", "int8_t", "int16_t",
    "int32_t", "int64_t", "uint8_t", "uint16_t", "uint32_t", "uint64_t", "wchar_t",
];

fn emit(job: &mut LayoutJob, chars: &[char], a: usize, b: usize, font: &FontId, color: Color32) {
    if a >= b {
        return;
    }
    let s: String = chars[a..b].iter().collect();
    job.append(&s, 0.0, TextFormat::simple(font.clone(), color));
}

pub fn layout(text: &str, pal: &Palette, font: FontId) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = f32::INFINITY; // code never wraps; the editor scrolls sideways
    let ch: Vec<char> = text.chars().collect();
    let n = ch.len();
    let mut i = 0;

    while i < n {
        let c = ch[i];
        let next = if i + 1 < n { ch[i + 1] } else { '\0' };

        if c == '/' && next == '/' {
            let mut j = i;
            while j < n && ch[j] != '\n' {
                j += 1;
            }
            emit(&mut job, &ch, i, j, &font, pal.comment);
            i = j;
        } else if c == '/' && next == '*' {
            let mut j = i + 2;
            while j + 1 < n && !(ch[j] == '*' && ch[j + 1] == '/') {
                j += 1;
            }
            j = (j + 2).min(n);
            emit(&mut job, &ch, i, j, &font, pal.comment);
            i = j;
        } else if c == '#' && {
            let mut k = i;
            while k > 0 && (ch[k - 1] == ' ' || ch[k - 1] == '\t') {
                k -= 1;
            }
            k == 0 || ch[k - 1] == '\n'
        } {
            let mut j = i;
            while j < n && ch[j] != '\n' {
                j += 1;
            }
            emit(&mut job, &ch, i, j, &font, pal.preproc);
            i = j;
        } else if c == '"' || c == '\'' {
            let mut j = i + 1;
            while j < n && ch[j] != c && ch[j] != '\n' {
                if ch[j] == '\\' {
                    j += 1;
                }
                j += 1;
            }
            j = (j + 1).min(n);
            emit(&mut job, &ch, i, j, &font, pal.string);
            i = j;
        } else if c.is_ascii_digit() {
            let mut j = i;
            while j < n && (ch[j].is_alphanumeric() || ch[j] == '.' || ch[j] == '_') {
                j += 1;
            }
            emit(&mut job, &ch, i, j, &font, pal.number);
            i = j;
        } else if c.is_alphabetic() || c == '_' {
            let mut j = i;
            while j < n && (ch[j].is_alphanumeric() || ch[j] == '_') {
                j += 1;
            }
            let word: String = ch[i..j].iter().collect();
            let mut k = j;
            while k < n && (ch[k] == ' ' || ch[k] == '\t') {
                k += 1;
            }
            let color = if KEYWORDS.contains(&word.as_str()) {
                pal.keyword
            } else if TYPES.contains(&word.as_str()) {
                pal.typ
            } else if k < n && ch[k] == '(' {
                pal.func
            } else {
                pal.text
            };
            emit(&mut job, &ch, i, j, &font, color);
            i = j;
        } else {
            let mut j = i + 1;
            while j < n {
                let d = ch[j];
                if d.is_alphanumeric() || d == '_' || d == '"' || d == '\'' || d == '/' || d == '#' {
                    break;
                }
                j += 1;
            }
            emit(&mut job, &ch, i, j, &font, pal.text);
            i = j;
        }
    }
    job
}
