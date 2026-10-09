use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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
    Back,
    ToggleFocus,
    Jump(usize),
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
    DebugLine,
    Quit,
    SearchSubmit,
    SearchCancel,
}

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
            Action::DebugLine => "memory usage",
            Action::Quit => "quit",
            Action::Play => "play now (replace queue)",
            Action::SearchSubmit => "search",
            Action::SearchCancel => "cancel",
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
    pub show_in_help: bool,
}

const fn bind(action: Action, keys: &'static [KeyBinding]) -> Binding {
    Binding {
        keys,
        action,
        show_in_help: true,
    }
}

const fn hidden(mut binding: Binding) -> Binding {
    binding.show_in_help = false;
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
        &[key('h'), code(KeyCode::Left), code(KeyCode::Backspace)],
    ),
    bind(Action::ToggleFocus, &[code(KeyCode::Tab)]),
    bind(Action::Jump(0), &[key('1')]),
    bind(Action::Jump(1), &[key('2')]),
    bind(Action::Jump(2), &[key('3')]),
    bind(Action::Jump(3), &[key('4')]),
    bind(Action::Jump(4), &[key('5')]),
    hidden(bind(Action::Search, &[key('/')])),
    bind(Action::PlayPause, &[key(' ')]),
    bind(Action::Next, &[key('>')]),
    bind(Action::Previous, &[key('<')]),
    bind(Action::SeekForward, &[key('.')]),
    bind(Action::SeekBack, &[key(',')]),
    bind(Action::VolumeUp, &[key('+')]),
    bind(Action::VolumeDown, &[key('-')]),
    bind(Action::Shuffle, &[key('s')]),
    bind(Action::Repeat, &[key('r')]),
    hidden(bind(Action::Append, &[key('a')])),
    hidden(bind(Action::PlayNext, &[key('n')])),
    hidden(bind(Action::Remove, &[key('d')])),
    hidden(bind(Action::MoveDown, &[key('J')])),
    hidden(bind(Action::MoveUp, &[key('K')])),
    hidden(bind(Action::Clear, &[key('D')])),
    hidden(bind(Action::Help, &[key('?')])),
    bind(Action::DebugLine, &[code(KeyCode::F(12))]),
    bind(Action::Quit, &[key('q')]),
    hidden(bind(Action::Play, &[key('p')])),
    hidden(bind(Action::SearchSubmit, &[code(KeyCode::Enter)])),
    hidden(bind(Action::SearchCancel, &[code(KeyCode::Esc)])),
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
