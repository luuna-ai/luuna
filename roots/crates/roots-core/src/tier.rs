//! Agent tier resolution and enforcement.
//!
//! Tiers only control which agent suggestions may be recorded.  They never
//! authorize edits to human-authored Roots prose.

use crate::config::{RootsConfig, Tier};
use crate::errors::{RootsError, RootsResult};
use crate::model::NodeRecord;

pub const TIER_NAMES: [&str; 4] = ["off", "ask", "propose", "observe"];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TierSource {
    Idea,
    Config,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EffectiveTier {
    pub tier: Tier,
    pub name: &'static str,
    pub source: TierSource,
}

#[must_use]
pub const fn is_tier(value: u8) -> bool {
    value <= 3
}

#[must_use]
pub const fn tier_name(value: Tier) -> &'static str {
    match value {
        0 => "off",
        1 => "ask",
        2 => "propose",
        3 => "observe",
        _ => "off",
    }
}

pub fn parse_tier(raw: &str) -> RootsResult<Tier> {
    let value = raw.trim().to_lowercase();
    if let Some(index) = TIER_NAMES.iter().position(|name| *name == value) {
        return Ok(index as Tier);
    }
    if value.len() == 1 && matches!(value.as_bytes()[0], b'0'..=b'3') {
        return Ok(value.as_bytes()[0] - b'0');
    }
    Err(RootsError::usage(format!(
        "tier must be 0-3 ({}), got \"{raw}\"",
        TIER_NAMES.join(", ")
    )))
}

#[must_use]
pub fn effective_tier(config: &RootsConfig, node: Option<&NodeRecord>) -> EffectiveTier {
    if let Some(value) = node
        .and_then(|node| node.tier)
        .filter(|value| is_tier(*value))
    {
        return EffectiveTier {
            tier: value,
            name: tier_name(value),
            source: TierSource::Idea,
        };
    }
    EffectiveTier {
        tier: config.tier,
        name: tier_name(config.tier),
        source: TierSource::Config,
    }
}

/// Refuse an agent action that is not enabled by the project's effective tier.
pub fn require_tier(
    config: &RootsConfig,
    minimum: Tier,
    action: &str,
    node: Option<&NodeRecord>,
) -> RootsResult<EffectiveTier> {
    let effective = effective_tier(config, node);
    if effective.tier >= minimum {
        return Ok(effective);
    }
    let need = format!("tier {minimum} ({})", tier_name(minimum));
    let have = format!("tier {} ({})", effective.tier, effective.name);
    if effective.source == TierSource::Idea {
        let node = node.expect("an idea-sourced tier always has a node");
        return Err(RootsError::guard(format!(
            "{action} needs agent {need}; {} is at {have} by a per-idea override. Only a human can raise it: `luuna-roots tier {} {minimum}`",
            node.id, node.id
        )));
    }
    let suffix = node.map_or_else(String::new, |node| {
        format!(" or run `luuna-roots tier {} {minimum}`", node.id)
    });
    Err(RootsError::guard(format!(
        "{action} needs agent {need}; this project is at {have}. Only a human can raise it: set `tier: {minimum}` in .luuna/roots/config.yaml{suffix}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::default_config;
    use crate::model::{IdeaStatus, NodeKind, NodeStatus};

    #[test]
    fn node_override_wins_and_names_the_human_fix() {
        let mut node = NodeRecord::new(
            "r-a1b2".into(),
            NodeKind::Idea,
            "a".into(),
            NodeStatus::Idea(IdeaStatus::Planted),
            "human:test".into(),
            "now".into(),
        );
        node.tier = Some(0);
        let error = require_tier(&default_config("p"), 1, "`luuna-roots ask`", Some(&node))
            .expect_err("override should refuse");
        assert!(error.message.contains("luuna-roots tier r-a1b2 1"));
        assert_eq!(parse_tier("PROPOSE").unwrap(), 2);
    }
}
