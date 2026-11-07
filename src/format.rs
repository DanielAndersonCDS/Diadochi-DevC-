//! Document formatter: operator/comma/keyword spacing, brace-based re-indentation,
//! trailing-whitespace and blank-line cleanup. Comments and strings are never touched
//! (positions come from the masked copy produced by the safety module).

use crate::safety::mask;
use regex::Regex;

const INDENT: &str = "    ";

pub fn format_source(src: &str) -> String {
    let src = src.replace("\r\n", "\n");
    let spaced = space_pass(&src);
    reindent(&spaced)
}

fn cidx(s: &str, byte: usize) -> usize {
    s[..byte].chars().count()
}

/// Normalize whitespace around the token rc[a..b]. Only real whitespace is touched.
fn pad(rc: &[char], a: usize, b: usize, space_left: bool, space_right: bool) -> (usize, usize, String) {
    let mut l = a;
    while l > 0 && rc[l - 1].is_whitespace() {
        l -= 1;
    }
    let mut r = b;
    while r < rc.len() && rc[r].is_whitespace() {
        r += 1;
    }
    let lead = rc[..l].iter().all(|c| c.is_whitespace());
    let left = if lead || !space_left { "" } else { " " };
    let right = if r >= rc.len() || !space_right { "" } else { " " };
    let start = if lead { a } else { l };
    let tok: String = rc[a..b].iter().collect();
    (start, r, format!("{left}{tok}{right}"))
}

fn space_pass(src: &str) -> String {
    let ops = Regex::new(r"<<=|>>=|==|!=|<=|>=|\+=|-=|\*=|/=|%=|&=|\|=|\^=|\|\||=").unwrap();
    let kw = Regex::new(r"\b(?:if|for|while|switch|catch)\(").unwrap();
    let brace = Regex::new(r"(?:\)|\belse)\{").unwrap();
    let raw: Vec<&str> = src.split('\n').collect();
    let clean = mask(src);
    let mut out: Vec<String> = Vec::with_capacity(raw.len());

    for (i, rawl) in raw.iter().enumerate() {
        let cl = clean.get(i).map(String::as_str).unwrap_or("");
        let rc: Vec<char> = rawl.chars().collect();
        let cc: Vec<char> = cl.chars().collect();
        if rc.len() != cc.len() || cl.trim_start().starts_with('#') || cl.contains("operator") {
            out.push(rawl.trim_end().to_string());
            continue;
        }
        let mut edits: Vec<(usize, usize, String)> = Vec::new();

        for m in ops.find_iter(cl) {
            let (a, b) = (cidx(cl, m.start()), cidx(cl, m.end()));
            if b - a == 1 {
                // lambda capture [=]
                let prev = cc[..a].iter().rev().find(|c| !c.is_whitespace());
                if prev == Some(&'[') {
                    continue;
                }
            }
            edits.push(pad(&rc, a, b, true, true));
        }
        let mut paren = 0i32;
        for (p, &ch) in cc.iter().enumerate() {
            match ch {
                '(' => paren += 1,
                ')' => paren -= 1,
                ',' => edits.push(pad(&rc, p, p + 1, false, true)),
                ';' if paren > 0 => {
                    let next = cc[p + 1..].iter().find(|c| !c.is_whitespace());
                    if !matches!(next, Some(';') | Some(')') | None) {
                        edits.push(pad(&rc, p, p + 1, false, true));
                    }
                }
                _ => {}
            }
        }
        for m in kw.find_iter(cl) {
            let p = cidx(cl, m.end()) - 1;
            edits.push((p, p, " ".into()));
        }
        for m in brace.find_iter(cl) {
            let p = cidx(cl, m.end()) - 1;
            edits.push((p, p, " ".into()));
        }

        edits.sort_by_key(|e| (e.0, e.1));
        let mut res = String::new();
        let mut pos = 0usize;
        for (s, e, t) in edits {
            if s < pos {
                continue; // overlaps an earlier edit
            }
            res.extend(rc[pos..s].iter());
            res.push_str(&t);
            pos = e;
        }
        res.extend(rc[pos..].iter());
        out.push(res.trim_end().to_string());
    }
    out.join("\n")
}

fn reindent(src: &str) -> String {
    let ctrl = Regex::new(r"^(?:if|for|while|else if|else)\b").unwrap();
    let label = Regex::new(r"^(?:case\b.*|default\s*|public\s*|private\s*|protected\s*):\s*($|[^:])").unwrap();
    let raw: Vec<&str> = src.split('\n').collect();
    let clean = mask(src);
    let mut out: Vec<String> = Vec::new();
    let mut depth: i32 = 0;
    let mut paren: i32 = 0;
    let mut single = false;
    let mut blank_run = 0;

    for (i, rawl) in raw.iter().enumerate() {
        let cl = clean.get(i).map(String::as_str).unwrap_or("");
        if rawl.trim().is_empty() {
            blank_run += 1;
            if blank_run <= 1 && !out.is_empty() {
                out.push(String::new());
            }
            continue;
        }
        blank_run = 0;
        let tc = cl.trim();
        let body = rawl.trim_start();
        let base = depth.max(0) as usize;

        if tc.is_empty() {
            // comment-only line, or the inside of a comment
            if body.starts_with("//") || body.starts_with("/*") {
                out.push(format!("{}{}", INDENT.repeat(base), body.trim_end()));
            } else if body.starts_with('*') {
                out.push(format!("{} {}", INDENT.repeat(base), body.trim_end()));
            } else {
                out.push(rawl.trim_end().to_string());
            }
            continue;
        }
        if tc.starts_with('#') {
            out.push(body.trim_end().to_string());
            continue;
        }

        let closers = tc.chars().take_while(|c| *c == '}').count() as i32;
        let mut level = (depth - closers).max(0) as usize;
        if label.is_match(tc) {
            level = (depth - 1).max(0) as usize;
        }
        if paren > 0 && closers == 0 {
            level += 1;
        }
        if single && !tc.starts_with('{') {
            level += 1;
        }
        single = false;
        out.push(format!("{}{}", INDENT.repeat(level), body.trim_end()));

        depth = (depth + tc.matches('{').count() as i32 - tc.matches('}').count() as i32).max(0);
        paren = (paren + tc.matches('(').count() as i32 - tc.matches(')').count() as i32).max(0);
        let t2 = tc.trim_start_matches('}').trim_start();
        if paren == 0
            && ctrl.is_match(t2)
            && !tc.ends_with('{')
            && !tc.ends_with(';')
            && !tc.ends_with('}')
            && (tc.ends_with(')') || t2 == "else")
        {
            single = true;
        }
    }
    while out.last().map_or(false, |l| l.is_empty()) {
        out.pop();
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indents_and_spaces() {
        let src = "int main(){\nint x=1;\nif(x==1){\nx=2;\n}\nreturn 0;\n}\n";
        let want = "int main() {\n    int x = 1;\n    if (x == 1) {\n        x = 2;\n    }\n    return 0;\n}\n";
        assert_eq!(format_source(src), want);
    }

    #[test]
    fn leaves_strings_and_comments_alone() {
        let src = "int main() {\n    printf(\"a=b,c\"); // x=1,y\n}\n";
        assert_eq!(format_source(src), src);
    }
}
