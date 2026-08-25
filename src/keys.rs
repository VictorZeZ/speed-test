use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// One central registry for every keyboard shortcut in the app.
///
/// Design rules:
/// 1. Every action MUST have at least one canonical binding on a
///    layout-independent key (function keys, Insert/Delete/Backspace, arrows,
///    Enter, Esc, Tab). The OS reports those as virtual keys, so they behave
///    identically no matter which keyboard language is active.
/// 2. Letter keys are OPTIONAL aliases for convenience. They go through
///    [`normalize_char`], which maps common non-Latin layouts back to US
///    positions, but they are never the only binding for an action.
/// 3. Resolution is scope-aware: a binding declared for `Scope::Monitor` only
///    fires while the monitor tab is open, so the same physical key can be
///    reused across tabs without collisions.
/// 4. To add or change any shortcut, edit [`KEYMAP`] — nothing else. Input
///    handling, footers and the help screen are generated from this table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Quit,
    CancelTest,
    RefreshConnection,
    TabNext,
    TabPrev,
    TabTest,
    TabMonitor,
    TabHistory,
    TabHelp,

    StartStopTest,
    ToggleContinuous,
    ProfileNext,
    ProfilePrev,

    MonitorToggle,
    ToggleGaming,
    EditTarget,
    ResetSession,

    HistoryUp,
    HistoryDown,
    DeleteEntry,
    ClearAll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Global,
    Test,
    Monitor,
    History,
}

impl Scope {
    pub fn label(self) -> &'static str {
        match self {
            Scope::Global => "GENERAL",
            Scope::Test => "SPEED TEST",
            Scope::Monitor => "MONITOR",
            Scope::History => "HISTORY",
        }
    }

    pub fn color(self) -> ratatui::style::Color {
        use ratatui::style::Color;
        match self {
            Scope::Global => Color::Yellow,
            Scope::Test => Color::Cyan,
            Scope::Monitor => Color::LightGreen,
            Scope::History => Color::Green,
        }
    }
}

pub struct KeyDef {
    pub action: Action,
    /// Canonical universal keys first, letter aliases after.
    pub keys: &'static [KeyCode],
    /// Human-readable names of `keys`, same order (used by help/footer).
    pub labels: &'static [&'static str],
    pub scope: Scope,
    pub desc: &'static str,
}

macro_rules! def {
    ($action:expr, $scope:expr, $desc:expr, $( ($key:expr, $label:expr) ),+ $(,)?) => {
        KeyDef {
            action: $action,
            keys: &[$($key),+],
            labels: &[$($label),+],
            scope: $scope,
            desc: $desc,
        }
    };
}

pub const KEYMAP: &[KeyDef] = &[
    // ---- global ----
    def!(Action::RefreshConnection, Scope::Global, "refresh IP",
        (KeyCode::F(5), "F5"), (KeyCode::Char('r'), "R")),
    def!(Action::Quit, Scope::Global, "quit",
        (KeyCode::F(10), "F10"), (KeyCode::Char('q'), "Q")),
    def!(Action::CancelTest, Scope::Global, "cancel / back",
        (KeyCode::Esc, "ESC")),
    def!(Action::TabNext, Scope::Global, "next tab",
        (KeyCode::Tab, "TAB")),
    def!(Action::TabPrev, Scope::Global, "previous tab",
        (KeyCode::BackTab, "SHIFT+TAB")),
    def!(Action::TabTest, Scope::Global, "open test tab",
        (KeyCode::F(1), "F1")),
    def!(Action::TabMonitor, Scope::Global, "open monitor tab",
        (KeyCode::F(2), "F2")),
    def!(Action::TabHistory, Scope::Global, "open history tab",
        (KeyCode::F(3), "F3")),
    def!(Action::TabHelp, Scope::Global, "open help tab",
        (KeyCode::F(4), "F4")),

    // ---- test tab ----
    def!(Action::StartStopTest, Scope::Test, "start test · stop continuous session",
        (KeyCode::Enter, "ENTER")),
    def!(Action::ToggleContinuous, Scope::Test, "single / loop",
        (KeyCode::Insert, "INS"), (KeyCode::Char('m'), "M")),
    def!(Action::ProfileNext, Scope::Test, "next profile",
        (KeyCode::Down, "DOWN"), (KeyCode::Char('p'), "P")),
    def!(Action::ProfilePrev, Scope::Test, "previous profile",
        (KeyCode::Up, "UP")),

    // ---- monitor tab ----
    def!(Action::MonitorToggle, Scope::Monitor, "start / stop continuous ping",
        (KeyCode::Enter, "ENTER")),
    def!(Action::ToggleGaming, Scope::Monitor, "gaming mode",
        (KeyCode::F(6), "F6"), (KeyCode::Char('g'), "G")),
    def!(Action::EditTarget, Scope::Monitor, "change target host",
        (KeyCode::F(7), "F7"), (KeyCode::Char('t'), "T")),
    def!(Action::ResetSession, Scope::Monitor, "reset session stats & events",
        (KeyCode::F(8), "F8"), (KeyCode::Char('c'), "C")),

    // ---- history tab ----
    def!(Action::HistoryUp, Scope::History, "previous entry",
        (KeyCode::Up, "UP"), (KeyCode::Char('k'), "K")),
    def!(Action::HistoryDown, Scope::History, "next entry",
        (KeyCode::Down, "DOWN"), (KeyCode::Char('j'), "J")),
    def!(Action::DeleteEntry, Scope::History, "delete selected entry",
        (KeyCode::Delete, "DEL"), (KeyCode::Char('d'), "D")),
    def!(Action::ClearAll, Scope::History, "clear entire history",
        (KeyCode::Backspace, "BACKSPACE"), (KeyCode::Char('x'), "X")),
];

fn normalize_char(c: char) -> char {
    let lower = c.to_lowercase().next().unwrap_or(c);
    match lower {
        // Russian ЙЦУКЕН → US position
        'й' => 'q', 'ц' => 'w', 'у' => 'e', 'к' => 'r', 'е' => 't',
        'н' => 'y', 'г' => 'u', 'ш' => 'i', 'щ' => 'o', 'з' => 'p',
        'ф' => 'a', 'ы' => 's', 'в' => 'd', 'а' => 'f', 'п' => 'g',
        'р' => 'h', 'о' => 'j', 'л' => 'k', 'д' => 'l',
        'я' => 'z', 'ч' => 'x', 'с' => 'c', 'м' => 'v', 'и' => 'b',
        'т' => 'n', 'ь' => 'm',
        other => other,
    }
}

/// Translate a raw key event into its logical character, applying the
/// non-Latin layout translation. Returns `None` for non-character keys.
pub fn logical_char(key: &KeyEvent) -> Option<char> {
    match key.code {
        KeyCode::Char(c) => Some(normalize_char(c)),
        _ => None,
    }
}

fn bindings_match(def_key: &KeyCode, key: &KeyEvent, lchar: Option<char>) -> bool {
    let matched = match def_key {
        KeyCode::Char(expected) => lchar.map(|c| c == *expected).unwrap_or(false),
        other => key.code == *other,
    };
    if !matched {
        return false;
    }
    // Modifier rules: ALT always rejects. Letter aliases accept bare or
    // SHIFT-modified presses (uppercase). Everything else must be unmodified;
    // BackTab legitimately arrives as SHIFT+TAB.
    if key.modifiers.contains(KeyModifiers::ALT)
        || key.modifiers.contains(KeyModifiers::CONTROL)
    {
        return false;
    }
    match def_key {
        KeyCode::BackTab => true,
        KeyCode::Char(_) => {
            key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT
        }
        _ => key.modifiers.is_empty(),
    }
}

/// Resolve a key event to an action. Bindings declared for `active` scope and
/// global bindings are considered; the first matching definition wins.
pub fn resolve(key: &KeyEvent, active: crate::app::Tab) -> Option<Action> {
    let lchar = logical_char(key);
    KEYMAP
        .iter()
        .filter(|def| def.scope == Scope::Global || def.scope == scope_of(active))
        .find(|def| def.keys.iter().any(|k| bindings_match(k, key, lchar)))
        .map(|def| def.action)
}

fn scope_of(tab: crate::app::Tab) -> Scope {
    match tab {
        crate::app::Tab::Test => Scope::Test,
        crate::app::Tab::Monitor => Scope::Monitor,
        crate::app::Tab::History => Scope::History,
        crate::app::Tab::Help => Scope::History, // help has no own bindings
    }
}

/// True when the key is Ctrl+C (layout-independent quit).
pub fn is_ctrl_c(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
}

/// Keys used while a text editor popup is open. These bypass the action map.
pub fn editor_key(key: &KeyEvent) -> EditorKey {
    match key.code {
        KeyCode::Enter => EditorKey::Confirm,
        KeyCode::Esc => EditorKey::Cancel,
        KeyCode::Backspace => EditorKey::Backspace,
        KeyCode::Char(c) => EditorKey::Char(c),
        _ => EditorKey::Ignored,
    }
}

pub enum EditorKey {
    Confirm,
    Cancel,
    Backspace,
    Char(char),
    Ignored,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Tab;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn universal_keys_work_regardless_of_layout() {
        assert_eq!(resolve(&key(KeyCode::F(5), KeyModifiers::NONE), Tab::Test), Some(Action::RefreshConnection));
        assert_eq!(resolve(&key(KeyCode::Insert, KeyModifiers::NONE), Tab::Test), Some(Action::ToggleContinuous));
        assert_eq!(resolve(&key(KeyCode::Enter, KeyModifiers::NONE), Tab::Test), Some(Action::StartStopTest));
        assert_eq!(resolve(&key(KeyCode::F(2), KeyModifiers::NONE), Tab::Help), Some(Action::TabMonitor));
        assert_eq!(resolve(&key(KeyCode::Delete, KeyModifiers::NONE), Tab::History), Some(Action::DeleteEntry));
        assert_eq!(resolve(&key(KeyCode::Esc, KeyModifiers::NONE), Tab::Test), Some(Action::CancelTest));
    }

    #[test]
    fn letters_match_across_translated_layouts() {
        // Physical M on Russian ЙЦУКЕН produces 'ь'.
        assert_eq!(resolve(&key(KeyCode::Char('ь'), KeyModifiers::NONE), Tab::Test), Some(Action::ToggleContinuous));
        // Physical R produces 'к' on Russian.
        assert_eq!(resolve(&key(KeyCode::Char('к'), KeyModifiers::NONE), Tab::Monitor), Some(Action::RefreshConnection));
        // English upper/lowercase both match.
        assert_eq!(resolve(&key(KeyCode::Char('M'), KeyModifiers::NONE), Tab::Test), Some(Action::ToggleContinuous));
        assert_eq!(resolve(&key(KeyCode::Char('m'), KeyModifiers::NONE), Tab::Test), Some(Action::ToggleContinuous));
    }

    #[test]
    fn scoped_keys_do_not_leak_across_tabs() {
        // Enter means different things per tab.
        assert_eq!(resolve(&key(KeyCode::Enter, KeyModifiers::NONE), Tab::Monitor), Some(Action::MonitorToggle));
        // F6 gaming exists only on monitor scope.
        assert_eq!(resolve(&key(KeyCode::F(6), KeyModifiers::NONE), Tab::Monitor), Some(Action::ToggleGaming));
        assert_eq!(resolve(&key(KeyCode::F(6), KeyModifiers::NONE), Tab::Test), None);
    }

    #[test]
    fn every_action_has_at_least_one_universal_binding() {
        for def in KEYMAP {
            let universal = def.keys.iter().any(|k| {
                matches!(
                    k,
                    KeyCode::F(_)
                        | KeyCode::Insert
                        | KeyCode::Delete
                        | KeyCode::Backspace
                        | KeyCode::Enter
                        | KeyCode::Esc
                        | KeyCode::Tab
                        | KeyCode::BackTab
                        | KeyCode::Up
                        | KeyCode::Down
                        | KeyCode::Left
                        | KeyCode::Right
                )
            });
            assert!(universal, "{:?} lacks a layout-independent binding", def.action);
        }
    }

    #[test]
    fn ctrl_c_detected_and_alt_rejected() {
        assert!(is_ctrl_c(&key(KeyCode::Char('c'), KeyModifiers::CONTROL)));
        assert_eq!(
            resolve(&key(KeyCode::Char('q'), KeyModifiers::ALT), Tab::Test),
            None
        );
    }
}
