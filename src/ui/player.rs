use image::RgbImage;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::crossterm::terminal::window_size;
use ratatui::layout::{Constraint, Flex, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Widget};
use unicode_width::UnicodeWidthStr;

use crate::app::browser::ListKind;
use crate::app::lyrics::Remote;
use crate::app::now_playing::NowPlaying;
use crate::app::queue::Repeat;
use crate::app::state::AppState;
use crate::plex::models::Track;
use crate::ui::string_formatting::{fit, truncate};
use crate::ui::track_row;

fn player_title(action: &str, accent: Color) -> Vec<Span<'static>> {
    vec![
        Span::raw(" Player <"),
        Span::styled("m", Style::new().fg(accent).add_modifier(Modifier::BOLD)),
        Span::raw(format!(" {action}>")),
    ]
}

const INFO_LINES: u16 = 4;

pub fn draw_minimized_player(f: &mut Frame, s: &AppState, area: Rect) {
    let block = Block::bordered().title(player_title("maximize", s.accent));
    let inner = block.inner(area).inner(Margin::new(1, 0));
    f.render_widget(block, area);
    f.render_widget(Paragraph::new(info_lines(s, inner.width as usize)), inner);
}

pub fn draw_maximized_player(f: &mut Frame, s: &AppState, area: Rect) {
    let block = Block::bordered().title(player_title("minimize", s.accent));
    let inner = block.inner(area).inner(Margin::new(1, 0));
    f.render_widget(block, area);
    let Some(track) = &s.playback.now.track else {
        f.render_widget(Paragraph::new(info_lines(s, inner.width as usize)), inner);
        return;
    };

    let cover = if track.thumb.is_empty() {
        None
    } else {
        let ratio = cell_ratio();
        let rows = (inner.height / 2)
            .min(16)
            .min((f32::from(inner.width / 3) / ratio) as u16);
        let cols = (f32::from(rows) * ratio).round() as u16;
        (rows >= 4).then_some((cols, rows))
    };

    let [header, _, list] = Layout::vertical([
        Constraint::Length(cover.map_or(INFO_LINES, |(_, rows)| rows.max(INFO_LINES))),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(inner);

    let info = match cover {
        Some((cols, _)) => {
            let [art_area, _, info] = Layout::horizontal([
                Constraint::Length(cols),
                Constraint::Length(2),
                Constraint::Fill(1),
            ])
            .areas(header);
            if let Some(art) = cover_art(s, track) {
                f.render_widget(CoverArt(art), art_area);
            }
            info
        }
        None => header,
    };
    let [info] = Layout::vertical([Constraint::Length(INFO_LINES)])
        .flex(Flex::Center)
        .areas(info);
    f.render_widget(Paragraph::new(info_lines(s, info.width as usize)), info);

    let queue = &s.playback.queue;
    let width = list.width as usize;
    let next = queue.current.map_or(0, |i| i + 1);

    if list.height < 2 || next >= queue.len() {
        return;
    }

    let mut lines = vec![Line::styled("up next", Modifier::DIM)];

    lines.extend(
        queue.entries[next..]
            .iter()
            .map(|e| {
                let (left, right) = track_row(&ListKind::Queue, &e.track, false);
                Line::raw(fit(&left, &right, width))
            })
            .take(list.height as usize - 1),
    );
    f.render_widget(Paragraph::new(lines), list);
}

/// cell height over cell width, from the terminal's pixel size when it reports one
fn cell_ratio() -> f32 {
    match window_size() {
        Ok(w) if w.width > 0 && w.height > 0 && w.columns > 0 && w.rows > 0 => {
            (f32::from(w.height) / f32::from(w.rows)) / (f32::from(w.width) / f32::from(w.columns))
        }
        _ => 2.0,
    }
}

/// the loaded cover, if it belongs to the current track
fn cover_art<'a>(s: &'a AppState, track: &Track) -> Option<&'a RgbImage> {
    s.playback
        .cover
        .as_ref()
        .filter(|c| c.thumb == track.thumb)
        .and_then(|c| c.art.as_ref())
}

/// pixelates the image into cells using half blocks: each cell shows two stacked pixels,
/// the top one as its foreground and the bottom one as its background
struct CoverArt<'a>(&'a RgbImage);

impl Widget for CoverArt<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let (iw, ih) = self.0.dimensions();
        let side = iw.min(ih);
        let (ox, oy) = ((iw - side) / 2, (ih - side) / 2);
        let (pw, ph) = (u32::from(area.width), u32::from(area.height) * 2);
        let pixel = |x: u32, y: u32| {
            let [r, g, b] = self.0.get_pixel(ox + x * side / pw, oy + y * side / ph).0;
            Color::Rgb(r, g, b)
        };
        for row in 0..area.height {
            for col in 0..area.width {
                let (x, y) = (u32::from(col), u32::from(row) * 2);
                if let Some(cell) = buf.cell_mut((area.x + col, area.y + row)) {
                    cell.set_symbol("▀")
                        .set_fg(pixel(x, y))
                        .set_bg(pixel(x, y + 1));
                }
            }
        }
    }
}

/// title, artist and album, progress and status, shared by both player views
fn info_lines(s: &AppState, width: usize) -> Vec<Line<'static>> {
    let now = &s.playback.now;
    let Some(track) = &now.track else {
        return vec![Line::raw("■ not playing")];
    };

    let artist_album: Vec<&str> = [&track.grandparent_title, &track.parent_title]
        .into_iter()
        .map(String::as_str)
        .filter(|p| !p.is_empty())
        .collect();

    vec![
        Line::styled(
            fit(
                &track.title,
                if now.buffering { "buffering…" } else { "" },
                width,
            ),
            Modifier::BOLD,
        ),
        Line::raw(truncate(&artist_album.join(" — "), width)),
        progress_line(now, track, width, s.accent),
        status_line(s),
    ]
}

fn lyrics(s: &AppState) -> Vec<Line<'static>> {
    let current_offset = s.playback.now.position_ms;
    let lyric_strings = match &s.playback.lyrics.lyrics {
        Remote::Idle => "not loading".to_string(),
        Remote::Loading => "Loading lyrics...".to_string(),
        Remote::Loaded(lyrics) => lyrics
            .lines
            .get(0)
            .map_or("ERROR".to_string(), |lyric_line| lyric_line.text.clone()),
    };

    vec![]
}

fn progress_line(now: &NowPlaying, track: &Track, width: usize, accent: Color) -> Line<'static> {
    let icon = if now.paused { "‖" } else { "▶" };
    let left = format!("{icon} {} ", crate::fmt_ms(now.position_ms));
    let right = track
        .duration_ms
        .map_or(String::new(), |d| format!(" {}", crate::fmt_ms(d)));
    let bar_width = width.saturating_sub(left.width() + right.width());
    let filled = match track.duration_ms {
        Some(d) if d > 0 => {
            ((now.position_ms.min(d) as f64 / d as f64) * bar_width as f64) as usize
        }
        _ => 0,
    };
    Line::from(vec![
        Span::raw(left),
        Span::styled("━".repeat(filled), accent),
        Span::raw("─".repeat(bar_width - filled)),
        Span::raw(right),
    ])
}

fn status_line(s: &AppState) -> Line<'static> {
    let queue = &s.playback.queue;
    let style = |active: bool| match active {
        true => Style::new().fg(s.accent).bold(),
        false => Style::new().add_modifier(Modifier::DIM),
    };
    let repeat = match queue.repeat {
        Repeat::Off => "repeat off",
        Repeat::All => "repeat all",
        Repeat::One => "repeat one",
    };
    let mut spans = vec![
        Span::styled("shuffle", style(queue.shuffle)),
        Span::raw(" · "),
        Span::styled(repeat, style(queue.repeat != Repeat::Off)),
        Span::raw(format!(" · vol {:.0}%", s.playback.now.volume * 100.0)),
    ];
    if s.debug_line {
        spans.push(Span::raw(format!(
            " · rss {:.1} MB",
            crate::rss_kb() as f64 / 1024.0
        )));
    }
    Line::from(spans)
}
