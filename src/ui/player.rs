use image::RgbImage;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Widget};
use unicode_width::UnicodeWidthStr;

use crate::app::now_playing::NowPlaying;
use crate::app::queue::{Queue, Repeat};
use crate::app::state::AppState;
use crate::ui::string_formatting::fit;

fn player_title(action: &str, accent: Color) -> Vec<Span<'static>> {
    vec![
        Span::raw(" Player <"),
        Span::styled("m", Style::new().fg(accent).add_modifier(Modifier::BOLD)),
        Span::raw(format!(" {action}>")),
    ]
}

pub fn draw_minimized_player(f: &mut Frame, s: &AppState, area: Rect) {
    let block = Block::bordered().title(player_title("maximize", s.accent));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let width = inner.width as usize;
    let now = &s.playback.now;

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
    if s.playback.queue.shuffle {
        right.push_str(" ⇄");
    }
    match s.playback.queue.repeat {
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

pub fn draw_maximized_player(f: &mut Frame, s: &AppState, area: Rect) {
    let block = Block::bordered().title(player_title("minimize", s.accent));
    f.render_widget(&block, area);

    match &s.playback.now.track {
        Some(_) => draw_non_empty_state(f, block.inner(area), s),
        None => draw_empty_state(f, block.inner(area)),
    }
}

fn draw_empty_state(f: &mut Frame, area: Rect) {
    let text = "quiet for now";
    let centered_area = area.centered(
        Constraint::Length(text.width() as u16),
        Constraint::Length(1),
    );
    f.render_widget(Paragraph::new(text).centered().bold(), centered_area)
}

fn draw_non_empty_state(f: &mut Frame, area: Rect, s: &AppState) {
    let now = &s.playback.now;
    let area_centered = area.inner(Margin::new(10, 1));

    let [cover_art, artist_info, progress_bar, actions] = Layout::vertical([
        Constraint::Min(20),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .spacing(1)
    .areas(area_centered);

    draw_cover_art(f, cover_art, s);
    draw_info(f, artist_info, now);
    draw_progress_bar(f, progress_bar, now, s.accent);
    draw_actions(f, actions, &s.playback.now, &s.playback.queue, s.accent)
}

fn draw_cover_art(f: &mut Frame, area: Rect, s: &AppState) {
    let thumb = s.playback.now.track.as_ref().map(|t| t.thumb.as_str());
    let art = s
        .playback
        .cover
        .as_ref()
        .filter(|c| Some(c.thumb.as_str()) == thumb)
        .and_then(|c| c.art.as_ref());
    let Some(art) = art else {
        let centered_art = area.centered(Constraint::Fill(1), Constraint::Length(1));
        f.render_widget(
            Paragraph::new("No cover art available").centered(),
            centered_art,
        );
        return;
    };
    // a terminal cell is about twice as tall as it is wide, so a square takes half as many rows
    let side = area.width.min(area.height.saturating_mul(2)) & !1;
    let square = area.centered(Constraint::Length(side), Constraint::Length(side / 2));
    f.render_widget(CoverArt(art), square);
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

fn draw_info(f: &mut Frame, area: Rect, now: &NowPlaying) {
    let [track_and_album, artist] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);

    let (track_text, album_and_artist_text) = match &now.track {
        None => (String::new(), String::new()),
        Some(t) => {
            if now.buffering {
                (String::from(" buffering…"), String::new())
            } else {
                (
                    t.title.to_owned(),
                    format!("{} · {}", t.parent_title, t.grandparent_title.clone()),
                )
            }
        }
    };

    f.render_widget(
        Paragraph::new(track_text).centered().bold(),
        track_and_album,
    );
    f.render_widget(Paragraph::new(album_and_artist_text).centered(), artist);
}

fn draw_progress_bar(f: &mut Frame, area: Rect, now: &NowPlaying, accent: Color) {
    let [time_stamp, bar] = Layout::horizontal([Constraint::Length(11), Constraint::Min(10)])
        .spacing(1)
        .areas(area);

    let width = bar.width as usize;
    let duration = now.track.as_ref().and_then(|t| t.duration_ms);

    let bar_spans = match duration {
        Some(d) if d > 0 && now.track.is_some() && width >= 5 => {
            let filled = ((now.position_ms.min(d) as f64 / d as f64) * width as f64) as usize;
            vec![
                Span::styled("━".repeat(filled), Style::new().fg(accent)),
                Span::raw("━".repeat(width - filled)),
            ]
        }
        _ => vec![],
    };
    let time = format!(
        "{} / {}",
        crate::fmt_ms(now.position_ms),
        now.track
            .as_ref()
            .and_then(|t| t.duration_ms)
            .map_or(crate::fmt_ms(0), crate::fmt_ms)
    );
    f.render_widget(time, time_stamp);
    f.render_widget(Line::from(bar_spans), bar);
}

fn draw_actions(f: &mut Frame, area: Rect, now: &NowPlaying, queue: &Queue, accent: Color) {
    let centered_area = area.centered_horizontally(Constraint::Max(6));
    let [shuffle, play_status, repeat] = Layout::horizontal([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
    ])
    .spacing(1)
    .areas(centered_area);

    let play_status_icon = match &now.track {
        None => "",
        Some(_) => {
            if now.paused {
                "‖"
            } else {
                "▶"
            }
        }
    };

    let (repeat_icon, repeat_active) = match queue.repeat {
        Repeat::Off => ("↻", false),
        Repeat::All => ("↻", true),
        Repeat::One => ("↻1", true),
    };

    f.render_widget(action_styling("⇄", accent, queue.shuffle), shuffle);
    f.render_widget(Line::raw(play_status_icon), play_status);
    f.render_widget(action_styling(repeat_icon, accent, repeat_active), repeat);

    let volume = format!("♪ {:.0}%", now.volume * 100.0);
    let volume_width = volume.width() as u16;
    if area.right().saturating_sub(volume_width) >= centered_area.right() + 2 {
        let [_, volume_area] =
            Layout::horizontal([Constraint::Fill(1), Constraint::Length(volume_width)]).areas(area);
        f.render_widget(Line::raw(volume).right_aligned(), volume_area);
    }
}

fn action_styling(content: &str, accent: Color, active: bool) -> Line<'_> {
    if active {
        Line::styled(content, Style::new().fg(accent).bold())
    } else {
        Line::raw(content)
    }
}
