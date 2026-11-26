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
}
impl FormField {
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
        true
    }
    /// The position of the current value among the choices.
    pub fn choice_index(&self) -> Option<usize> {
        self.choices
            .iter()
            .position(|(value, _)| *value == self.value)
    }
}
#[derive(Clone, Debug)]
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
                label: if arg.prompt.is_empty() {
                    arg.name.clone()
                } else {
                    arg.prompt.clone()
                },
                hint: arg.description.clone(),
                value: context.get(&arg.name).cloned().unwrap_or_default(),
                argument: true,
                boolean: false,
                repeated: false,
                choices: Vec::new(),
            })
            .collect::<Vec<_>>();
        fields.extend(
            command
                .flags
                .iter()
                .filter(|(name, _)| name.as_str() != "acknowledge-hash")
                .map(|(name, flag)| FormField {
                    name: name.clone(),
                    label: format!("--{name}"),
                    hint: if flag.prompt.is_empty() {
                        flag.description.clone()
                    } else {
                        flag.prompt.clone()
                    },
                    value: String::new(),
                    argument: false,
                    boolean: flag.boolean,
                    repeated: flag.repeat || flag.multiline,
                    choices: if flag.boolean {
                        vec![("yes".into(), String::new()), ("no".into(), String::new())]
                    } else {
                        Vec::new()
                    },
                }),
        );
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
