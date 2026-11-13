//! The terminal client's settings file: key bindings, colors and terminal options, in JSON.
//! It is read once when the workbench starts. A missing file means the defaults; a value
//! that cannot be used is reported in the status line and skipped, never fatal.
//!
//! Location, first match wins: `$LUUNA_TUI_CONFIG`; `$XDG_CONFIG_HOME/luuna/tui.json`;
//! `%APPDATA%\luuna\tui.json` on Windows; `~/.config/luuna/tui.json` elsewhere (macOS too).
use crate::keys::{self, Keymap};
use crate::theme::{self, Palette, ThemeConfig};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Optional: lets editors validate the file. Ignored by the client.
    #[serde(rename = "$schema")]
    pub schema: Option<String>,
    /// Action name → every key that triggers it. A listed action's defaults are replaced.
    pub keys: BTreeMap<String, Vec<String>>,
    pub theme: ThemeConfig,
    /// Ask the terminal for unambiguous key reports (kitty protocol: kitty, WezTerm, Ghostty,
    /// foot, iTerm2 with CSI u). Lets Shift+Enter and Ctrl+Enter arrive. Defaults to true.
    pub keyboard_enhancement: Option<bool>,
}

#[derive(Clone)]
pub struct Loaded {
    pub path: Option<PathBuf>,
    pub found: bool,
    pub project_path: Option<PathBuf>,
    pub keyboard_enhancement: bool,
    pub warnings: Vec<String>,
}

pub fn path() -> Option<PathBuf> {
    let var = |name: &str| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if let Some(path) = var("LUUNA_TUI_CONFIG") {
        return Some(path);
    }
    if let Some(dir) = var("XDG_CONFIG_HOME") {
        return Some(dir.join("luuna").join("tui.json"));
    }
    if cfg!(windows) {
        if let Some(dir) = var("APPDATA") {
            return Some(dir.join("luuna").join("tui.json"));
        }
    }
    var("HOME")
        .or_else(|| var("USERPROFILE"))
        .map(|home| home.join(".config").join("luuna").join("tui.json"))
}

/// Parses a settings document; problems become warnings and the rest still applies.
pub fn parse(text: &str) -> (Config, Vec<String>) {
    match serde_json::from_str::<Config>(text) {
        Ok(config) => (config, Vec::new()),
        Err(error) => (
            Config::default(),
            vec![format!("settings ignored: {error}")],
        ),
    }
}

/// The closest project overrides, stopping at the checkout boundary.
pub fn project_path(cwd: &std::path::Path) -> Option<PathBuf> {
    for root in cwd.ancestors() {
        let path = root.join(".luuna/tui.json");
        if path.is_file() {
            return Some(path);
        }
        if root.join(".git").exists() {
            break;
        }
    }
    None
}

fn overlay(base: &mut serde_json::Value, patch: serde_json::Value) {
    match (base, patch) {
        (serde_json::Value::Object(base), serde_json::Value::Object(patch)) => {
            for (key, value) in patch {
                overlay(base.entry(key).or_insert(serde_json::Value::Null), value);
            }
        }
        (base, value) => *base = value,
    }
}

fn project_override(config: Config, text: &str, user_text: Option<&str>) -> (Config, Vec<String>) {
    let (_, warnings) = parse(text);
    if !warnings.is_empty() {
        return (config, warnings);
    }
    let mut base = user_text
        .and_then(|text| serde_json::from_str::<serde_json::Value>(text).ok())
        .filter(|value| value.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    let patch = serde_json::from_str(text).expect("validated project settings");
    overlay(&mut base, patch);
    let (merged, warnings) = parse(&base.to_string());
    if warnings.is_empty() {
        (merged, warnings)
    } else {
        (config, warnings)
    }
}

/// Reads the settings file and installs its keymap and palette for the whole process.
pub fn load_and_install() -> Loaded {
    let path = path();
    let mut warnings = Vec::new();
    let user_text = path
        .as_ref()
        .and_then(|path| std::fs::read_to_string(path).ok());
    let (config, found) = match path.as_ref().map(std::fs::read_to_string) {
        Some(Ok(text)) => {
            let (config, mut problems) = parse(&text);
            warnings.append(&mut problems);
            (config, true)
        }
        Some(Err(error)) if error.kind() != std::io::ErrorKind::NotFound => {
            warnings.push(format!("settings unreadable: {error}"));
            (Config::default(), false)
        }
        _ => (Config::default(), false),
    };
    let project_path = std::env::current_dir()
        .ok()
        .and_then(|cwd| project_path(&cwd));
    let config = if let Some(project) = &project_path {
        match std::fs::read_to_string(project) {
            Ok(text) => {
                let (config, problems) = project_override(config, &text, user_text.as_deref());
                warnings.extend(
                    problems
                        .into_iter()
                        .map(|problem| format!("Project {problem}")),
                );
                config
            }
            Err(error) => {
                warnings.push(format!("Project settings unreadable: {error}"));
                config
            }
        }
    } else {
        config
    };
    let (keymap, mut key_warnings) = Keymap::build(&config.keys);
    warnings.append(&mut key_warnings);
    keys::install(keymap);
    let palette = Palette::configured(&config.theme, &mut warnings);
    theme::install(palette, config.theme.color);
    let keyboard_enhancement = match std::env::var("LUUNA_TUI_KEYBOARD_ENHANCEMENT")
        .ok()
        .as_deref()
    {
        Some("0" | "false" | "off") => false,
        Some("1" | "true" | "on") => true,
        _ => config.keyboard_enhancement.unwrap_or(true),
    };
    let loaded = Loaded {
        path,
        found,
        project_path,
        keyboard_enhancement,
        warnings,
    };
    let _ = LOADED.set(loaded.clone());
    loaded
}

static LOADED: std::sync::OnceLock<Loaded> = std::sync::OnceLock::new();

/// What the workbench loaded at startup, for Help; `None` before loading (tests).
pub fn loaded() -> Option<&'static Loaded> {
    LOADED.get()
}

/// A complete settings file holding every default, for `luuna-tui --print-config`.
pub fn defaults_json() -> String {
    let keys: serde_json::Map<String, serde_json::Value> = keys::ACTIONS
        .iter()
        .map(|(name, _, defaults, _)| ((*name).to_string(), serde_json::json!(defaults)))
        .collect();
    let p = Palette::CLASSIC;
    let hex = |c: ratatui::style::Color| match c {
        ratatui::style::Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        other => format!("{other}").to_lowercase(),
    };
    let style =
        |s: theme::Slot| serde_json::json!({"fg": hex(s.fg), "bg": hex(s.bg), "bold": s.bold});
    let value = serde_json::json!({
        "keyboard_enhancement": true,
        "theme": {
            "preset": "classic",
            "styles": {
                "base": style(p.base), "panel": style(p.panel), "accent": style(p.accent),
                "selected": style(p.selected), "chrome": style(p.chrome),
                "mnemonic": style(p.mnemonic), "title": style(p.title),
            },
            "tones": {
                "success": hex(p.success), "active": hex(p.active), "danger": hex(p.danger),
                "attention": hex(p.attention), "interrupted": hex(p.interrupted),
                "neutral": hex(p.neutral),
            },
        },
        "keys": keys,
    });
    serde_json::to_string_pretty(&value).expect("serializable") + "\n"
}

/// Persist one explicit setting, preserving every unrelated field. Invalid edits
/// never replace the existing document, including when its JSON is malformed.
pub fn set(path: &std::path::Path, key: &str, value: serde_json::Value) -> std::io::Result<()> {
    use std::io::{self, Write};
    let mut document: serde_json::Value = match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map_err(io::Error::other)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => serde_json::json!({}),
        Err(error) => return Err(error),
    };
    let parts: Vec<_> = key.split('.').collect();
    if parts.iter().any(|part| part.is_empty()) {
        return Err(io::Error::other(
            "Use a setting name such as theme.preset or keyboard_enhancement",
        ));
    }
    let mut cursor = &mut document;
    for part in &parts[..parts.len() - 1] {
        let object = cursor
            .as_object_mut()
            .ok_or_else(|| io::Error::other("The setting parent is not an object"))?;
        cursor = object.entry(*part).or_insert_with(|| serde_json::json!({}));
    }
    cursor
        .as_object_mut()
        .ok_or_else(|| io::Error::other("The setting parent is not an object"))?
        .insert(parts[parts.len() - 1].into(), value);
    let text = serde_json::to_string_pretty(&document).map_err(io::Error::other)? + "\n";
    let (config, warnings) = parse(&text);
    let (_, mut invalid) = Keymap::build(&config.keys);
    invalid.extend(warnings);
    Palette::configured(&config.theme, &mut invalid);
    if !invalid.is_empty() {
        return Err(io::Error::other(invalid.join("; ")));
    }
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."));
    std::fs::create_dir_all(parent)?;
    let temp = parent.join(format!(".tui-{}.tmp", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    let result = (|| {
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_overrides_merge_without_resetting_user_settings_and_stop_at_git_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("checkout");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("nested")).unwrap();
        std::fs::create_dir_all(dir.path().join(".luuna")).unwrap();
        std::fs::write(dir.path().join(".luuna/tui.json"), "{}").unwrap();
        assert!(project_path(&root.join("nested")).is_none());
        std::fs::create_dir_all(root.join(".luuna")).unwrap();
        std::fs::write(root.join(".luuna/tui.json"), "{}").unwrap();
        assert_eq!(
            project_path(&root.join("nested")),
            Some(root.join(".luuna/tui.json"))
        );
        let user = r#"{"keyboard_enhancement":false,"keys":{"quit":["Ctrl+Q"]},"theme":{"preset":"terminal"}}"#;
        let (config, _) = parse(user);
        let (merged, warnings) =
            project_override(config.clone(), r#"{"keys":{"help":["F1"]}}"#, Some(user));
        assert!(warnings.is_empty());
        assert_eq!(merged.keyboard_enhancement, Some(false));
        assert_eq!(merged.theme.preset.as_deref(), Some("terminal"));
        assert_eq!(merged.keys["quit"], vec!["Ctrl+Q"]);
        assert_eq!(merged.keys["help"], vec!["F1"]);
        let (kept, warnings) = project_override(config, r#"{"unknown":true}"#, Some(user));
        assert!(!warnings.is_empty());
        assert_eq!(kept.keyboard_enhancement, Some(false));
    }

    #[test]
    fn saving_creates_the_user_file_preserves_other_settings_and_refuses_bad_edits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config/luuna/tui.json");
        set(&path, "keyboard_enhancement", serde_json::json!(false)).unwrap();
        set(&path, "keys.quit", serde_json::json!(["Ctrl+Q"])).unwrap();
        let before = std::fs::read(&path).unwrap();
        let (config, warnings) = parse(std::str::from_utf8(&before).unwrap());
        assert!(warnings.is_empty());
        assert_eq!(config.keyboard_enhancement, Some(false));
        assert_eq!(config.keys["quit"], vec!["Ctrl+Q"]);
        assert!(set(&path, "keyboard_enhancement", serde_json::json!("invalid")).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(set(&path, "unknown", serde_json::json!(true)).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn the_printed_defaults_load_without_warnings() {
        let (config, warnings) = parse(&defaults_json());
        assert!(warnings.is_empty(), "{warnings:?}");
        let (_, key_warnings) = Keymap::build(&config.keys);
        assert!(key_warnings.is_empty(), "{key_warnings:?}");
        let mut theme_warnings = Vec::new();
        assert_eq!(
            Palette::configured(&config.theme, &mut theme_warnings),
            Palette::CLASSIC
        );
        assert!(theme_warnings.is_empty(), "{theme_warnings:?}");
    }

    #[test]
    fn malformed_settings_fall_back_with_a_warning() {
        let (config, warnings) = parse("{\"keys\": {\"quit\": \"q\"}}");
        assert!(config.keys.is_empty());
        assert_eq!(warnings.len(), 1);
        let (_, warnings) = parse("{\"colour\": {}}");
        assert!(warnings[0].contains("colour"), "{warnings:?}");
    }
}
