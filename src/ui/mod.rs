use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, Paragraph};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::keys::{BINDINGS, key_name};
use crate::app::queue::{Queue, Repeat};
use crate::app::state::{AppState, Focus, Items, ListKind, SECTIONS, SearchItem, View};

const SIDEBAR_WIDTH: u16 = 19;

pub fn draw(f: &mut Frame, s: &mut AppState) {
    let msg_height = u16::from(s.message.is_some());
    let [main, msg, bar] = Layout::vertical([
        Constraint::Min(3),
        Constraint::Length(msg_height),
        Constraint::Length(4),
    ])
    .areas(f.area());
    let [side, list] =
        Layout::horizontal([Constraint::Length(SIDEBAR_WIDTH), Constraint::Min(10)]).areas(main);

    draw_sidebar(f, s, side);
    draw_list(f, s, list);
    if let Some(m) = &s.message {
        let text = truncate(&m.text, msg.width as usize);
        let style = if m.error {
            Style::new().fg(Color::Red)
        } else {
            Style::new()
        };
        f.render_widget(Paragraph::new(text).style(style), msg);
    }
    draw_now_playing(f, s, bar);
    if s.help {
        draw_help(f);
    }
}

fn selected_style(focused: bool, accent: Color) -> Style {
    if focused {
        // reversed rather than a fixed foreground so text stays readable whatever the accent
        Style::new().fg(accent).add_modifier(Modifier::REVERSED)
    } else {
        Style::new().add_modifier(Modifier::BOLD)
    }
}

fn border_style(focused: bool, accent: Color) -> Style {
    if focused {
        Style::new().fg(accent)
    } else {
        Style::new()
    }
}

fn draw_sidebar(f: &mut Frame, s: &AppState, area: Rect) {
    let block = Block::bordered()
        .title(" Library ")
        .border_style(border_style(s.focus == Focus::Sidebar, s.accent));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let width = inner.width as usize;
    let lines: Vec<Line> = SECTIONS
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let selected = i == s.section;
            let marker = if selected { "> " } else { "  " };
            let text = match i {
                4 => format!("{marker}{name} ({})", s.queue.len()),
                _ => format!("{marker}{name}"),
            };
            let line = Line::raw(fit(&text, "", width));
            if selected {
                line.style(selected_style(s.focus == Focus::Sidebar, s.accent))
            } else {
                line
            }
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}

fn breadcrumb(stack: &[View]) -> String {
    stack
        .iter()
        .map(|v| v.title.as_str())
        .collect::<Vec<_>>()
        .join(" › ")
}

fn draw_list(f: &mut Frame, s: &mut AppState, area: Rect) {
    let title_width = (area.width as usize).saturating_sub(16);
    let crumb = match &s.search_input {
        Some(q) => truncate_left(&format!("Search: {q}▏"), title_width),
        None => truncate_left(&breadcrumb(&s.stack), title_width),
    };
    let focused = s.focus == Focus::List;
    let Some(v) = s.stack.last_mut() else { return };
    let is_queue = matches!(v.kind, ListKind::Queue);

    let count = match (v.total, v.loading) {
        _ if is_queue => format!(" {} ", s.queue.len()),
        (Some(t), _) if v.items.len() < t as usize => format!(" {}/{t} ", v.items.len()),
        (Some(t), _) => format!(" {t} "),
        (None, true) => " … ".to_owned(),
        (None, false) => String::new(),
    };
    let block = Block::bordered()
        .title(format!(" {crumb} "))
        .title_top(Line::raw(count).right_aligned())
        .border_style(border_style(focused, s.accent));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let height = inner.height as usize;
    let width = inner.width as usize;
    s.list_height = height;
    let len = if is_queue {
        s.queue.len()
    } else {
        v.items.len()
    };
    if len == 0 {
        let hint = if v.loading {
            "loading…"
        } else {
            match v.kind {
                ListKind::Search if s.search_input.is_some() => {
                    "type a query, Enter to search, Esc to cancel"
                }
                ListKind::Search => "press / to search",
                ListKind::Queue => "queue is empty",
                _ => "nothing here",
            }
        };
        f.render_widget(Paragraph::new(format!("  {hint}")), inner);
        return;
    }

    if v.selected < v.offset {
        v.offset = v.selected;
    } else if v.selected >= v.offset + height {
        v.offset = v.selected + 1 - height;
    }
    let end = (v.offset + height).min(len);
    let lines: Vec<Line> = (v.offset..end)
        .map(|i| {
            let (left, right) = if is_queue {
                queue_row(&s.queue, i)
            } else {
                row(v, i)
            };
            if let Items::Search(items) = &v.items
                && let SearchItem::Header(h) = &items[i]
            {
                return Line::raw(fit(h, "", width))
                    .style(Style::new().fg(s.accent).add_modifier(Modifier::BOLD));
            }
            let marker = if i == v.selected { "> " } else { "  " };
            let line = Line::raw(fit(&format!("{marker}{left}"), &right, width));
            if i == v.selected {
                line.style(selected_style(focused, s.accent))
            } else if is_queue && s.queue.current == Some(i) {
                line.style(Style::new().fg(s.accent))
            } else {
                line
            }
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}

fn row(v: &View, i: usize) -> (String, String) {
    match &v.items {
        Items::Artists(a) => (a[i].title.clone(), String::new()),
        Items::Albums(a) => {
            let a = &a[i];
            let year = a.year.map_or("    ".to_owned(), |y| y.to_string());
            let left = match v.kind {
                ListKind::ArtistAlbums(_) => format!("{year}  {}", a.title),
                _ => format!("{year}  {} — {}", a.title, a.parent_title),
            };
            let right = a
                .leaf_count
                .map_or(String::new(), |n| format!("{n} tracks"));
            (left, right)
        }
        Items::Tracks(tracks) => {
            let t = &tracks[i];
            let left = match v.kind {
                ListKind::AlbumTracks(_) => {
                    let n = t.index.map_or(String::new(), |n| n.to_string());
                    if tracks.iter().any(|t| t.disc > Some(1)) {
                        let disc = t.disc.unwrap_or(1);
                        format!("{disc}-{n:0>2}  {}", t.title)
                    } else {
                        format!("{n:>2}  {}", t.title)
                    }
                }
                _ => format!("{} — {}", t.title, t.grandparent_title),
            };
            (left, t.duration_ms.map_or(String::new(), crate::fmt_ms))
        }
        Items::Search(items) => match &items[i] {
            SearchItem::Header(_) => (String::new(), String::new()),
            SearchItem::Artist(a) => (a.title.clone(), String::new()),
            SearchItem::Album(a) => {
                let year = a.year.map_or(String::new(), |y| y.to_string());
                (format!("{} — {}", a.title, a.parent_title), year)
            }
            SearchItem::Track(t) => (
                format!("{} — {} · {}", t.title, t.grandparent_title, t.parent_title),
                t.duration_ms.map_or(String::new(), crate::fmt_ms),
            ),
        },
        Items::Playlists(p) => {
            let p = &p[i];
            let count = p.leaf_count.map(|n| format!("{n} tracks"));
            let dur = p.duration_ms.map(crate::fmt_ms);
            let right = [count, dur].into_iter().flatten().collect::<Vec<_>>();
            (p.title.clone(), right.join(" · "))
        }
    }
}

fn queue_row(q: &Queue, i: usize) -> (String, String) {
    let t = &q.entries[i].track;
    let playing = if q.current == Some(i) { "▶ " } else { "  " };
    let left = format!("{playing}{} — {}", t.title, t.grandparent_title);
    (left, t.duration_ms.map_or(String::new(), crate::fmt_ms))
}

fn draw_now_playing(f: &mut Frame, s: &AppState, area: Rect) {
    let block = Block::bordered();
    let inner = block.inner(area);
    f.render_widget(block, area);
    let width = inner.width as usize;
    let now = &s.now;

    let first = match &now.track {
        None => fit("■ not playing", "", width),
        Some(t) => {
            let icon = if now.paused { "‖" } else { "▶" };
            let mut text = format!("{icon} {}", t.title);
            for part in [&t.grandparent_title, &t.parent_title] {
                if !part.is_empty() {
                    text.push_str(if text.contains(" — ") {
                        " · "
                    } else {
                        " — "
                    });
                    text.push_str(part);
                }
            }
            fit(&text, if now.buffering { "buffering…" } else { "" }, width)
        }
    };

    let mut right = String::new();
    let duration = now.track.as_ref().and_then(|t| t.duration_ms);
    if let Some(d) = duration {
        right.push_str(&format!(" {}", crate::fmt_ms(d)));
    }
    right.push_str(&format!("  vol {:.0}%", now.volume * 100.0));
    if s.queue.shuffle {
        right.push_str(" ⇄");
    }
    match s.queue.repeat {
        Repeat::Off => {}
        Repeat::All => right.push_str(" ↻"),
        Repeat::One => right.push_str(" ↻1"),
    }
    if s.debug_line {
        right.push_str(&format!("  rss {:.1} MB", crate::rss_kb() as f64 / 1024.0));
    }
    let left = match now.track {
        Some(_) => format!("{} ", crate::fmt_ms(now.position_ms)),
        None => String::new(),
    };
    let bar_width = width.saturating_sub(left.width() + right.width());
    let bar = match duration {
        Some(d) if d > 0 && now.track.is_some() && bar_width >= 5 => {
            let filled = ((now.position_ms.min(d) as f64 / d as f64) * bar_width as f64) as usize;
            format!("{}{}", "━".repeat(filled), "─".repeat(bar_width - filled))
        }
        _ => String::new(),
    };
    let second = fit(&format!("{left}{bar}"), right.trim_start(), width);
    f.render_widget(
        Paragraph::new(vec![Line::raw(first), Line::raw(second)]),
        inner,
    );
}

fn draw_help(f: &mut Frame) {
    let mut groups: Vec<(&str, Vec<String>)> = Vec::new();
    for b in BINDINGS {
        let desc = b.action.describe();
        match groups.iter_mut().find(|(d, _)| *d == desc) {
            Some((_, keys)) => keys.push(key_name(b)),
            None => groups.push((desc, vec![key_name(b)])),
        }
    }
    let keys: Vec<String> = groups.iter().map(|(_, k)| k.join(" ")).collect();
    let key_width = keys.iter().map(|k| k.width()).max().unwrap_or(0);

    let area = f.area();
    let cols = if groups.len() + 2 > area.height as usize {
        2
    } else {
        1
    };
    let rows = groups.len().div_ceil(cols);
    let width = (cols as u16 * 39 + 2).min(area.width);
    let height = (rows as u16 + 2).min(area.height);
    let popup = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    let block = Block::bordered().title(" Keys — ? to close ");
    let inner = block.inner(popup);
    let entries: Vec<String> = groups
        .iter()
        .zip(&keys)
        .map(|((desc, _), k)| format!("{k}{}{desc}", " ".repeat(key_width - k.width() + 2)))
        .collect();
    let col_width = inner.width as usize / cols;
    let lines: Vec<Line> = (0..rows)
        .map(|r| {
            let line: String = (0..cols)
                .filter_map(|c| entries.get(c * rows + r))
                .map(|e| fit(e, "", col_width))
                .collect();
            Line::raw(line)
        })
        .collect();
    f.render_widget(Clear, popup);
    f.render_widget(block, popup);
    f.render_widget(Paragraph::new(lines), inner);
}

/// `left` truncated with an ellipsis, then `right` flush against the right edge
fn fit(left: &str, right: &str, width: usize) -> String {
    let rw = right.width();
    if rw + 2 > width {
        return truncate(left, width);
    }
    let l = truncate(left, width - rw - 1);
    let pad = width - l.width() - rw;
    format!("{l}{}{right}", " ".repeat(pad))
}

fn truncate(s: &str, width: usize) -> String {
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

fn truncate_left(s: &str, width: usize) -> String {
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
