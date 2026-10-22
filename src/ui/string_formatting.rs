use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// `left` truncated with an ellipsis, then `right` flush against the right edge
pub fn fit(left: &str, right: &str, width: usize) -> String {
    let rw = right.width();
    if rw + 2 > width {
        return truncate(left, width);
    }
    let l = truncate(left, width - rw - 1);
    let pad = width - l.width() - rw;
    format!("{l}{}{right}", " ".repeat(pad))
}

pub fn truncate(s: &str, width: usize) -> String {
    if s.width() <= width {
        return s.to_owned();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if w + cw + 1 > width {
            break;
        }
        out.push(c);
        w += cw;
    }
    if width > 0 {
        out.push('…');
    }
    out
}

pub fn truncate_left(s: &str, width: usize) -> String {
    if s.width() <= width {
        return s.to_owned();
    }
    let mut out: Vec<char> = Vec::new();
    let mut w = 0;
    for c in s.chars().rev() {
        let cw = c.width().unwrap_or(0);
        if w + cw + 1 > width {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out.iter().rev().collect()
}

pub fn fmt_ms(ms: u64) -> String {
    let secs = ms / 1000;
    match secs / 3600 {
        0 => format!("{}:{:02}", secs / 60, secs % 60),
        h => format!("{h}:{:02}:{:02}", secs / 60 % 60, secs % 60),
    }
}
