//! Typing assistance: auto-close brackets and quotes, smart Enter / Backspace / Tab.
//! Works by rewriting egui's input events before the TextEdit sees them.

use eframe::egui::{
    self,
    text::{CCursor, CCursorRange},
    Event, Key,
};

const INDENT: &str = "    ";

fn byte_of(s: &str, ci: usize) -> usize {
    s.char_indices().nth(ci).map(|(b, _)| b).unwrap_or(s.len())
}
fn insert_at(s: &mut String, ci: usize, t: &str) {
    let b = byte_of(s, ci);
    s.insert_str(b, t);
}
fn delete_chars(s: &mut String, a: usize, b: usize) {
    let (x, y) = (byte_of(s, a), byte_of(s, b));
    s.replace_range(x..y, "");
}
fn char_at(s: &str, ci: usize) -> Option<char> {
    s.chars().nth(ci)
}
fn line_start(s: &str, ci: usize) -> usize {
    let mut start = 0;
    for (n, c) in s.chars().enumerate() {
        if n >= ci {
            break;
        }
        if c == '\n' {
            start = n + 1;
        }
    }
    start
}
fn text_before_caret_on_line(s: &str, caret: usize) -> String {
    let ls = line_start(s, caret);
    s.chars().skip(ls).take(caret - ls).collect()
}
fn pair(c: char) -> char {
    match c {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        '"' => '"',
        '\'' => '\'',
        _ => '\0',
    }
}
fn ok_to_autoclose(next: Option<char>) -> bool {
    next.map_or(true, |x| x.is_whitespace() || ")]};,".contains(x))
}

/// Returns true if the text or cursor was changed.
pub fn process(ctx: &egui::Context, id: egui::Id, text: &mut String) -> bool {
    if !ctx.memory(|m| m.has_focus(id)) {
        return false;
    }
    let Some(mut st) = egui::TextEdit::load_state(ctx, id) else { return false };
    let Some(range) = st.cursor.char_range() else { return false };
    let mut caret = range.primary.index;
    let mut anchor = range.secondary.index;

    let events = ctx.input_mut(|i| std::mem::take(&mut i.events));
    let mut kept = Vec::with_capacity(events.len());
    let mut changed = false;
    for ev in events {
        if handle(&ev, text, &mut caret, &mut anchor) {
            changed = true;
        } else {
            kept.push(ev);
        }
    }
    ctx.input_mut(|i| i.events = kept);

    if changed {
        st.cursor.set_char_range(Some(CCursorRange {
            primary: CCursor::new(caret),
            secondary: CCursor::new(anchor),
        }));
        st.store(ctx, id);
    }
    changed
}

fn handle(ev: &Event, text: &mut String, caret: &mut usize, anchor: &mut usize) -> bool {
    let lo = (*caret).min(*anchor);
    let hi = (*caret).max(*anchor);
    let has_sel = lo != hi;

    match ev {
        Event::Text(t) => {
            let mut it = t.chars();
            let (Some(c), None) = (it.next(), it.next()) else { return false };
            typed(c, text, caret, anchor, lo, hi, has_sel)
        }
        Event::Key { key, pressed: true, modifiers, .. } => match key {
            Key::Enter if !modifiers.command => {
                if has_sel {
                    delete_chars(text, lo, hi);
                    *caret = lo;
                    *anchor = lo;
                }
                let c = *caret;
                let before = text_before_caret_on_line(text, c);
                let indent: String = before.chars().take_while(|ch| *ch == ' ' || *ch == '\t').collect();
                let bt = before.trim_end();
                let head = bt.trim_start();
                let n = char_at(text, c);
                let opener = bt.ends_with('{');
                let control = (head.starts_with("if") || head.starts_with("for") || head.starts_with("while") || head.starts_with("else"))
                    && (bt.ends_with(')') || head == "else");
                let case_label = (head.starts_with("case ") || head == "default:") && bt.ends_with(':');
                if opener && n == Some('}') {
                    let ins = format!("\n{indent}{INDENT}\n{indent}");
                    insert_at(text, c, &ins);
                    *caret = c + 1 + indent.chars().count() + INDENT.len();
                } else {
                    let extra = if opener || control || case_label { INDENT } else { "" };
                    let ins = format!("\n{indent}{extra}");
                    insert_at(text, c, &ins);
                    *caret = c + ins.chars().count();
                }
                *anchor = *caret;
                true
            }
            Key::Backspace if modifiers.is_none() => {
                if has_sel || *caret == 0 {
                    return false;
                }
                let c = *caret;
                if let (Some(b), Some(n)) = (char_at(text, c - 1), char_at(text, c)) {
                    if pair(b) == n && n != '\0' {
                        delete_chars(text, c - 1, c + 1);
                        *caret = c - 1;
                        *anchor = c - 1;
                        return true;
                    }
                }
                let before = text_before_caret_on_line(text, c);
                let col = before.chars().count();
                if col > 0 && before.chars().all(|ch| ch == ' ') {
                    let k = if col % 4 == 0 { 4 } else { col % 4 };
                    delete_chars(text, c - k, c);
                    *caret = c - k;
                    *anchor = *caret;
                    return true;
                }
                false
            }
            Key::Tab if !modifiers.command && !modifiers.alt => {
                if !has_sel && !modifiers.shift {
                    let col = text_before_caret_on_line(text, *caret).chars().count();
                    let n = 4 - col % 4;
                    insert_at(text, *caret, &" ".repeat(n));
                    *caret += n;
                    *anchor = *caret;
                } else {
                    shift_lines(text, lo, hi, modifiers.shift, caret, anchor);
                }
                true
            }
            _ => false,
        },
        _ => false,
    }
}

fn typed(c: char, text: &mut String, caret: &mut usize, anchor: &mut usize, lo: usize, hi: usize, has_sel: bool) -> bool {
    if has_sel {
        if matches!(c, '(' | '[' | '{' | '"' | '\'') {
            // wrap the selection
            insert_at(text, hi, &pair(c).to_string());
            insert_at(text, lo, &c.to_string());
            *anchor = lo + 1;
            *caret = hi + 1;
            return true;
        }
        return false;
    }
    let c0 = *caret;
    let next = char_at(text, c0);
    let prev = if c0 > 0 { char_at(text, c0 - 1) } else { None };
    match c {
        '(' | '[' | '{' => {
            if !ok_to_autoclose(next) {
                return false;
            }
            insert_at(text, c0, &format!("{}{}", c, pair(c)));
            *caret = c0 + 1;
            *anchor = *caret;
            true
        }
        ')' | ']' | '}' => {
            if next == Some(c) {
                *caret = c0 + 1; // type over the auto-inserted closer
                *anchor = *caret;
                return true;
            }
            if c == '}' {
                let before = text_before_caret_on_line(text, c0);
                if before.chars().count() >= 4 && before.chars().all(|ch| ch == ' ') {
                    delete_chars(text, c0 - 4, c0);
                    insert_at(text, c0 - 4, "}");
                    *caret = c0 - 3;
                    *anchor = *caret;
                    return true;
                }
            }
            false
        }
        '"' | '\'' => {
            if next == Some(c) {
                *caret = c0 + 1;
                *anchor = *caret;
                return true;
            }
            if prev.map_or(false, |x| x.is_alphanumeric() || x == '_' || x == '\\') || !ok_to_autoclose(next) {
                return false;
            }
            insert_at(text, c0, &format!("{}{}", c, c));
            *caret = c0 + 1;
            *anchor = *caret;
            true
        }
        _ => false,
    }
}

/// Tab / Shift+Tab on a selection (or Shift+Tab on the current line).
fn shift_lines(text: &mut String, lo: usize, hi: usize, dedent: bool, caret: &mut usize, anchor: &mut usize) {
    let chars: Vec<char> = text.chars().collect();
    let mut ls0 = lo;
    while ls0 > 0 && chars[ls0 - 1] != '\n' {
        ls0 -= 1;
    }
    let mut end = hi;
    if hi > lo && chars[hi - 1] == '\n' {
        end = hi - 1;
    }
    let mut le = end;
    while le < chars.len() && chars[le] != '\n' {
        le += 1;
    }
    let block: String = chars[ls0..le].iter().collect();
    let mut first_delta: isize = 0;
    let mut out: Vec<String> = Vec::new();
    for (k, l) in block.split('\n').enumerate() {
        let new_line = if dedent {
            let rm = l.chars().take(4).take_while(|c| *c == ' ').count();
            l[rm..].to_string()
        } else if l.is_empty() {
            String::new()
        } else {
            format!("{INDENT}{l}")
        };
        if k == 0 {
            first_delta = new_line.chars().count() as isize - l.chars().count() as isize;
        }
        out.push(new_line);
    }
    let new_block = out.join("\n");
    let new_len = new_block.chars().count();
    let (a, b) = (byte_of(text, ls0), byte_of(text, le));
    text.replace_range(a..b, &new_block);
    if lo == hi {
        let nc = (*caret as isize + first_delta).max(ls0 as isize) as usize;
        *caret = nc;
        *anchor = nc;
    } else {
        *anchor = ls0;
        *caret = ls0 + new_len;
    }
}
