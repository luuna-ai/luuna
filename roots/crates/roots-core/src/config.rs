//! Defaults, validation, and stable YAML rendering for Roots configuration.

use crate::errors::{RootsError, RootsResult};
use crate::paths::RootsPaths;
use crate::store::atomic_write;
use crate::yaml::{YamlMap, parse_yaml, stringify_yaml};
use serde_json::{Map, Number, Value};
use std::fs;

pub type Tier = u8;
pub const CONFIG_VERSION: &str = "1";
pub const DEFAULT_AGENT_TIMEOUT_SECONDS: u64 = 120;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentConfig {
    pub command: Option<String>,
    pub timeout_seconds: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Limits {
    pub proposals: u64,
    pub sprouts: u64,
    pub questions_per_session: u64,
    pub proposal_ttl_days: u64,
    pub sprout_ttl_days: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuestionThresholds {
    pub done_after_sessions: u64,
    pub big_body_lines: u64,
    pub orphan_after_days: u64,
    pub stale_after_days: u64,
    pub snooze_days: u64,
}

pub const DEFAULT_QUESTION_THRESHOLDS: QuestionThresholds = QuestionThresholds {
    done_after_sessions: 1,
    big_body_lines: 30,
    orphan_after_days: 3,
    stale_after_days: 30,
    snooze_days: 7,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RootsConfig {
    pub project: String,
    pub version: String,
    pub tier: Tier,
    pub agent: AgentConfig,
    pub limits: Limits,
    pub questions: QuestionThresholds,
    pub view_write: bool,
    pub flow_heading: bool,
}

#[must_use]
pub fn default_config(project: impl Into<String>) -> RootsConfig {
    RootsConfig {
        project: project.into(),
        version: CONFIG_VERSION.to_owned(),
        tier: 2,
        agent: AgentConfig {
            command: None,
            timeout_seconds: DEFAULT_AGENT_TIMEOUT_SECONDS,
        },
        limits: Limits {
            proposals: 10,
            sprouts: 5,
            questions_per_session: 3,
            proposal_ttl_days: 14,
            sprout_ttl_days: 30,
        },
        questions: DEFAULT_QUESTION_THRESHOLDS,
        view_write: false,
        flow_heading: true,
    }
}

fn error(message: impl Into<String>) -> RootsError {
    RootsError::config(message)
}

fn mapping(value: Option<&Value>, field: &str) -> RootsResult<YamlMap> {
    match value {
        None | Some(Value::Null) => Ok(Map::new()),
        Some(Value::Object(map)) => Ok(map.clone()),
        _ => Err(error(format!("config.yaml: `{field}` must be a mapping"))),
    }
}

fn number_string(number: &Number) -> String {
    if let Some(value) = number.as_i64() {
        return value.to_string();
    }
    if let Some(value) = number.as_u64() {
        return value.to_string();
    }
    let value = number.as_f64().unwrap_or_default();
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        value.to_string()
    }
}

fn integer(map: &YamlMap, key: &str, field: &str, fallback: u64, min: u64) -> RootsResult<u64> {
    let Some(value) = map.get(key) else {
        return Ok(fallback);
    };
    if value.is_null() {
        return Ok(fallback);
    }
    let Some(value) = value.as_u64() else {
        return Err(error(format!(
            "config.yaml: `{field}` must be an integer >= {min}"
        )));
    };
    if value < min {
        return Err(error(format!(
            "config.yaml: `{field}` must be an integer >= {min}"
        )));
    }
    Ok(value)
}

fn string(map: &YamlMap, key: &str, field: &str, fallback: &str) -> RootsResult<String> {
    match map.get(key) {
        None | Some(Value::Null) => Ok(fallback.to_owned()),
        Some(Value::String(value)) => Ok(value.clone()),
        Some(Value::Number(value)) => Ok(number_string(value)),
        _ => Err(error(format!("config.yaml: `{field}` must be a string"))),
    }
}

fn boolean(map: &YamlMap, key: &str, field: &str, fallback: bool) -> RootsResult<bool> {
    match map.get(key) {
        None | Some(Value::Null) => Ok(fallback),
        Some(Value::Bool(value)) => Ok(*value),
        _ => Err(error(format!(
            "config.yaml: `{field}` must be true or false"
        ))),
    }
}

fn agent_command(map: &YamlMap) -> RootsResult<Option<String>> {
    match map.get("command") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if value.is_empty() => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        _ => Err(error("config.yaml: `agent.command` must be a string")),
    }
}

/// Merge parsed YAML over defaults, retaining the historical behavior that
/// unknown keys are ignored.
pub fn config_from_yaml(document: &YamlMap, fallback_project: &str) -> RootsResult<RootsConfig> {
    let defaults = default_config(fallback_project);
    let agent = mapping(document.get("agent"), "agent")?;
    let limits = mapping(document.get("limits"), "limits")?;
    let questions = mapping(document.get("questions"), "questions")?;
    let view = mapping(document.get("view"), "view")?;
    let flow = mapping(document.get("flow"), "flow")?;
    let tier = integer(document, "tier", "tier", u64::from(defaults.tier), 0)?;
    if tier > 3 {
        return Err(error("config.yaml: `tier` must be 0, 1, 2 or 3"));
    }
    Ok(RootsConfig {
        project: string(document, "project", "project", &defaults.project)?,
        version: string(document, "version", "version", &defaults.version)?,
        tier: tier as Tier,
        agent: AgentConfig {
            command: agent_command(&agent)?,
            timeout_seconds: integer(
                &agent,
                "timeoutSeconds",
                "agent.timeoutSeconds",
                defaults.agent.timeout_seconds,
                1,
            )?,
        },
        limits: Limits {
            proposals: integer(
                &limits,
                "proposals",
                "limits.proposals",
                defaults.limits.proposals,
                0,
            )?,
            sprouts: integer(
                &limits,
                "sprouts",
                "limits.sprouts",
                defaults.limits.sprouts,
                0,
            )?,
            questions_per_session: integer(
                &limits,
                "questionsPerSession",
                "limits.questionsPerSession",
                defaults.limits.questions_per_session,
                0,
            )?,
            proposal_ttl_days: integer(
                &limits,
                "proposalTtlDays",
                "limits.proposalTtlDays",
                defaults.limits.proposal_ttl_days,
                1,
            )?,
            sprout_ttl_days: integer(
                &limits,
                "sproutTtlDays",
                "limits.sproutTtlDays",
                defaults.limits.sprout_ttl_days,
                1,
            )?,
        },
        questions: QuestionThresholds {
            done_after_sessions: integer(
                &questions,
                "doneAfterSessions",
                "questions.doneAfterSessions",
                defaults.questions.done_after_sessions,
                0,
            )?,
            big_body_lines: integer(
                &questions,
                "bigBodyLines",
                "questions.bigBodyLines",
                defaults.questions.big_body_lines,
                1,
            )?,
            orphan_after_days: integer(
                &questions,
                "orphanAfterDays",
                "questions.orphanAfterDays",
                defaults.questions.orphan_after_days,
                0,
            )?,
            stale_after_days: integer(
                &questions,
                "staleAfterDays",
                "questions.staleAfterDays",
                defaults.questions.stale_after_days,
                1,
            )?,
            snooze_days: integer(
                &questions,
                "snoozeDays",
                "questions.snoozeDays",
                defaults.questions.snooze_days,
                1,
            )?,
        },
        view_write: boolean(&view, "write", "view.write", defaults.view_write)?,
        flow_heading: boolean(&flow, "heading", "flow.heading", defaults.flow_heading)?,
    })
}

pub fn parse_config(text: &str, fallback_project: &str) -> RootsResult<RootsConfig> {
    let document = parse_yaml(text).map_err(|value| error(format!("config.yaml: {value}")))?;
    config_from_yaml(&document, fallback_project)
}

pub fn load_config(paths: &RootsPaths) -> RootsResult<RootsConfig> {
    let project = paths
        .root
        .file_name()
        .and_then(|part| part.to_str())
        .unwrap_or("project");
    if !paths.config.exists() {
        return Ok(default_config(project));
    }
    let text =
        fs::read_to_string(&paths.config).map_err(|value| RootsError::error(value.to_string()))?;
    parse_config(&text, project)
}

fn object(entries: impl IntoIterator<Item = (String, Value)>) -> Value {
    Value::Object(entries.into_iter().collect())
}

fn config_yaml(config: &RootsConfig) -> YamlMap {
    let mut agent = Map::new();
    if let Some(command) = &config.agent.command {
        if !command.is_empty() {
            agent.insert("command".to_owned(), Value::String(command.clone()));
        }
    }
    if config.agent.timeout_seconds != DEFAULT_AGENT_TIMEOUT_SECONDS {
        agent.insert(
            "timeoutSeconds".to_owned(),
            Value::Number(Number::from(config.agent.timeout_seconds)),
        );
    }
    [
        ("project".to_owned(), Value::String(config.project.clone())),
        ("version".to_owned(), Value::String(config.version.clone())),
        ("tier".to_owned(), Value::Number(Number::from(config.tier))),
        ("agent".to_owned(), Value::Object(agent)),
        (
            "limits".to_owned(),
            object([
                (
                    "proposals".to_owned(),
                    Value::Number(Number::from(config.limits.proposals)),
                ),
                (
                    "sprouts".to_owned(),
                    Value::Number(Number::from(config.limits.sprouts)),
                ),
                (
                    "questionsPerSession".to_owned(),
                    Value::Number(Number::from(config.limits.questions_per_session)),
                ),
                (
                    "proposalTtlDays".to_owned(),
                    Value::Number(Number::from(config.limits.proposal_ttl_days)),
                ),
                (
                    "sproutTtlDays".to_owned(),
                    Value::Number(Number::from(config.limits.sprout_ttl_days)),
                ),
            ]),
        ),
        (
            "questions".to_owned(),
            object([
                (
                    "doneAfterSessions".to_owned(),
                    Value::Number(Number::from(config.questions.done_after_sessions)),
                ),
                (
                    "bigBodyLines".to_owned(),
                    Value::Number(Number::from(config.questions.big_body_lines)),
                ),
                (
                    "orphanAfterDays".to_owned(),
                    Value::Number(Number::from(config.questions.orphan_after_days)),
                ),
                (
                    "staleAfterDays".to_owned(),
                    Value::Number(Number::from(config.questions.stale_after_days)),
                ),
                (
                    "snoozeDays".to_owned(),
                    Value::Number(Number::from(config.questions.snooze_days)),
                ),
            ]),
        ),
        (
            "view".to_owned(),
            object([("write".to_owned(), Value::Bool(config.view_write))]),
        ),
        (
            "flow".to_owned(),
            object([("heading".to_owned(), Value::Bool(config.flow_heading))]),
        ),
    ]
    .into_iter()
    .collect()
}

#[must_use]
pub fn render_config(config: &RootsConfig) -> String {
    stringify_yaml(&config_yaml(config)).expect("Roots config only contains supported YAML")
}

#[must_use]
pub fn render_initial_config(config: &RootsConfig) -> String {
    let quote = |value: &str| serde_json::to_string(value).expect("strings serialize");
    let command = match &config.agent.command {
        Some(value) if !value.is_empty() => format!("  command: {}", quote(value)),
        _ => {
            "  # command: \"claude -p --model claude-opus-5-5\"   # used by `think` to get questions"
                .to_owned()
        }
    };
    let l = &config.limits;
    let q = &config.questions;
    [
        format!("project: {}", quote(&config.project)),
        format!("version: {}", quote(&config.version)),
        format!("tier: {}                    # 0 off, 1 ask, 2 propose, 3 observe", config.tier),
        "agent:".to_owned(), command,
        format!("  timeoutSeconds: {}      # think continues without agent questions after this", config.agent.timeout_seconds),
        "limits:".to_owned(),
        format!("  proposals: {}            # max pending proposals", l.proposals),
        format!("  sprouts: {}               # max open sprouts", l.sprouts),
        format!("  questionsPerSession: {}", l.questions_per_session),
        format!("  proposalTtlDays: {}", l.proposal_ttl_days),
        format!("  sproutTtlDays: {}", l.sprout_ttl_days),
        "questions:                 # thresholds for deterministic question rules".to_owned(),
        format!("  doneAfterSessions: {}     # missing-done: ask from the Nth think session on", q.done_after_sessions),
        format!("  bigBodyLines: {}         # too-big: body longer than N lines", q.big_body_lines),
        format!("  orphanAfterDays: {}       # orphan: no edges N days after planting", q.orphan_after_days),
        format!("  staleAfterDays: {}       # stale: committed, untouched for N days", q.stale_after_days),
        format!("  snoozeDays: {}            # [z] in think hides a question for N days", q.snooze_days),
        "view:".to_owned(),
        format!("  write: {}             # true: `luuna-roots view` also writes ROOTS.md", config.view_write),
        "flow:".to_owned(),
        format!("  heading: {}           # `luuna-roots flow`: the agent says where the session is going", config.flow_heading),
        String::new(),
    ].join("\n")
}

pub fn write_config(paths: &RootsPaths, config: &RootsConfig) -> RootsResult<()> {
    atomic_write(&paths.config, &render_config(config))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn initial_config_round_trips_and_validation_is_strict() {
        let config = default_config("my app");
        assert_eq!(
            parse_config(&render_initial_config(&config), "x").expect("parse"),
            config
        );
        assert!(
            parse_config("tier: 7\n", "p")
                .unwrap_err()
                .message
                .contains("tier")
        );
        assert_eq!(
            parse_config("project: 42\n", "p")
                .expect("numeric project")
                .project,
            "42"
        );
    }
}
