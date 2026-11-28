//! The workbench's key bindings. Every handler matches one canonical key per action (the
//! keys the workbench has always used); `Keymap::translate` turns whatever the developer
//! pressed into that canonical key, so a binding change never touches a handler. Defaults
//! add alternatives for keys macOS terminals do not deliver: F1–F12 (taken by the system
//! unless Fn is held), Option as Alt (Terminal.app and iTerm2 send symbols by default) and
//! Home/End/PageUp/PageDown (Fn+arrows, which Terminal.app keeps for its own scrollback).
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Chord {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

/// (name, canonical key, default bindings, what it does).
pub const ACTIONS: &[(&str, &str, &[&str], &str)] = &[
    (
        "interrupt",
        "ctrl+c",
        &["ctrl+c"],
        "Quit the client from anywhere",
    ),
    ("quit", "q", &["q"], "Quit (outside text fields)"),
    (
        "back",
        "esc",
        &["esc"],
        "Back, close, or stop waiting locally",
    ),
    ("help", "f1", &["f1", "?"], "Help"),
    ("commands", "f2", &["f2", ":"], "Every runtime command"),
    ("home", "f9", &["1"], "Home"),
    ("projects", "f3", &["f3", "2"], "Projects"),
    ("orbit", "f11", &["3"], "Orbit: every repository at once"),
    ("missions", "f4", &["f4", "4"], "Missions board"),
    (
        "refresh",
        "f5",
        &["f5", "ctrl+r"],
        "Refresh the current read",
    ),
    ("memory", "f6", &["f6", "5"], "Atlas memory"),
    ("prompts", "f7", &["f7", "6"], "Beacon prompts"),
    ("runs", "f8", &["f8", "7"], "Runs"),
    ("menu", "f10", &["f10", "m"], "Toggle the section menu"),
    ("up", "up", &["up", "k", "ctrl+p"], "Move up"),
    ("down", "down", &["down", "j", "ctrl+n"], "Move down"),
    (
        "left",
        "left",
        &["left", "h"],
        "Previous column, tab or form choice",
    ),
    (
        "right",
        "right",
        &["right", "l"],
        "Next column, tab or form choice",
    ),
    (
        "page-up",
        "pageup",
        &["pageup", "ctrl+b", "alt+up"],
        "Page up",
    ),
    (
        "page-down",
        "pagedown",
        &["pagedown", "ctrl+f", "alt+down"],
        "Page down",
    ),
    ("top", "home", &["home", "<"], "First row or top"),
    ("bottom", "end", &["end", ">"], "Last row or bottom"),
    ("next", "tab", &["tab"], "Next field, focus or filter"),
    (
        "previous",
        "backtab",
        &["backtab"],
        "Previous field or focus",
    ),
    ("open", "enter", &["enter"], "Open, answer or advance"),
    (
        "newline",
        "alt+enter",
        &["alt+enter", "shift+enter", "ctrl+j"],
        "Insert a line in a form field",
    ),
    (
        "submit",
        "ctrl+s",
        &["ctrl+s", "ctrl+enter"],
        "Submit a form or document",
    ),
    ("clear", "ctrl+u", &["ctrl+u"], "Clear the field"),
    (
        "delete",
        "backspace",
        &["backspace", "ctrl+h"],
        "Delete a character",
    ),
    ("search", "/", &["/"], "Search"),
    ("reload", "r", &["r"], "Refresh; Orbit: run a recipe"),
    ("new", "n", &["n"], "New project, ticket, memory or prompt"),
    ("edit", "e", &["e"], "Edit the inspected record"),
    ("select", "s", &["s"], "Setup; select context; Orbit: sync"),
    (
        "plan",
        "p",
        &["p"],
        "Plan; promote memory; open the run's plan",
    ),
    ("discover", "d", &["d"], "Discover; forget memory"),
    ("baseline", "b", &["b"], "Baseline; Orbit: sandbox plan"),
    ("components", "x", &["x"], "Components; Orbit: exec"),
    ("enable", "y", &["y"], "Enable components"),
    ("source", "o", &["o"], "Ticket source"),
    ("confirm", "c", &["c"], "Confirm; reconcile; cancel the run"),
    ("go-run", "g", &["g"], "Open the run"),
    ("approve", "a", &["a"], "Approve the plan; run actions"),
    ("implement", "i", &["i"], "Implement the approved plan"),
    ("follow", "f", &["f"], "Follow the run"),
    ("user-scope", "u", &["u"], "Include or exclude user memory"),
    ("clear-context", "z", &["z"], "Clear unsent plan context"),
];

/// Ignores what a terminal reports inconsistently: Shift on a character (the character
/// already carries it), Shift on BackTab, and the case of a Ctrl or Alt letter.
pub fn normalize(key: &KeyEvent) -> Chord {
    let mut modifiers =
        key.modifiers & (KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT);
    let mut code = key.code;
    match code {
        KeyCode::Char(c) => {
            modifiers.remove(KeyModifiers::SHIFT);
            if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
                code = KeyCode::Char(c.to_ascii_lowercase());
            }
        }
        KeyCode::BackTab => modifiers.remove(KeyModifiers::SHIFT),
        _ => {}
    }
    Chord { code, modifiers }
}

/// A character to insert: no Ctrl, or Ctrl+Alt together (AltGr on Windows layouts).
pub fn is_text(key: &KeyEvent) -> bool {
    let KeyCode::Char(c) = key.code else {
        return false;
    };
    !c.is_control()
        && (!key.modifiers.contains(KeyModifiers::CONTROL)
            || key.modifiers.contains(KeyModifiers::ALT))
}

pub fn parse(spec: &str) -> Result<Chord, String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err("empty key".into());
    }
    // "+" alone, or a chord ending in "+" ("ctrl++"), names the plus key.
    let (prefix, last) = match spec.strip_suffix("++") {
        Some(rest) => (rest, "+"),
        None if spec == "+" => ("", "+"),
        None => spec.rsplit_once('+').unwrap_or(("", spec)),
    };
    let mut modifiers = KeyModifiers::NONE;
    for part in prefix.split('+').filter(|p| !p.is_empty()) {
        modifiers |= match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => KeyModifiers::CONTROL,
            "alt" | "option" | "opt" | "meta" => KeyModifiers::ALT,
            "shift" => KeyModifiers::SHIFT,
            "cmd" | "command" | "super" => {
                return Err(format!(
                    "{spec:?}: terminals do not receive Cmd/Super; use ctrl or alt"
                ))
            }
            other => return Err(format!("unknown modifier {other:?} in {spec:?}")),
        };
    }
    let lower = last.to_ascii_lowercase();
    let code = match lower.as_str() {
        "esc" | "escape" => KeyCode::Esc,
        "enter" | "return" => KeyCode::Enter,
        "tab" => KeyCode::Tab,
        "backtab" => KeyCode::BackTab,
        "backspace" | "bs" => KeyCode::Backspace,
        "delete" | "del" => KeyCode::Delete,
        "insert" | "ins" => KeyCode::Insert,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" | "pgup" => KeyCode::PageUp,
        "pagedown" | "pgdn" | "pgdown" => KeyCode::PageDown,
        "space" => KeyCode::Char(' '),
        "plus" => KeyCode::Char('+'),
        f if f.len() > 1 && f.starts_with('f') && f[1..].chars().all(|c| c.is_ascii_digit()) => {
            match f[1..].parse::<u8>() {
                Ok(n @ 1..=24) => KeyCode::F(n),
                _ => return Err(format!("unknown key {last:?} in {spec:?}")),
            }
        }
        _ => {
            let mut chars = last.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => KeyCode::Char(c),
                _ => return Err(format!("unknown key {last:?} in {spec:?}")),
            }
        }
    };
    if modifiers.contains(KeyModifiers::SHIFT) && code == KeyCode::Tab {
        return Ok(Chord {
            code: KeyCode::BackTab,
            modifiers: modifiers - KeyModifiers::SHIFT,
        });
    }
    Ok(normalize(&KeyEvent::new(code, modifiers)))
}

/// The display form used in hints and Help: "Ctrl+S", "F2", "?".
pub fn label(chord: &Chord) -> String {
    let mut out = String::new();
    for (flag, name) in [
        (KeyModifiers::CONTROL, "Ctrl+"),
        (KeyModifiers::ALT, "Alt+"),
        (KeyModifiers::SHIFT, "Shift+"),
    ] {
        if chord.modifiers.contains(flag) {
            out.push_str(name);
        }
    }
    match chord.code {
        KeyCode::Char(' ') => out.push_str("Space"),
        KeyCode::Char(c) if !chord.modifiers.is_empty() => out.push(c.to_ascii_uppercase()),
        KeyCode::Char(c) => out.push(c),
        KeyCode::F(n) => out.push_str(&format!("F{n}")),
        KeyCode::Esc => out.push_str("Esc"),
        KeyCode::Enter => out.push_str("Enter"),
        KeyCode::Tab => out.push_str("Tab"),
        KeyCode::BackTab => out.push_str("Shift+Tab"),
        KeyCode::Backspace => out.push_str("Backspace"),
        KeyCode::PageUp => out.push_str("PgUp"),
        KeyCode::PageDown => out.push_str("PgDn"),
        KeyCode::Up => out.push('↑'),
        KeyCode::Down => out.push('↓'),
        KeyCode::Left => out.push('←'),
        KeyCode::Right => out.push('→'),
        other => out.push_str(&format!("{other:?}")),
    }
    out
}

pub struct Keymap {
    canonical: Vec<Chord>,
    bound: Vec<Vec<Chord>>,
    lookup: HashMap<Chord, usize>,
    canonicals: HashSet<Chord>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self::build(&BTreeMap::new()).0
    }
}

impl Keymap {
    /// Overrides replace an action's whole list (an empty list unbinds it). Returns the
    /// problems found; a bad entry is skipped, never fatal.
    pub fn build(overrides: &BTreeMap<String, Vec<String>>) -> (Self, Vec<String>) {
        let mut warnings = Vec::new();
        for name in overrides.keys() {
            if !ACTIONS.iter().any(|(action, ..)| action == name) {
                warnings.push(format!("keys: unknown action {name:?}"));
            }
        }
        let mut map = Self {
            canonical: Vec::new(),
            bound: Vec::new(),
            lookup: HashMap::new(),
            canonicals: HashSet::new(),
        };
        let mut owner: HashMap<Chord, &str> = HashMap::new();
        for (index, (name, canonical, defaults, _)) in ACTIONS.iter().enumerate() {
            let canonical = parse(canonical).expect("built-in canonical key");
            map.canonical.push(canonical);
            map.canonicals.insert(canonical);
            let specs: Vec<String> = match overrides.get(*name) {
                Some(list) => list.clone(),
                None => defaults.iter().map(|s| s.to_string()).collect(),
            };
            let mut chords = Vec::new();
            for spec in specs {
                match parse(&spec) {
                    Ok(chord) => {
                        if let Some(previous) = owner.insert(chord, name) {
                            warnings.push(format!(
                                "keys: {spec:?} is bound to both {previous} and {name}; {name} wins"
                            ));
                            if let Some(list) = ACTIONS
                                .iter()
                                .position(|(a, ..)| *a == previous)
                                .and_then(|i| map.bound.get_mut(i))
                            {
                                list.retain(|c| *c != chord);
                            }
                        }
                        map.lookup.insert(chord, index);
                        chords.push(chord);
                    }
                    Err(error) => warnings.push(format!("keys.{name}: {error}")),
                }
            }
            map.bound.push(chords);
        }
        (map, warnings)
    }

    /// The canonical key for what was pressed; `None` when the pressed key is some
    /// action's canonical key that the configuration moved elsewhere. In a text field a
    /// character is always text, so letters never trigger actions while typing.
    pub fn translate(&self, key: KeyEvent, text_entry: bool) -> Option<KeyEvent> {
        if text_entry && is_text(&key) {
            return Some(key);
        }
        let chord = normalize(&key);
        if let Some(&index) = self.lookup.get(&chord) {
            // Always the canonical event: a terminal's Ctrl+Shift+S must reach the Ctrl+S handler.
            let canonical = self.canonical[index];
            return Some(KeyEvent {
                code: canonical.code,
                modifiers: canonical.modifiers,
                kind: key.kind,
                state: key.state,
            });
        }
        if self.canonicals.contains(&chord) {
            return None;
        }
        // A modified form of a named canonical key whose action was unbound or moved (Ctrl+Right
        // after `"right": []`, or once Right opens instead) must not reach that action's handler
        // either: it passes only while the key still belongs to its own action.
        if !chord.modifiers.is_empty() && !matches!(chord.code, KeyCode::Char(_)) {
            let base = Chord {
                code: chord.code,
                modifiers: KeyModifiers::NONE,
            };
            let own = self.lookup.get(&base).map(|&i| self.canonical[i]) == Some(base);
            if self.canonicals.contains(&base) && !own {
                return None;
            }
        }
        Some(key)
    }

    pub fn bindings(&self, action: &str) -> &[Chord] {
        ACTIONS
            .iter()
            .position(|(name, ..)| *name == action)
            .map(|i| self.bound[i].as_slice())
            .unwrap_or(&[])
    }

    /// The first binding's label, for hints; "unbound" when there is none.
    pub fn first(&self, action: &str) -> String {
        self.bindings(action)
            .first()
            .map(label)
            .unwrap_or_else(|| "unbound".into())
    }

    /// The binding to show in a hint: the first one that is not a function key (macOS keeps
    /// F1-F12 unless Fn is held), else the first; "unbound" when there is none.
    pub fn hint(&self, action: &str) -> String {
        let bound = self.bindings(action);
        bound
            .iter()
            .find(|chord| !matches!(chord.code, KeyCode::F(_)))
            .or_else(|| bound.first())
            .map(label)
            .unwrap_or_else(|| "unbound".into())
    }

    /// Every binding's label, joined with " / ".
    pub fn all(&self, action: &str) -> String {
        let labels: Vec<String> = self.bindings(action).iter().map(label).collect();
        if labels.is_empty() {
            "unbound".into()
        } else {
            labels.join(" / ")
        }
    }
}

static KEYMAP: OnceLock<Keymap> = OnceLock::new();

/// Installs the configured keymap once, at startup; later calls are ignored.
pub fn install(keymap: Keymap) {
    let _ = KEYMAP.set(keymap);
}

/// The installed keymap, or the defaults when none was installed (tests, one-shot commands).
pub fn keymap() -> &'static Keymap {
    KEYMAP.get_or_init(Keymap::default)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn every_builtin_binding_parses_and_defaults_are_conflict_free() {
        let (map, warnings) = Keymap::build(&BTreeMap::new());
        assert!(warnings.is_empty(), "{warnings:?}");
        for (name, ..) in ACTIONS {
            assert!(!map.bindings(name).is_empty(), "{name} has no binding");
        }
    }

    #[test]
    fn mac_alternatives_reach_the_canonical_keys() {
        let map = Keymap::default();
        let t = |code, m| {
            map.translate(press(code, m), false)
                .map(|k| (k.code, k.modifiers))
        };
        assert_eq!(
            t(KeyCode::Char('?'), KeyModifiers::SHIFT),
            Some((KeyCode::F(1), KeyModifiers::NONE))
        );
        assert_eq!(
            t(KeyCode::Char(':'), KeyModifiers::SHIFT),
            Some((KeyCode::F(2), KeyModifiers::NONE))
        );
        assert_eq!(
            t(KeyCode::Char('m'), KeyModifiers::NONE),
            Some((KeyCode::F(10), KeyModifiers::NONE))
        );
        assert_eq!(
            t(KeyCode::Char('j'), KeyModifiers::CONTROL),
            Some((KeyCode::Enter, KeyModifiers::ALT))
        );
        assert_eq!(
            t(KeyCode::Enter, KeyModifiers::SHIFT),
            Some((KeyCode::Enter, KeyModifiers::ALT))
        );
        assert_eq!(
            t(
                KeyCode::Char('F'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT
            ),
            Some((KeyCode::PageDown, KeyModifiers::NONE))
        );
        assert_eq!(
            t(KeyCode::Char('h'), KeyModifiers::CONTROL),
            Some((KeyCode::Backspace, KeyModifiers::NONE))
        );
        assert_eq!(
            t(KeyCode::BackTab, KeyModifiers::SHIFT).map(|k| k.0),
            Some(KeyCode::BackTab)
        );
    }

    #[test]
    fn text_fields_keep_characters_and_still_honour_chords() {
        let map = Keymap::default();
        for c in ['q', '?', '1', 'm', 'j'] {
            let key = press(KeyCode::Char(c), KeyModifiers::NONE);
            assert_eq!(map.translate(key, true), Some(key));
        }
        let altgr = press(
            KeyCode::Char('@'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        );
        assert!(is_text(&altgr));
        assert_eq!(map.translate(altgr, true), Some(altgr));
        let newline = map
            .translate(press(KeyCode::Char('j'), KeyModifiers::CONTROL), true)
            .unwrap();
        assert_eq!(
            (newline.code, newline.modifiers),
            (KeyCode::Enter, KeyModifiers::ALT)
        );
        assert!(!is_text(&press(KeyCode::Char('s'), KeyModifiers::CONTROL)));
    }

    #[test]
    fn rebinding_moves_an_action_and_frees_its_old_key() {
        let overrides = BTreeMap::from([
            ("quit".to_string(), vec!["ctrl+q".to_string()]),
            ("help".to_string(), vec![]),
        ]);
        let (map, warnings) = Keymap::build(&overrides);
        assert!(warnings.is_empty(), "{warnings:?}");
        let q = map
            .translate(press(KeyCode::Char('q'), KeyModifiers::CONTROL), false)
            .unwrap();
        assert_eq!(q.code, KeyCode::Char('q'));
        assert_eq!(q.modifiers, KeyModifiers::NONE);
        assert_eq!(
            map.translate(press(KeyCode::Char('q'), KeyModifiers::NONE), false),
            None
        );
        assert_eq!(
            map.translate(press(KeyCode::F(1), KeyModifiers::NONE), false),
            None
        );
        assert_eq!(map.first("help"), "unbound");
    }

    #[test]
    fn unbinding_a_named_key_also_stops_its_modified_forms() {
        let (map, _) = Keymap::build(&BTreeMap::from([("right".to_string(), vec![])]));
        for (modifiers, text_entry) in [
            (KeyModifiers::NONE, false),
            (KeyModifiers::CONTROL, false),
            (KeyModifiers::SHIFT, true),
            (KeyModifiers::ALT, true),
        ] {
            assert_eq!(
                map.translate(press(KeyCode::Right, modifiers), text_entry),
                None,
                "{modifiers:?}"
            );
        }
        // Moved rather than unbound: the new chord reaches the handler as the canonical key.
        let (moved, _) = Keymap::build(&BTreeMap::from([(
            "right".to_string(),
            vec!["ctrl+right".into()],
        )]));
        let right = moved
            .translate(press(KeyCode::Right, KeyModifiers::CONTROL), true)
            .unwrap();
        assert_eq!(
            (right.code, right.modifiers),
            (KeyCode::Right, KeyModifiers::NONE)
        );
        assert_eq!(
            moved.translate(press(KeyCode::Right, KeyModifiers::SHIFT), true),
            None
        );
        // Given to another action: Right opens, and its modified forms never pick or page.
        let (taken, warnings) = Keymap::build(&BTreeMap::from([
            ("right".to_string(), vec!["l".into()]),
            ("open".to_string(), vec!["enter".into(), "right".into()]),
        ]));
        assert!(warnings.is_empty(), "{warnings:?}");
        let opened = taken
            .translate(press(KeyCode::Right, KeyModifiers::NONE), false)
            .unwrap();
        assert_eq!(opened.code, KeyCode::Enter);
        for modifiers in [KeyModifiers::SHIFT, KeyModifiers::CONTROL] {
            assert_eq!(
                taken.translate(press(KeyCode::Right, modifiers), true),
                None
            );
        }
        // With the defaults a modified arrow still passes through untouched.
        let (defaults, _) = Keymap::build(&BTreeMap::new());
        let passed = defaults
            .translate(press(KeyCode::Right, KeyModifiers::CONTROL), true)
            .unwrap();
        assert_eq!(passed.modifiers, KeyModifiers::CONTROL);
    }

    #[test]
    fn bad_entries_warn_without_failing() {
        let overrides = BTreeMap::from([
            ("nope".to_string(), vec!["x".to_string()]),
            (
                "up".to_string(),
                vec!["hyper+k".to_string(), "w".to_string()],
            ),
            ("down".to_string(), vec!["w".to_string()]),
        ]);
        let (map, warnings) = Keymap::build(&overrides);
        assert_eq!(warnings.len(), 3, "{warnings:?}");
        assert!(map.bindings("up").is_empty());
        assert_eq!(map.all("down"), "w");
    }

    #[test]
    fn parse_and_label_round_trip() {
        for (spec, shown) in [
            ("ctrl+s", "Ctrl+S"),
            ("F10", "F10"),
            ("shift+tab", "Shift+Tab"),
            ("option+enter", "Alt+Enter"),
            ("space", "Space"),
            ("ctrl++", "Ctrl++"),
            ("?", "?"),
        ] {
            assert_eq!(label(&parse(spec).unwrap()), shown, "{spec}");
        }
        assert!(parse("f99").is_err());
        assert!(parse("ctrl+").is_err());
    }
}
