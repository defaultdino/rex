use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::browser::Section;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Down,
    Up,
    Top,
    Bottom,
    HalfDown,
    HalfUp,
    Open,
    Play,
    PlayShuffle,
    Back,
    ToggleFocus,
    Jump(Section),
    Search,
    PlayPause,
    Next,
    Previous,
    SeekForward,
    SeekBack,
    VolumeUp,
    VolumeDown,
    Shuffle,
    Repeat,
    Append,
    PlayNext,
    Remove,
    MoveDown,
    MoveUp,
    Clear,
    Help,
    Quit,
    TogglePlayer,
    SearchSubmit,
    SearchCancel,
    #[cfg(feature = "cast")]
    Cast,
}

pub const GLOBAL_PLAYER_KEYS: &[Action] = &[
    Action::PlayPause,
    Action::Next,
    Action::Previous,
    Action::SeekForward,
    Action::SeekBack,
    Action::VolumeUp,
    Action::VolumeDown,
    Action::Shuffle,
    Action::Repeat,
    Action::Help,
    Action::Quit,
    Action::TogglePlayer,
    #[cfg(feature = "cast")]
    Action::Cast,
];

pub const NAV_KEYS: &[Action] = &[
    Action::Down,
    Action::Up,
    Action::Top,
    Action::Bottom,
    Action::HalfDown,
    Action::HalfUp,
    Action::Open,
    Action::Back,
    Action::ToggleFocus,
    Action::Jump(Section::Artists),
    Action::Jump(Section::Albums),
    Action::Jump(Section::Playlists),
    Action::Jump(Section::Search),
    Action::Jump(Section::Queue),
    Action::Search,
];

pub const LIST_KEYS: &[Action] = &[
    Action::Play,
    Action::PlayShuffle,
    Action::Append,
    Action::PlayNext,
];

pub const QUEUE_KEYS: &[Action] = &[
    Action::Remove,
    Action::Clear,
    Action::MoveDown,
    Action::MoveUp,
];

pub const SEARCH_INPUT_KEYS: &[Action] = &[Action::SearchSubmit, Action::SearchCancel];

pub const HELP_KEYS: &[Action] = &[Action::Help, Action::Quit, Action::Back];

#[cfg(feature = "cast")]
pub const CAST_KEYS: &[Action] = &[Action::Down, Action::Up, Action::Open, Action::Back];

impl Action {
    pub fn describe_short(self) -> &'static str {
        match self {
            Action::Play => "play now",
            Action::Remove => "remove",
            Action::MoveDown => "move down",
            Action::MoveUp => "move up",
            Action::Clear => "clear",
            _ => self.describe(),
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Action::Down => "move down",
            Action::Up => "move up",
            Action::Top => "top",
            Action::Bottom => "bottom",
            Action::HalfDown => "half page down",
            Action::HalfUp => "half page up",
            Action::Open => "open",
            Action::Back => "back",
            Action::ToggleFocus => "toggle sidebar / list",
            Action::Jump(_) => "jump to section",
            Action::Search => "search",
            Action::PlayPause => "play / pause",
            Action::Next => "next track",
            Action::Previous => "previous / restart",
            Action::SeekForward => "seek +10s",
            Action::SeekBack => "seek -10s",
            Action::VolumeUp => "volume up",
            Action::VolumeDown => "volume down",
            Action::Shuffle => "shuffle",
            Action::Repeat => "repeat off/all/one",
            Action::Append => "add to queue",
            Action::PlayNext => "play next",
            Action::Remove => "queue: remove",
            Action::MoveDown => "queue: move down",
            Action::MoveUp => "queue: move up",
            Action::Clear => "queue: clear",
            Action::Help => "help",
            Action::Quit => "quit",
            Action::Play => "play now (replace queue)",
            Action::PlayShuffle => "play shuffled (replace queue)",
            Action::TogglePlayer => "toggle player maximized/minimized",
            Action::SearchSubmit => "search",
            Action::SearchCancel => "cancel",
            #[cfg(feature = "cast")]
            Action::Cast => "cast to a device",
        }
    }
}

pub struct KeyBinding {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

pub struct Binding {
    pub keys: &'static [KeyBinding],
    pub action: Action,
    pub show_in_help_bar: bool,
}

const fn bind(action: Action, keys: &'static [KeyBinding]) -> Binding {
    Binding {
        keys,
        action,
        show_in_help_bar: false,
    }
}

const fn in_help_bar(mut binding: Binding) -> Binding {
    binding.show_in_help_bar = true;
    binding
}

const fn key(c: char) -> KeyBinding {
    KeyBinding {
        code: KeyCode::Char(c),
        mods: KeyModifiers::NONE,
    }
}

const fn code(code: KeyCode) -> KeyBinding {
    KeyBinding {
        code,
        mods: KeyModifiers::NONE,
    }
}

const fn ctrl(c: char) -> KeyBinding {
    KeyBinding {
        code: KeyCode::Char(c),
        mods: KeyModifiers::CONTROL,
    }
}

pub const BINDINGS: &[Binding] = &[
    bind(Action::Down, &[key('j'), code(KeyCode::Down)]),
    bind(Action::Up, &[key('k'), code(KeyCode::Up)]),
    bind(Action::Top, &[key('g'), code(KeyCode::Home)]),
    bind(Action::Bottom, &[key('G'), code(KeyCode::End)]),
    bind(Action::HalfDown, &[ctrl('d'), code(KeyCode::PageDown)]),
    bind(Action::HalfUp, &[ctrl('u'), code(KeyCode::PageUp)]),
    bind(
        Action::Open,
        &[key('l'), code(KeyCode::Right), code(KeyCode::Enter)],
    ),
    bind(
        Action::Back,
        &[
            key('h'),
            code(KeyCode::Left),
            code(KeyCode::Backspace),
            code(KeyCode::Esc),
        ],
    ),
    bind(Action::ToggleFocus, &[code(KeyCode::Tab)]),
    bind(Action::Jump(Section::Artists), &[key('1')]),
    bind(Action::Jump(Section::Albums), &[key('2')]),
    bind(Action::Jump(Section::Playlists), &[key('3')]),
    bind(Action::Jump(Section::Search), &[key('4')]),
    bind(Action::Jump(Section::Queue), &[key('5')]),
    in_help_bar(bind(Action::Search, &[key('/')])),
    bind(Action::PlayPause, &[key(' ')]),
    bind(Action::Next, &[key('>')]),
    bind(Action::Previous, &[key('<')]),
    bind(Action::SeekForward, &[key('.')]),
    bind(Action::SeekBack, &[key(',')]),
    bind(Action::VolumeUp, &[key('+')]),
    bind(Action::VolumeDown, &[key('-')]),
    bind(Action::Shuffle, &[key('s')]),
    bind(Action::Repeat, &[key('r')]),
    in_help_bar(bind(Action::Append, &[key('a')])),
    in_help_bar(bind(Action::PlayNext, &[key('n')])),
    in_help_bar(bind(Action::Remove, &[key('d')])),
    in_help_bar(bind(Action::MoveDown, &[key('J')])),
    in_help_bar(bind(Action::MoveUp, &[key('K')])),
    in_help_bar(bind(Action::Clear, &[key('D')])),
    in_help_bar(bind(Action::Help, &[key('?')])),
    bind(Action::Quit, &[key('q')]),
    in_help_bar(bind(Action::Play, &[key('P')])),
    in_help_bar(bind(Action::PlayShuffle, &[key('S')])),
    in_help_bar(bind(Action::SearchSubmit, &[code(KeyCode::Enter)])),
    in_help_bar(bind(Action::SearchCancel, &[code(KeyCode::Esc)])),
    bind(Action::TogglePlayer, &[key('m')]),
    #[cfg(feature = "cast")]
    bind(Action::Cast, &[key('c')]),
];

pub fn lookup(k: KeyEvent) -> Option<Action> {
    let mut mods = k.modifiers;
    // terminals report shifted characters like `G` or `?` with SHIFT set
    if matches!(k.code, KeyCode::Char(_)) {
        mods.remove(KeyModifiers::SHIFT);
    }
    BINDINGS
        .iter()
        .find(|b| b.keys.iter().any(|kb| kb.code == k.code && kb.mods == mods))
        .map(|b| b.action)
}

pub fn key_name(b: &Binding) -> String {
    b.keys.iter().map(key_label).collect::<Vec<_>>().join(" ")
}

fn key_label(k: &KeyBinding) -> String {
    let name = match k.code {
        KeyCode::Char(' ') => "Space".into(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Down => "↓".into(),
        KeyCode::Up => "↑".into(),
        KeyCode::Left => "←".into(),
        KeyCode::Right => "→".into(),
        KeyCode::Home => "Home".into(),
        KeyCode::End => "End".into(),
        KeyCode::PageDown => "PgDn".into(),
        KeyCode::PageUp => "PgUp".into(),
        KeyCode::Enter => "Enter".into(),
        KeyCode::Backspace => "Backspace".into(),
        KeyCode::Tab => "Tab".into(),
        KeyCode::Esc => "Esc".into(),
        KeyCode::F(n) => format!("F{n}"),
        other => format!("{other:?}"),
    };
    if k.mods.contains(KeyModifiers::CONTROL) {
        format!("Ctrl-{name}")
    } else {
        name
    }
}
