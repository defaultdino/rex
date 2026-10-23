use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::app::browser::{Items, ListKind, SearchItem, Section, View};
use crate::app::keys::{BINDINGS, SEARCH_INPUT_KEYS, key_name};
use crate::app::now_playing::NowPlaying;
use crate::app::queue::Queue;
use crate::app::state::{AppState, Focus, Modal, Screen};
use crate::plex::models::{Album, Artist, Track};
use crate::ui::help_bar::draw_help_bar;
use crate::ui::player::{draw_maximized_player, draw_minimized_player};
use crate::ui::string_formatting::{fit, fmt_ms, truncate_left};

mod help_bar;
mod player;
mod string_formatting;

const SIDEBAR_WIDTH: u16 = 19;

pub fn draw(f: &mut Frame, s: &mut AppState) {
    let [main, help_bar] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(f.area());

    match s.screen {
        Screen::Player => draw_maximized_player(f, s, main),
        Screen::Browse(_) => draw_minimized_player_layout(f, s, main),
    }

    draw_help_bar(f, s, help_bar);
    if matches!(s.modal, Some(Modal::Help)) {
        draw_help(f);
    }
    #[cfg(feature = "cast")]
    if let Some(Modal::Cast(p)) = &s.modal {
        draw_cast(f, p, s.accent);
    }
}

#[cfg(feature = "cast")]
fn draw_cast(f: &mut Frame, p: &crate::app::cast::Picker, accent: Color) {
    use crate::app::cast::Search;
    let row = |i: usize, text: String| {
        let line = Line::raw(text);
        if i == p.selected {
            line.style(selected_style(true, accent))
        } else {
            line
        }
    };
    let mut lines = vec![row(0, "this computer".into())];
    lines.extend(
        p.devices
            .iter()
            .enumerate()
            .map(|(i, d)| row(i + 1, format!("{}  {}", d.name, d.ip))),
    );
    match &p.search {
        Search::Running => lines.push(Line::raw("looking for devices…")),
        Search::Failed(e) => lines.push(Line::styled(e.as_str(), Style::new().fg(Color::Red))),
        Search::Done if p.devices.is_empty() => lines.push(Line::raw("no cast devices found")),
        Search::Done => {}
    }
    let popup = f.area().centered(
        Constraint::Length(50),
        Constraint::Length(lines.len() as u16 + 2),
    );
    let block = Block::bordered().title(" Cast to — Esc to close ");
    let inner = block.inner(popup);
    f.render_widget(Clear, popup);
    f.render_widget(block, popup);
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_minimized_player_layout(f: &mut Frame, s: &mut AppState, area: Rect) {
    let [main, bar] = Layout::vertical([Constraint::Min(3), Constraint::Length(6)]).areas(area);
    let [side, list] =
        Layout::horizontal([Constraint::Length(SIDEBAR_WIDTH), Constraint::Min(10)]).areas(main);

    draw_sidebar(f, s, side);
    draw_list(f, s, list);
    draw_minimized_player(f, s, bar);
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
    let focused = s.screen == Screen::Browse(Focus::Sidebar);
    let block = Block::bordered()
        .title(" Library ")
        .border_style(border_style(focused, s.accent));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let width = inner.width as usize;
    let lines: Vec<Line> = Section::ALL
        .into_iter()
        .map(|section| {
            let selected = section == s.browser.section;
            let marker = if selected { "> " } else { "  " };
            let name = section.title();
            let text = match section {
                Section::Queue => format!("{marker}{name} ({})", s.playback.queue.len()),
                _ => format!("{marker}{name}"),
            };
            let line = Line::raw(fit(&text, "", width));
            if selected {
                line.style(selected_style(focused, s.accent))
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
    let search = match &s.modal {
        Some(Modal::Search(q)) => Some(format!("Search: {q}▏")),
        _ => None,
    };
    let searching = search.is_some();
    let crumb = truncate_left(
        &search.unwrap_or_else(|| breadcrumb(&s.browser.stack)),
        title_width,
    );
    let focused = s.screen == Screen::Browse(Focus::List);
    let Some(v) = s.browser.stack.last_mut() else {
        return;
    };
    let is_queue = matches!(v.kind, ListKind::Queue);

    let count = match (v.total, v.loading) {
        _ if is_queue => format!(" {} ", s.playback.queue.len()),
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
    s.browser.list_height = height;
    let len = if is_queue {
        s.playback.queue.len()
    } else {
        v.items.len()
    };
    if len == 0 {
        let hint = if v.loading {
            "loading…"
        } else {
            match v.kind {
                ListKind::Search if searching => "type a query, Enter to search, Esc to cancel",
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
    let multi_disc = matches!(v.kind, ListKind::AlbumTracks(_))
        && matches!(&v.items, Items::Tracks(ts) if ts.iter().any(|t| t.disc > Some(1)));
    let lines: Vec<Line> = (v.offset..end)
        .map(|i| {
            let is_playing = item_is_playing(v, i, &s.playback.now, &s.playback.queue);
            let (left, right) = row(v, i, &s.playback.queue, multi_disc);
            if let Items::Search(items) = &v.items
                && let SearchItem::Header(h) = &items[i]
            {
                return Line::raw(fit(h, "", width))
                    .style(Style::new().fg(s.accent).add_modifier(Modifier::BOLD));
            }
            let marker = render_line_marker(v, i, is_playing);
            let line = Line::raw(fit(&format!("{marker}{left}"), &right, width));
            if i == v.selected {
                line.style(selected_style(focused, s.accent))
            } else if is_playing {
                line.style(Style::new().fg(s.accent))
            } else {
                line
            }
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}

fn render_line_marker(v: &View, i: usize, is_playing: bool) -> &'static str {
    if is_playing {
        "▶ "
    } else if i == v.selected {
        "> "
    } else {
        "  "
    }
}

fn item_is_playing(v: &View, i: usize, now_playing: &NowPlaying, queue: &Queue) -> bool {
    match &v.items {
        Items::Artists(a) => now_playing.artist_playing_now(&a[i].rating_key),
        Items::Albums(a) => now_playing.album_playing_now(&a[i].rating_key),
        Items::Tracks(ts) => match v.kind {
            ListKind::Queue => {
                let entry = &queue.entries[i];
                now_playing.track_playing_now(&entry.track.rating_key) && queue.is_current(entry.id)
            }
            _ => now_playing.track_playing_now(&ts[i].rating_key),
        },
        Items::Search(items) => match &items[i] {
            SearchItem::Header(_) => false,
            SearchItem::Artist(a) => now_playing.artist_playing_now(&a.rating_key),
            SearchItem::Album(a) => now_playing.album_playing_now(&a.rating_key),
            SearchItem::Track(t) => now_playing.track_playing_now(&t.rating_key),
        },
        _ => false,
    }
}

fn artist_row(a: &Artist) -> (String, String) {
    let left = a.title.clone();
    (left, String::new())
}

fn album_row(list_kind: &ListKind, a: &Album) -> (String, String) {
    let year = a.year.map_or("    ".to_owned(), |y| y.to_string());
    let left = match list_kind {
        ListKind::ArtistAlbums(_) => format!("{year}  {}", a.title),
        _ => format!("{year}  {} — {}", a.title, a.parent_title),
    };
    let right = a
        .leaf_count
        .map_or(String::new(), |n| format!("{n} tracks"));
    (left, right)
}

pub(super) fn track_row(list_kind: &ListKind, t: &Track, multi_disc: bool) -> (String, String) {
    let left = match list_kind {
        ListKind::AlbumTracks(_) => {
            let n = t.index.map_or(String::new(), |n| n.to_string());
            if multi_disc {
                let disc = t.disc.unwrap_or(1);
                format!("{disc}-{n:0>2}  {}", t.title)
            } else {
                format!("{n:>2}  {}", t.title)
            }
        }
        _ => format!("{} — {}", t.title, t.grandparent_title),
    };
    let right = t.duration_ms.map_or(String::new(), fmt_ms);

    (left, right)
}

fn row(v: &View, i: usize, queue: &Queue, multi_disc: bool) -> (String, String) {
    match &v.items {
        Items::Artists(a) => artist_row(&a[i]),
        Items::Albums(a) => album_row(&v.kind, &a[i]),
        Items::Tracks(tracks) => match &v.kind {
            ListKind::Queue => track_row(&v.kind, &queue.entries[i].track, false),
            _ => track_row(&v.kind, &tracks[i], multi_disc),
        },
        Items::Search(items) => match &items[i] {
            SearchItem::Header(_) => (String::new(), String::new()),
            SearchItem::Artist(a) => artist_row(a),
            SearchItem::Album(a) => album_row(&v.kind, a),
            SearchItem::Track(t) => track_row(&v.kind, t, false),
        },
        Items::Playlists(p) => {
            let p = &p[i];
            let count = p.leaf_count.map(|n| format!("{n} tracks"));
            let dur = p.duration_ms.map(fmt_ms);
            let right = [count, dur].into_iter().flatten().collect::<Vec<_>>();
            (p.title.clone(), right.join(" · "))
        }
    }
}

fn draw_help(f: &mut Frame) {
    let mut groups: Vec<(&str, Vec<String>)> = Vec::new();
    for b in BINDINGS
        .iter()
        .filter(|b| !SEARCH_INPUT_KEYS.contains(&b.action))
    {
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
    let popup = area.centered(
        Constraint::Length(cols as u16 * 39 + 2),
        Constraint::Length(rows as u16 + 2),
    );
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
