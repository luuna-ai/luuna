//! Pure decision logic for the Claude Code `PreToolUse` Roots hook.
//!
//! A malformed hook payload deliberately fails open.  The hook is a defense
//! against accidental writes into human intent, not an authorization system.

use crate::boundary::real_or_self;
use serde::de::Deserializer;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use std::path::{Component, Path, PathBuf};

pub const DENY_REASON: &str = "roots: .luuna/roots/human/ holds human-written intent; agents never edit it. Ask a question with `luuna-roots ask`, propose with `luuna-roots propose`/`luuna-roots sprout`, or attach material with `luuna-roots note`.";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Allow,
    Deny,
}

impl Decision {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GuardDecision {
    pub decision: Decision,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The original path or command presented by the hook.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

impl GuardDecision {
    fn allow(target: Option<String>) -> Self {
        Self {
            decision: Decision::Allow,
            reason: None,
            target,
        }
    }

    fn deny(reason: String, target: String) -> Self {
        Self {
            decision: Decision::Deny,
            reason: Some(reason),
            target: Some(target),
        }
    }
}

/// A tolerant view of the untrusted hook JSON.  Every field is retained as a
/// JSON value so a wrong type becomes an allowed no-op rather than a parsing
/// failure that blocks a Claude session.
#[derive(Clone, Debug, Default)]
pub struct HookInput {
    pub tool_name: Value,
    pub tool_input: Value,
    pub cwd: Value,
}

impl HookInput {
    #[must_use]
    pub fn from_value(value: &Value) -> Self {
        let Some(object) = value.as_object() else {
            return Self::default();
        };
        Self {
            tool_name: object.get("tool_name").cloned().unwrap_or(Value::Null),
            tool_input: object.get("tool_input").cloned().unwrap_or(Value::Null),
            cwd: object.get("cwd").cloned().unwrap_or(Value::Null),
        }
    }
}

impl<'de> Deserialize<'de> for HookInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(Self::from_value(&Value::deserialize(deserializer)?))
    }
}

/// Parse stdin for the hook command.  Invalid JSON is represented as `None`
/// so the command layer can log a short fail-open note and exit successfully.
#[must_use]
pub fn parse_hook_input(payload: &str) -> Option<HookInput> {
    serde_json::from_str::<Value>(payload)
        .ok()
        .map(|value| HookInput::from_value(&value))
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    let absolute = path.is_absolute();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push(Path::new(std::path::MAIN_SEPARATOR_STR)),
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() && !absolute {
                    out.push("..");
                }
            }
            Component::Normal(part) => out.push(part),
        }
    }
    out
}

fn resolve_from(cwd: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        lexical_normalize(path)
    } else {
        lexical_normalize(&cwd.join(path))
    }
}

fn lower_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase()
}

/// True for an absolute, normalized path under either supported human store:
/// `.luuna/roots/human` or the legacy `.roots/human`.
#[must_use]
pub fn lexically_human(path: impl AsRef<Path>) -> bool {
    let normalized = lower_path(path.as_ref());
    let components = normalized
        .split('/')
        .filter(|component| !component.is_empty())
        .collect::<Vec<_>>();
    components
        .windows(3)
        .any(|window| window == [".luuna", "roots", "human"])
        || components
            .windows(2)
            .any(|window| window == [".roots", "human"])
}

fn is_link(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink())
}

/// Resolve the place a write would land, following a final (including
/// dangling) symlink and canonicalizing the deepest existing ancestor.
#[must_use]
pub fn resolve_write_target(path: impl AsRef<Path>) -> PathBuf {
    let mut current = resolve_from(
        &std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        path.as_ref(),
    );
    for _ in 0..40 {
        if !is_link(&current) {
            break;
        }
        let Ok(link) = fs::read_link(&current) else {
            break;
        };
        let parent = current.parent().unwrap_or_else(|| Path::new(""));
        current = if link.is_absolute() {
            resolve_from(Path::new(""), &link)
        } else {
            resolve_from(parent, &link)
        };
    }

    let mut tail = Vec::new();
    let mut probe = current.clone();
    while !probe.exists() {
        let Some(parent) = probe.parent().map(Path::to_path_buf) else {
            return current;
        };
        if parent == probe {
            return current;
        }
        if let Some(name) = probe.file_name() {
            tail.push(name.to_os_string());
        }
        probe = parent;
    }
    let mut resolved = real_or_self(&probe);
    for name in tail.into_iter().rev() {
        resolved.push(name);
    }
    resolved
}

/// True when writing `path`, relative to `cwd`, would touch human-owned data.
#[must_use]
pub fn is_protected_path(path: &str, cwd: impl AsRef<Path>) -> bool {
    let absolute = resolve_from(cwd.as_ref(), Path::new(path));
    lexically_human(&absolute) || lexically_human(resolve_write_target(&absolute))
}

fn contains_human_mention(value: &str) -> bool {
    let value = value.replace('\\', "/").to_ascii_lowercase();
    for marker in [".luuna", ".roots"] {
        let mut offset = 0;
        while let Some(found) = value[offset..].find(marker) {
            let start = offset + found + marker.len();
            let after_marker = &value[start..];
            let after_slashes = after_marker.trim_start_matches('/');
            if after_slashes.len() == after_marker.len() {
                offset = start;
                continue;
            }
            let needed = if marker == ".luuna" { "roots" } else { "human" };
            if let Some(rest) = after_slashes.strip_prefix(needed) {
                let rest_after_slashes = rest.trim_start_matches('/');
                let found_human = if marker == ".luuna" {
                    rest_after_slashes.starts_with("human") && rest_after_slashes.len() < rest.len()
                } else {
                    true
                };
                if found_human {
                    return true;
                }
            }
            offset = start;
        }
    }
    false
}

fn unquote(value: &str) -> String {
    let value = value.strip_prefix(['\'', '"']).unwrap_or(value);
    value.strip_suffix(['\'', '"']).unwrap_or(value).to_owned()
}

fn mentions_human(value: &str, cwd: &Path) -> bool {
    let value = unquote(value);
    !value.is_empty() && (contains_human_mention(&value) || is_protected_path(&value, cwd))
}

const REMOVE_OR_TOUCH: &[&str] = &[
    "rm", "rmdir", "mv", "touch", "truncate", "chmod", "chown", "unlink", "shred", "mkdir", "tee",
];
const COPY_TO_LAST: &[&str] = &["cp", "ln", "install", "rsync"];
const GIT_WRITES: &[&str] = &["checkout", "restore", "rm", "mv", "apply"];
const WRAPPERS: &[&str] = &["sudo", "env", "command", "nohup", "time", "xargs"];

fn is_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

fn command_words(segment: &str) -> Vec<String> {
    let mut words = segment
        .split_whitespace()
        .filter(|word| !word.is_empty())
        .map(unquote)
        .collect::<Vec<_>>();
    while let Some(first) = words.first() {
        if is_assignment(first) || WRAPPERS.contains(&first.as_str()) {
            words.remove(0);
        } else {
            break;
        }
    }
    words
}

fn redirection_targets(segment: &str) -> Vec<String> {
    let bytes = segment.as_bytes();
    let mut targets = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'>' || (index > 0 && bytes[index - 1] == b'>') {
            index += 1;
            continue;
        }
        // Mirror the left boundary in the legacy redirection expression:
        // `(?:^|[^0-9<])(?:&?>>?|[0-9]>>?)`.  In particular, `12>` was
        // not considered by that best-effort parser, so do not broaden its
        // observable behavior while porting it.
        let operator_start = if index > 0 && matches!(bytes[index - 1], b'&' | b'0'..=b'9') {
            index - 1
        } else {
            index
        };
        if operator_start > 0 && matches!(bytes[operator_start - 1], b'0'..=b'9' | b'<') {
            index += 1;
            continue;
        }
        // The first `>` owns a `>>` operator.  Later characters in the pair
        // are skipped with the cursor below.
        let mut cursor = index + 1;
        if cursor < bytes.len() && bytes[cursor] == b'>' {
            cursor += 1;
        }
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor >= bytes.len() || bytes[cursor] == b'&' {
            index = cursor.saturating_add(1);
            continue;
        }
        let start = cursor;
        if matches!(bytes[cursor], b'\'' | b'"') {
            let quote = bytes[cursor];
            cursor += 1;
            while cursor < bytes.len() && bytes[cursor] != quote {
                cursor += 1;
            }
            if cursor < bytes.len() {
                cursor += 1;
            }
        } else {
            while cursor < bytes.len()
                && !bytes[cursor].is_ascii_whitespace()
                && !matches!(bytes[cursor], b';' | b'&' | b'|' | b'<' | b'>')
            {
                cursor += 1;
            }
        }
        if start < cursor {
            targets.push(segment[start..cursor].to_owned());
        }
        index = cursor;
    }
    targets
}

fn redirects_into_human(segment: &str, cwd: &Path) -> bool {
    redirection_targets(segment)
        .iter()
        .any(|target| !target.starts_with('&') && mentions_human(target, cwd))
}

fn in_place_flag(argument: &str) -> bool {
    if argument.starts_with("--in-place") {
        return true;
    }
    let Some(flags) = argument.strip_prefix('-') else {
        return false;
    };
    flags
        .chars()
        .take_while(|character| character.is_ascii_alphabetic())
        .any(|character| character == 'i')
}

fn segment_writes_human(segment: &str, cwd: &Path) -> bool {
    if redirects_into_human(segment, cwd) {
        return true;
    }
    let words = command_words(segment);
    let Some((command, arguments)) = words.split_first() else {
        return false;
    };
    let name = command.rsplit('/').next().unwrap_or(command);
    let paths = arguments
        .iter()
        .filter(|argument| !argument.starts_with('-'))
        .collect::<Vec<_>>();
    let any_human = paths.iter().any(|argument| mentions_human(argument, cwd));
    if REMOVE_OR_TOUCH.contains(&name) {
        return any_human;
    }
    if COPY_TO_LAST.contains(&name) {
        return paths
            .last()
            .is_some_and(|argument| mentions_human(argument, cwd));
    }
    if matches!(name, "sed" | "perl") {
        return any_human && arguments.iter().any(|argument| in_place_flag(argument));
    }
    if name == "dd" {
        return arguments.iter().any(|argument| {
            argument
                .strip_prefix("of=")
                .is_some_and(|target| mentions_human(target, cwd))
        });
    }
    name == "git"
        && paths
            .first()
            .is_some_and(|subcommand| GIT_WRITES.contains(&subcommand.as_str()))
        && any_human
}

fn shell_segments(command: &str) -> Vec<&str> {
    let bytes = command.as_bytes();
    let mut segments = Vec::new();
    let mut start = 0;
    let mut index = 0;
    while index < bytes.len() {
        let width = match bytes[index] {
            b'\n' | b';' | b'|' => {
                if bytes[index] == b'|' && bytes.get(index + 1) == Some(&b'|') {
                    2
                } else {
                    1
                }
            }
            b'&' if bytes.get(index + 1) == Some(&b'&') => 2,
            _ => {
                index += 1;
                continue;
            }
        };
        segments.push(&command[start..index]);
        index += width;
        start = index;
    }
    segments.push(&command[start..]);
    segments
}

/// Best-effort shell classification for writes under a protected human tree.
#[must_use]
pub fn bash_writes_human(command: &str, cwd: impl AsRef<Path>) -> bool {
    if !contains_human_mention(command) {
        return false;
    }
    let cwd = cwd.as_ref();
    shell_segments(command)
        .into_iter()
        .any(|segment| segment_writes_human(segment, cwd))
}

fn input_field<'a>(input: &'a HookInput, key: &str) -> Option<&'a str> {
    input.tool_input.as_object()?.get(key)?.as_str()
}

/// Decide what the PreToolUse hook should do.  Unknown tools and malformed
/// fields are allowed deliberately; callers can report malformed JSON to stderr
/// while returning successful hook status.
#[must_use]
pub fn decide_hook(input: &HookInput, fallback_cwd: impl AsRef<Path>) -> GuardDecision {
    let tool = input.tool_name.as_str().unwrap_or_default();
    let cwd = input
        .cwd
        .as_str()
        .filter(|cwd| !cwd.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| fallback_cwd.as_ref().to_path_buf());
    let field = match tool {
        "Write" | "Edit" | "MultiEdit" => Some("file_path"),
        "NotebookEdit" => Some("notebook_path"),
        _ => None,
    };
    if let Some(field) = field {
        let Some(path) = input_field(input, field).filter(|path| !path.is_empty()) else {
            return GuardDecision::allow(None);
        };
        return if is_protected_path(path, &cwd) {
            GuardDecision::deny(
                format!("{DENY_REASON} (blocked {tool} on {path})"),
                path.to_owned(),
            )
        } else {
            GuardDecision::allow(Some(path.to_owned()))
        };
    }
    if tool == "Bash" {
        let Some(command) = input_field(input, "command") else {
            return GuardDecision::allow(None);
        };
        return if bash_writes_human(command, &cwd) {
            GuardDecision::deny(
                format!("{DENY_REASON} (blocked a shell write)"),
                command.to_owned(),
            )
        } else {
            GuardDecision::allow(Some(command.to_owned()))
        };
    }
    GuardDecision::allow(None)
}

/// Claude Code's exact `PreToolUse` JSON payload for a denial.
#[must_use]
pub fn deny_output(reason: &str) -> String {
    format!(
        "{}\n",
        json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": reason,
            }
        })
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn catches_file_targets_and_keeps_normal_sources_allowed() {
        let root = std::env::temp_dir().join(format!("roots-hook-{}", crate::ids::random_hex(8)));
        fs::create_dir_all(root.join(".luuna/roots/human/a")).expect("create human tree");
        assert!(is_protected_path(".luuna/roots/human/a/idea.md", &root));
        assert!(is_protected_path(".Roots/Human/x.md", &root));
        assert!(!is_protected_path(".luuna/roots/agent/notes/a/x.md", &root));
        assert!(!is_protected_path(".luuna/roots/humane/x.md", &root));
        fs::remove_dir_all(root).expect("clean test root");
    }

    #[test]
    fn follows_a_symlinked_write_target_when_supported() {
        let root =
            std::env::temp_dir().join(format!("roots-hook-link-{}", crate::ids::random_hex(8)));
        let human = root.join(".luuna/roots/human/a");
        let source = root.join("src");
        fs::create_dir_all(&human).expect("create human tree");
        fs::create_dir_all(&source).expect("create source tree");
        let alias = source.join("intent.md");
        let created = {
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(human.join("idea.md"), &alias).is_ok()
            }
            #[cfg(windows)]
            {
                std::os::windows::fs::symlink_file(human.join("idea.md"), &alias).is_ok()
            }
            #[cfg(not(any(unix, windows)))]
            {
                false
            }
        };
        if created {
            assert!(is_protected_path("src/intent.md", &root));
        }
        fs::remove_dir_all(root).expect("clean test root");
    }

    #[test]
    fn detects_the_supported_shell_write_shapes() {
        let cwd = std::env::temp_dir();
        for command in [
            "echo hi > .luuna/roots/human/a/idea.md",
            "cat x | tee .luuna/roots/human/a/idea.md",
            "rm -rf .luuna/roots/human/a",
            "cp /tmp/x.md .luuna/roots/human/a/idea.md",
            "sed -i '' 's/a/b/' .luuna/roots/human/a/idea.md",
            "dd if=/dev/zero of=.luuna/roots/human/a/idea.md",
            "git checkout -- .luuna/roots/human/a/idea.md",
        ] {
            assert!(bash_writes_human(command, &cwd), "{command}");
        }
        for command in [
            "cat .luuna/roots/human/a/idea.md",
            "cp .luuna/roots/human/a/idea.md /tmp/copy.md",
            "sed 's/a/b/' .luuna/roots/human/a/idea.md > /tmp/out.md",
        ] {
            assert!(!bash_writes_human(command, &cwd), "{command}");
        }
    }

    #[test]
    fn hook_decision_and_deny_json_match_the_contract() {
        let input = HookInput::from_value(&json!({
            "tool_name": "Edit",
            "tool_input": { "file_path": ".luuna/roots/human/a/idea.md" },
        }));
        let decision = decide_hook(&input, std::env::temp_dir());
        assert_eq!(decision.decision, Decision::Deny);
        let output = deny_output(decision.reason.as_deref().expect("reason"));
        let parsed: Value = serde_json::from_str(&output).expect("valid JSON");
        assert!(output.ends_with('\n'));
        assert_eq!(
            parsed["hookSpecificOutput"]["permissionDecision"],
            Value::String("deny".to_owned())
        );
        assert!(parse_hook_input("{not json").is_none());
        assert_eq!(
            parse_hook_input("[]").expect("tolerant shape").tool_name,
            Value::Null
        );
    }
}
