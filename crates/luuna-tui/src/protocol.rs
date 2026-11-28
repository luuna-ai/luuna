//! Versioned presentation protocol. The Node client remains the HTTP/API implementation.
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

pub const VERSION: u8 = 1;
pub const MAX_FRAME: usize = 1024 * 1024;
pub const QUEUE_SIZE: usize = 32;
pub const MAX_OUTPUT: usize = 256 * 1024;
pub const MAX_EVENTS: usize = 200;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ArgumentDef {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub context: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct FlagDef {
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub boolean: bool,
    #[serde(default)]
    pub repeat: bool,
    #[serde(default)]
    pub multiline: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct CommandDef {
    pub id: String,
    pub name: String,
    pub action: String,
    pub context: String,
    #[serde(default)]
    pub args: Vec<ArgumentDef>,
    #[serde(default)]
    pub flags: BTreeMap<String, FlagDef>,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default)]
    pub confirm: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ClientCommand {
    pub v: u8,
    pub id: String,
    pub op: String,
    #[serde(flatten)]
    pub fields: Map<String, Value>,
}

impl ClientCommand {
    pub fn new(id: impl Into<String>, op: &str, fields: Value) -> Self {
        Self {
            v: VERSION,
            id: id.into(),
            op: op.into(),
            fields: fields.as_object().cloned().unwrap_or_default(),
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>, String> {
        let mut bytes = serde_json::to_vec(self).map_err(|_| "Could not encode request.")?;
        if bytes.len() > MAX_FRAME {
            return Err("Request exceeds the 1 MiB protocol limit.".into());
        }
        bytes.push(b'\n');
        Ok(bytes)
    }
}

/// The kinds of event the bridge sends; anything else is a protocol failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EventKind {
    Catalog,
    Result,
    Output,
    Prompt,
    Event,
    Connection,
    Error,
    Guide,
}

impl EventKind {
    pub const ALL: [EventKind; 8] = [
        Self::Catalog,
        Self::Result,
        Self::Output,
        Self::Prompt,
        Self::Event,
        Self::Connection,
        Self::Error,
        Self::Guide,
    ];
    /// The name the event travels under.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Catalog => "catalog",
            Self::Result => "result",
            Self::Output => "output",
            Self::Prompt => "prompt",
            Self::Event => "event",
            Self::Connection => "connection",
            Self::Error => "error",
            Self::Guide => "guide",
        }
    }
    /// The kind a wire name names, if it is one.
    pub fn from_wire(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == name)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BridgeEvent {
    pub v: u8,
    pub id: String,
    pub event: EventKind,
    #[serde(flatten)]
    pub fields: Map<String, Value>,
}

/// A frame as read, before its event name is checked.
#[derive(Deserialize)]
struct RawEvent {
    v: u8,
    id: String,
    event: String,
    #[serde(flatten)]
    fields: Map<String, Value>,
}

impl BridgeEvent {
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_FRAME {
            return Err("Bridge frame exceeds 1 MiB.".into());
        }
        let raw: RawEvent =
            serde_json::from_slice(bytes).map_err(|_| "Bridge sent invalid JSON.")?;
        if raw.v != VERSION || raw.id.is_empty() || raw.id.len() > 128 {
            return Err("Bridge protocol version or request identity is invalid.".into());
        }
        let event = EventKind::from_wire(&raw.event).ok_or("Bridge sent an unknown event.")?;
        Ok(Self {
            v: raw.v,
            id: raw.id,
            event,
            fields: raw.fields,
        })
    }

    pub fn text(&self, key: &str) -> &str {
        self.fields.get(key).and_then(Value::as_str).unwrap_or("")
    }
}

/// Drop terminal control sequences, including OSC hyperlinks/title/clipboard and CSI commands.
/// Keeping only printable characters is insufficient: the remainder of an escape would leak.
pub fn safe_text(text: &str) -> String {
    #[derive(Clone, Copy)]
    enum State {
        Text,
        Escape,
        Csi,
        Osc,
        OscEscape,
        String,
        StringEscape,
    }
    let mut state = State::Text;
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        state = match state {
            State::Text => match c {
                '\x1b' => State::Escape,
                '\u{009b}' => State::Csi,
                '\u{009d}' => State::Osc,
                '\u{0090}' | '\u{009e}' | '\u{009f}' => State::String,
                '\n' => {
                    out.push('\n');
                    State::Text
                }
                '\t' => {
                    out.push_str("    ");
                    State::Text
                }
                c if c.is_control() => State::Text,
                c => {
                    out.push(c);
                    State::Text
                }
            },
            State::Escape => match c {
                '[' => State::Csi,
                ']' => State::Osc,
                'P' | '^' | '_' => State::String,
                _ => State::Text,
            },
            State::Csi => {
                if ('@'..='~').contains(&c) {
                    State::Text
                } else {
                    State::Csi
                }
            }
            State::Osc => match c {
                '\x07' | '\u{009c}' => State::Text,
                '\x1b' => State::OscEscape,
                _ => State::Osc,
            },
            State::OscEscape => {
                if c == '\\' {
                    State::Text
                } else {
                    State::Osc
                }
            }
            State::String => match c {
                '\u{009c}' => State::Text,
                '\x1b' => State::StringEscape,
                _ => State::String,
            },
            State::StringEscape => {
                if c == '\\' {
                    State::Text
                } else {
                    State::String
                }
            }
        }
    }
    out
}

pub fn bounded_append(target: &mut String, text: &str) -> bool {
    target.push_str(&safe_text(text));
    if target.len() <= MAX_OUTPUT {
        return false;
    }
    let mut start = target.len() - MAX_OUTPUT;
    while !target.is_char_boundary(start) {
        start += 1;
    }
    target.drain(..start);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn controls_cannot_execute_or_leave_osc_payload() {
        assert_eq!(
            safe_text("hello\x1b[31mred\x1b[0m\x1b]52;c;secret\x07!\u{009d}title\u{009c}\n界"),
            "hellored!\n界"
        );
        assert_eq!(safe_text("a\x1bPpayload\x1b\\b\x00\t"), "ab    ");
    }
    #[test]
    fn every_event_kind_round_trips_its_wire_name_and_unknown_names_are_refused() {
        for kind in EventKind::ALL {
            assert_eq!(EventKind::from_wire(kind.as_str()), Some(kind));
            assert_eq!(serde_json::to_value(kind).unwrap(), json!(kind.as_str()));
        }
        assert_eq!(EventKind::from_wire("telemetry"), None);
        assert_eq!(
            BridgeEvent::decode(br#"{"v":1,"id":"x","event":"telemetry"}"#).unwrap_err(),
            "Bridge sent an unknown event."
        );
    }
    #[test]
    fn strict_version_size_and_utf8_bounds() {
        assert!(BridgeEvent::decode(br#"{"v":2,"id":"x","event":"result"}"#).is_err());
        assert!(BridgeEvent::decode(&vec![b' '; MAX_FRAME + 1]).is_err());
        assert!(
            ClientCommand::new("x", "invoke", json!({"value":"x".repeat(MAX_FRAME)}))
                .encode()
                .is_err()
        );
        let mut output = String::new();
        assert!(bounded_append(&mut output, &"界".repeat(MAX_OUTPUT)));
        assert!(output.len() <= MAX_OUTPUT);
        assert!(output.is_char_boundary(0));
    }
}
