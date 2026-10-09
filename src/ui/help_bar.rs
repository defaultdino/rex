use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::keys::{Action, BINDINGS, key_name};
use crate::app::state::{AppState, Focus, ListKind};

const SEARCH_INPUT_KEYS: &[Action] = &[Action::SearchSubmit, Action::SearchCancel];

const SIDEBAR_KEYS: &[Action] = &[Action::Open, Action::Search, Action::Help];

const LIST_KEYS: &[Action] = &[
    Action::Open,
    Action::Play,
    Action::Append,
    Action::PlayNext,
    Action::Search,
    Action::Help,
];

const QUEUE_KEYS: &[Action] = &[
    Action::Remove,
    Action::Clear,
    Action::MoveDown,
    Action::MoveUp,
    Action::Search,
    Action::Help,
];

fn get_actions(s: &AppState) -> &[Action] {
    if s.search_input.is_some() {
        return SEARCH_INPUT_KEYS;
    }
    match s.focus {
        Focus::Sidebar => SIDEBAR_KEYS,
        Focus::List => match s.view() {
            Some(v) => match v.kind {
                ListKind::Queue => QUEUE_KEYS,
                _ => LIST_KEYS,
            },
            None => &[],
        },
    }
}

pub fn draw_help_bar(f: &mut Frame, s: &AppState, area: Rect) {
    let key_style = Style::new().fg(s.accent).add_modifier(Modifier::BOLD);
    let spans: Vec<Span> = get_actions(s)
        .iter()
        .filter_map(|&action| {
            BINDINGS
                .iter()
                .find(|b| b.action == action)
                .map(|b| (b, action))
        })
        .flat_map(|(b, action)| {
            [
                Span::styled(key_name(b), key_style),
                Span::raw(format!(" {}   ", action.describe_short())),
            ]
        })
        .collect();
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}
