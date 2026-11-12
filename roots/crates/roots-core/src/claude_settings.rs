//! Pure Claude Code hook-settings merge primitives.
//!
//! Roots owns only command handlers beginning with `luuna-roots`/`roots` and
//! one of `prime`, `guard`, or `drift`.  All unrelated settings and handlers
//! survive an install or remove operation unchanged.

use serde::Serialize;
use serde_json::{Map, Value};
use std::borrow::Cow;
use std::fmt::{Display, Formatter};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HookEvent {
    SessionStart,
    PreToolUse,
    Stop,
}

impl HookEvent {
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::SessionStart => "SessionStart",
            Self::PreToolUse => "PreToolUse",
            Self::Stop => "Stop",
        }
    }
}

impl Serialize for HookEvent {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.key())
    }
}

pub const MANAGED_EVENTS: [HookEvent; 3] = [
    HookEvent::SessionStart,
    HookEvent::PreToolUse,
    HookEvent::Stop,
];

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RootsHookSpec {
    pub event: HookEvent,
    pub command: Cow<'static, str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matcher: Option<Cow<'static, str>>,
}

impl RootsHookSpec {
    #[must_use]
    pub fn owned(event: HookEvent, matcher: Option<String>, command: impl Into<String>) -> Self {
        Self {
            event,
            command: Cow::Owned(command.into()),
            matcher: matcher.map(Cow::Owned),
        }
    }
}

pub const PRIME_HOOK: RootsHookSpec = RootsHookSpec {
    event: HookEvent::SessionStart,
    command: Cow::Borrowed("luuna-roots prime --hook"),
    matcher: None,
};

pub const GUARD_HOOK: RootsHookSpec = RootsHookSpec {
    event: HookEvent::PreToolUse,
    command: Cow::Borrowed("luuna-roots guard"),
    matcher: Some(Cow::Borrowed("Write|Edit|MultiEdit|NotebookEdit|Bash")),
};

pub const DRIFT_HOOK: RootsHookSpec = RootsHookSpec {
    event: HookEvent::Stop,
    command: Cow::Borrowed("luuna-roots drift --diff HEAD"),
    matcher: None,
};

/// Documentation form of the TypeScript compatibility expression:
/// `^\s*(?:luuna-roots|roots)\s+(prime|guard|drift)(\s|$)`.
pub const ROOTS_COMMAND_PATTERN: &str = r"^\s*(?:luuna-roots|roots)\s+(prime|guard|drift)(\s|$)";

#[derive(Clone, Copy)]
pub struct HookContext<'a> {
    /// The project tier, or `None` for a user-level setup outside a project.
    pub tier: Option<u8>,
    /// Whether a command is registered.  `drift` must exist before installing
    /// the Stop hook, preventing a broken hook configuration in reduced builds.
    pub has_command: &'a dyn Fn(&str) -> bool,
}

/// Return the Roots hooks wanted for an installation context.
#[must_use]
pub fn desired_hooks(context: HookContext<'_>) -> Vec<RootsHookSpec> {
    let mut hooks = vec![PRIME_HOOK, GUARD_HOOK];
    if context.tier == Some(3) && (context.has_command)("drift") {
        hooks.push(DRIFT_HOOK);
    }
    hooks
}

/// True when a command is Roots-owned under the compatibility grammar.
#[must_use]
pub fn is_roots_command(command: &str) -> bool {
    let command = command.trim_start();
    let rest = ["luuna-roots", "roots"].iter().find_map(|prefix| {
        command
            .strip_prefix(prefix)
            .filter(|remaining| remaining.chars().next().is_some_and(char::is_whitespace))
    });
    let Some(rest) = rest else {
        return false;
    };
    let rest = rest.trim_start();
    ["prime", "guard", "drift"].iter().any(|verb| {
        rest.strip_prefix(verb).is_some_and(|remaining| {
            remaining.is_empty() || remaining.chars().next().is_some_and(char::is_whitespace)
        })
    })
}

/// True when an arbitrary handler belongs to Roots and should be managed.
#[must_use]
pub fn is_roots_handler(handler: &Value) -> bool {
    handler
        .as_object()
        .and_then(|object| object.get("command"))
        .and_then(Value::as_str)
        .is_some_and(is_roots_command)
}

fn group_hooks(group: &Value) -> &[Value] {
    group
        .as_object()
        .and_then(|object| object.get("hooks"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn make_group(spec: &RootsHookSpec) -> Value {
    let mut command = Map::new();
    command.insert("type".to_owned(), Value::String("command".to_owned()));
    command.insert(
        "command".to_owned(),
        Value::String(spec.command.to_string()),
    );
    let mut group = Map::new();
    if let Some(matcher) = &spec.matcher {
        group.insert("matcher".to_owned(), Value::String(matcher.to_string()));
    }
    group.insert(
        "hooks".to_owned(),
        Value::Array(vec![Value::Object(command)]),
    );
    Value::Object(group)
}

fn group_matches_spec(group: &Value, handler: &Value, spec: &RootsHookSpec) -> bool {
    let Some(group_object) = group.as_object() else {
        return false;
    };
    let handler_matches = handler
        .as_object()
        .and_then(|object| object.get("command"))
        .and_then(Value::as_str)
        .is_some_and(|command| command == spec.command);
    let matcher_matches = match &spec.matcher {
        Some(matcher) => group_object
            .get("matcher")
            .and_then(Value::as_str)
            .is_some_and(|current| current == matcher),
        None => !group_object.contains_key("matcher"),
    };
    handler_matches && matcher_matches && group_hooks(group).len() == 1
}

/// Whether an event already contains exactly the Roots hooks being requested.
fn already_installed(groups: &[Value], specs: &[RootsHookSpec]) -> bool {
    let roots = groups
        .iter()
        .flat_map(|group| {
            group_hooks(group)
                .iter()
                .map(move |handler| (group, handler))
        })
        .filter(|(_, handler)| is_roots_handler(handler))
        .collect::<Vec<_>>();
    roots.len() == specs.len()
        && specs.iter().all(|spec| {
            roots
                .iter()
                .any(|(group, handler)| group_matches_spec(group, handler, spec))
        })
}

fn strip_roots(groups: &[Value]) -> Vec<Value> {
    let mut output = Vec::new();
    for group in groups {
        let Some(group_object) = group.as_object() else {
            output.push(group.clone());
            continue;
        };
        let Some(handlers) = group_object.get("hooks").and_then(Value::as_array) else {
            output.push(group.clone());
            continue;
        };
        let kept = handlers
            .iter()
            .filter(|handler| !is_roots_handler(handler))
            .cloned()
            .collect::<Vec<_>>();
        if kept.len() == handlers.len() {
            output.push(group.clone());
        } else if !kept.is_empty() {
            let mut next = group_object.clone();
            next.insert("hooks".to_owned(), Value::Array(kept));
            output.push(Value::Object(next));
        }
    }
    output
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SettingsShapeError {
    message: String,
}

impl SettingsShapeError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl Display for SettingsShapeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for SettingsShapeError {}

fn hooks_of(settings: &Map<String, Value>) -> Result<Map<String, Value>, SettingsShapeError> {
    match settings.get("hooks") {
        None => Ok(Map::new()),
        Some(Value::Object(hooks)) => Ok(hooks.clone()),
        Some(_) => Err(SettingsShapeError::new("`hooks` is not an object")),
    }
}

fn sync_event(
    hooks: &mut Map<String, Value>,
    event: HookEvent,
    specs: &[RootsHookSpec],
) -> Result<bool, SettingsShapeError> {
    let key = event.key();
    let current = hooks.get(key);
    if current.is_some_and(|value| !value.is_array()) {
        return Err(SettingsShapeError::new(format!(
            "`hooks.{key}` is not an array"
        )));
    }
    let groups = current
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if already_installed(&groups, specs) {
        return Ok(false);
    }
    let mut next = strip_roots(&groups);
    next.extend(specs.iter().map(make_group));
    if next.is_empty() {
        hooks.remove(key);
    } else {
        hooks.insert(key.to_owned(), Value::Array(next));
    }
    Ok(true)
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MergeResult {
    pub settings: Map<String, Value>,
    pub changed: bool,
}

/// Make Roots handlers in settings exactly equal to `wanted`, without mutating
/// the supplied JSON object.  If unchanged, callers should retain their input
/// text instead of reserializing the equivalent returned map.
pub fn sync_roots_hooks(
    settings: &Map<String, Value>,
    wanted: &[RootsHookSpec],
) -> Result<MergeResult, SettingsShapeError> {
    let mut next = settings.clone();
    let mut hooks = hooks_of(&next)?;
    let mut changed = false;
    for event in MANAGED_EVENTS {
        let specs = wanted
            .iter()
            .filter(|spec| spec.event == event)
            .cloned()
            .collect::<Vec<_>>();
        if !hooks.contains_key(event.key()) && specs.is_empty() {
            continue;
        }
        if sync_event(&mut hooks, event, &specs)? {
            changed = true;
        }
    }
    if !changed {
        return Ok(MergeResult {
            settings: settings.clone(),
            changed: false,
        });
    }
    // Keep the object once this operation changed it, even when all managed
    // events were removed.  Claude's settings writer has historically left
    // `{ "hooks": {} }` behind after Roots removes its final handler; this
    // is observable both in the saved bytes and in a subsequent no-op remove.
    next.insert("hooks".to_owned(), Value::Object(hooks));
    Ok(MergeResult {
        settings: next,
        changed: true,
    })
}

fn group_roots_hooks(event: HookEvent, group: &Value) -> Vec<RootsHookSpec> {
    let matcher = group
        .as_object()
        .and_then(|object| object.get("matcher"))
        .and_then(Value::as_str)
        .filter(|matcher| !matcher.is_empty())
        .map(|matcher| Cow::Owned(matcher.to_owned()));
    group_hooks(group)
        .iter()
        .filter(|handler| is_roots_handler(handler))
        .filter_map(|handler| {
            handler
                .as_object()
                .and_then(|object| object.get("command"))
                .and_then(Value::as_str)
                .map(|command| RootsHookSpec {
                    event,
                    matcher: matcher.clone(),
                    command: Cow::Owned(command.to_owned()),
                })
        })
        .collect()
}

/// Return Roots handlers currently installed for reporting only.  Malformed
/// hook shapes are ignored here; `sync_roots_hooks` performs strict validation
/// before it writes anything.
#[must_use]
pub fn installed_roots_hooks(settings: &Map<String, Value>) -> Vec<RootsHookSpec> {
    let Some(hooks) = settings.get("hooks").and_then(Value::as_object) else {
        return Vec::new();
    };
    MANAGED_EVENTS
        .iter()
        .flat_map(|event| {
            hooks
                .get(event.key())
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .flat_map(|group| group_roots_hooks(*event, group))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn user_hook() -> Value {
        json!({
            "matcher": "Bash",
            "hooks": [{ "type": "command", "command": "ml prime" }],
        })
    }

    #[test]
    fn wants_drift_only_when_the_command_exists_at_tier_three() {
        let all = |_: &str| true;
        let none = |_: &str| false;
        assert_eq!(
            desired_hooks(HookContext {
                tier: Some(3),
                has_command: &none,
            }),
            vec![PRIME_HOOK, GUARD_HOOK]
        );
        assert_eq!(
            desired_hooks(HookContext {
                tier: Some(3),
                has_command: &all,
            }),
            vec![PRIME_HOOK, GUARD_HOOK, DRIFT_HOOK]
        );
        assert_eq!(
            serde_json::to_string(&GUARD_HOOK).expect("serialize hook"),
            r#"{"event":"PreToolUse","command":"luuna-roots guard","matcher":"Write|Edit|MultiEdit|NotebookEdit|Bash"}"#
        );
    }

    #[test]
    fn merges_roots_handlers_without_disturbing_user_handlers() {
        let mut hooks = Map::new();
        hooks.insert(
            "PreToolUse".to_owned(),
            Value::Array(vec![
                user_hook(),
                json!({
                    "matcher": "Write",
                    "hooks": [{ "type": "command", "command": "luuna-roots guard --old" }],
                }),
            ]),
        );
        let mut settings = Map::new();
        settings.insert("model".to_owned(), Value::String("opus".to_owned()));
        settings.insert("hooks".to_owned(), Value::Object(hooks));
        let once = sync_roots_hooks(&settings, &[PRIME_HOOK, GUARD_HOOK]).expect("merge hooks");
        assert!(once.changed);
        assert_eq!(once.settings["hooks"]["PreToolUse"][0], user_hook());
        let twice =
            sync_roots_hooks(&once.settings, &[PRIME_HOOK, GUARD_HOOK]).expect("idempotent merge");
        assert!(!twice.changed);
        assert_eq!(twice.settings, once.settings);
    }

    #[test]
    fn removes_only_roots_handlers_and_validates_shape() {
        let mut hooks = Map::new();
        hooks.insert("PreToolUse".to_owned(), Value::Array(vec![user_hook()]));
        let mut settings = Map::new();
        settings.insert("hooks".to_owned(), Value::Object(hooks));
        let installed = sync_roots_hooks(&settings, &[PRIME_HOOK, GUARD_HOOK, DRIFT_HOOK])
            .expect("install hooks");
        let removed = sync_roots_hooks(&installed.settings, &[]).expect("remove hooks");
        assert_eq!(
            removed.settings["hooks"]["PreToolUse"],
            Value::Array(vec![user_hook()])
        );

        let mut malformed = Map::new();
        malformed.insert("hooks".to_owned(), Value::Array(Vec::new()));
        assert_eq!(
            sync_roots_hooks(&malformed, &[])
                .expect_err("invalid shape")
                .to_string(),
            "`hooks` is not an object"
        );
    }

    #[test]
    fn removal_of_the_last_roots_hook_keeps_an_explicit_empty_hooks_object() {
        let settings = Map::new();
        let installed =
            sync_roots_hooks(&settings, &[PRIME_HOOK, GUARD_HOOK]).expect("install roots hooks");
        let removed = sync_roots_hooks(&installed.settings, &[]).expect("remove roots hooks");

        assert!(removed.changed);
        assert_eq!(
            removed.settings.get("hooks"),
            Some(&Value::Object(Map::new()))
        );
    }
}
