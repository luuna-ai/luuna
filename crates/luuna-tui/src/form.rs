//! Action forms: one field per catalog argument and flag, and the line editor their fields share.
use crate::protocol::CommandDef;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default)]
pub struct FormField {
    pub name: String,
    pub label: String,
    pub hint: String,
    pub value: String,
    pub argument: bool,
    pub boolean: bool,
    pub repeated: bool,
    /// Values the developer can pick with Left/Right instead of typing (ADR 0042): records the
    /// workbench already read (as short ids), the scopes and roles the runtime accepts, yes/no.
    /// Each is `(value, label)`. Typing stays possible; a choice only fills the field.
    pub choices: Vec<(String, String)>,
    /// The value must be one of `choices` (a ticket type, a scope, yes/no): typing a letter
    /// picks the next choice starting with it instead of inserting text.
    pub fixed: bool,
    /// Rarely needed (idempotency keys, revisions, budgets): listed last, under "More options".
    pub advanced: bool,
    /// Letters typed into a fixed field since its last pick, so "process" picks process
    /// while a repeated "p" steps through the p choices.
    pub typed: String,
}
impl FormField {
    /// What a key does to this field: a fixed choice field jumps to the next choice that
    /// starts with the typed letter (Backspace or Ctrl+U clears it); any other field edits text.
    pub fn press(&mut self, key: KeyEvent) {
        if !self.fixed || self.choices.is_empty() {
            edit(&mut self.value, key, true);
            return;
        }
        match key.code {
            KeyCode::Char(c) if crate::keys::is_text(&key) => {
                let c = c.to_ascii_lowercase();
                let starts = |(value, label): &(String, String), prefix: &str| {
                    value.to_ascii_lowercase().starts_with(prefix)
                        || label.to_ascii_lowercase().starts_with(prefix)
                };
                // Extend what was typed while a choice still starts with it; otherwise step to
                // the next choice starting with this letter.
                let longer = format!("{}{c}", self.typed);
                if let Some(at) = self.choices.iter().position(|ch| starts(ch, &longer)) {
                    if longer.len() > 1 || self.choice_index() != Some(at) {
                        self.value = self.choices[at].0.clone();
                        self.typed = longer;
                        return;
                    }
                }
                let n = self.choices.len();
                let from = self.choice_index().map_or(0, |i| i + 1);
                let letter = c.to_string();
                if let Some(next) = (0..n)
                    .map(|step| (from + step) % n)
                    .find(|&i| starts(&self.choices[i], &letter))
                {
                    self.value = self.choices[next].0.clone();
                    self.typed = letter;
                }
            }
            KeyCode::Backspace => {
                self.value.clear();
                self.typed.clear();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.value.clear();
                self.typed.clear();
            }
            _ => {}
        }
    }
    /// Moves to the next (or previous) choice and puts its value in the field.
    pub fn cycle(&mut self, forward: bool) -> bool {
        let n = self.choices.len();
        if n == 0 {
            return false;
        }
        let next = match self.choice_index() {
            Some(i) if forward => (i + 1) % n,
            Some(i) => (i + n - 1) % n,
            None if forward => 0,
            None => n - 1,
        };
        self.value = self.choices[next].0.clone();
        self.typed.clear();
        true
    }
    /// The position of the current value among the choices.
    pub fn choice_index(&self) -> Option<usize> {
        self.choices
            .iter()
            .position(|(value, _)| *value == self.value)
    }
}
#[derive(Clone, Debug, Default)]
pub struct ActionForm {
    pub command: CommandDef,
    pub fields: Vec<FormField>,
    pub cursor: usize,
}
impl ActionForm {
    pub fn new(command: CommandDef, context: &BTreeMap<String, String>) -> Self {
        let mut fields = command
            .args
            .iter()
            .map(|arg| FormField {
                name: arg.name.clone(),
                label: label(&arg.prompt, &arg.name),
                hint: sentence(&arg.description),
                value: context.get(&arg.name).cloned().unwrap_or_default(),
                argument: true,
                ..Default::default()
            })
            .collect::<Vec<_>>();
        // Flags in the order commands.json lists them (the most useful first), not the
        // alphabetical order the catalog's JSON object arrives in.
        let order: Vec<String> = crate::commands::by_name(&command.name)
            .map(|c| c.flags.iter().map(|(name, _)| name.clone()).collect())
            .unwrap_or_default();
        let mut flags: Vec<_> = command
            .flags
            .iter()
            .filter(|(name, _)| name.as_str() != "acknowledge-hash")
            .collect();
        flags.sort_by_key(|(name, _)| order.iter().position(|o| o == *name).unwrap_or(usize::MAX));
        fields.extend(flags.into_iter().map(|(name, flag)| {
            let repeated = flag.repeat || flag.multiline;
            let mut hint = sentence(&flag.description);
            if repeated && !flag.boolean {
                hint.push_str(" One per line: Alt+Enter adds a line.");
            }
            FormField {
                name: name.clone(),
                label: label(&flag.prompt, name),
                hint,
                boolean: flag.boolean,
                repeated,
                choices: if flag.boolean {
                    vec![("yes".into(), String::new()), ("no".into(), String::new())]
                } else {
                    Vec::new()
                },
                fixed: flag.boolean,
                advanced: ADVANCED.contains(&name.as_str()),
                ..Default::default()
            }
        }));
        // Advanced fields last; arguments keep their order among themselves, as `input` needs.
        fields.sort_by_key(|field| field.advanced);
        Self {
            command,
            fields,
            cursor: 0,
        }
    }
    pub fn input(&self) -> Result<(Vec<String>, Map<String, Value>), String> {
        let mut args = Vec::new();
        let mut flags = Map::new();
        for field in &self.fields {
            if field.name == "@confirm" {
                if !matches!(field.value.trim().to_lowercase().as_str(), "yes" | "y") {
                    return Err("Type yes in the confirmation field to perform this action.".into());
                }
                continue;
            }
            if field.name.starts_with('@') {
                continue;
            }
            if field.argument {
                if field.value.trim().is_empty() {
                    return Err(format!("{} is required.", field.label));
                }
                args.push(field.value.clone());
            } else if !field.value.is_empty() {
                let value = if field.boolean {
                    match field.value.to_lowercase().trim() {
                        "yes" | "true" | "1" | "y" => Value::Bool(true),
                        "no" | "false" | "0" | "n" => Value::Bool(false),
                        _ => return Err(format!("{} needs yes or no.", field.label)),
                    }
                } else if field.repeated {
                    json!(field
                        .value
                        .lines()
                        .filter(|line| !line.trim().is_empty())
                        .collect::<Vec<_>>())
                } else {
                    json!(field.value)
                };
                flags.insert(field.name.clone(), value);
            }
        }
        Ok((args, flags))
    }
}
/// Flags that are rarely set by hand: idempotency keys, revisions read for you, budgets and
/// the multi-repository plan syntax. Forms list them last, under "More options".
pub const ADVANCED: &[&str] = &[
    "key",
    "id",
    "revision",
    "expected-version",
    "derived-from",
    "override",
    "task",
    "workers",
    "attempts",
    "timeout",
    "cost-usd",
    "check",
    "checks",
    "clear-labels",
];

/// A field's label: the catalog's prompt without its parenthetical instructions (the hint
/// and the choices say those), or the name in words ("cost-usd" is "Cost usd").
pub fn label(prompt: &str, name: &str) -> String {
    let prompt = prompt.trim();
    let base = match prompt.find(" (") {
        Some(at) if prompt.ends_with(')') => &prompt[..at],
        _ => prompt,
    };
    if !base.is_empty() {
        return base.to_string();
    }
    let words = name.replace(['-', '_'], " ");
    let mut chars = words.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// A catalog description as a sentence: capitalized, with a full stop.
pub fn sentence(text: &str) -> String {
    let text = text.trim();
    let mut chars = text.chars();
    let mut out: String = match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => return String::new(),
    };
    if !out.ends_with(['.', '?', '!']) {
        out.push('.');
    }
    out
}

pub(crate) fn edit(text: &mut String, key: KeyEvent, multiline: bool) {
    if text.len() >= 256 * 1024 && key.code != KeyCode::Backspace {
        return;
    }
    match key.code {
        KeyCode::Backspace => {
            text.pop();
        }
        KeyCode::Enter if multiline => text.push('\n'),
        KeyCode::Char(c) if crate::keys::is_text(&key) => text.push(c),
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => text.clear(),
        _ => {}
    }
}
