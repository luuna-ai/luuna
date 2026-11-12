//! JSONL record shapes for `.luuna/roots/`.
//!
//! These types deliberately use the same camel-case field names and tagged
//! records as the former TypeScript implementation.  The store module keeps
//! the original record order; serde only supplies the wire conversion here.

use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const IDEA_STATUSES: [&str; 5] = ["planted", "shaping", "committed", "built", "composted"];
pub const SPROUT_STATUSES: [&str; 4] = ["open", "adopted", "rejected", "expired"];
pub const NODE_KINDS: [&str; 2] = ["idea", "sprout"];
pub const EDGE_RELS: [&str; 4] = ["serves", "tension", "replaces", "derives"];

pub type Actor = String;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum IdeaStatus {
    Planted,
    Shaping,
    Committed,
    Built,
    Composted,
}

impl IdeaStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Planted => "planted",
            Self::Shaping => "shaping",
            Self::Committed => "committed",
            Self::Built => "built",
            Self::Composted => "composted",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SproutStatus {
    Open,
    Adopted,
    Rejected,
    Expired,
}

impl SproutStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Adopted => "adopted",
            Self::Rejected => "rejected",
            Self::Expired => "expired",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum NodeStatus {
    Idea(IdeaStatus),
    Sprout(SproutStatus),
}

impl NodeStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idea(status) => status.as_str(),
            Self::Sprout(status) => status.as_str(),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeKind {
    Idea,
    Sprout,
}

impl NodeKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idea => "idea",
            Self::Sprout => "sprout",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EdgeRel {
    Serves,
    Tension,
    Replaces,
    Derives,
}

impl EdgeRel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Serves => "serves",
            Self::Tension => "tension",
            Self::Replaces => "replaces",
            Self::Derives => "derives",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Endorsement {
    pub sprout: String,
    pub hash: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NodeRecord {
    #[serde(rename = "type")]
    pub record_type: String,
    pub id: String,
    pub kind: NodeKind,
    pub slug: String,
    pub status: NodeStatus,
    pub author: Actor,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "updatedAt", skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tier: Option<u8>,
    #[serde(rename = "expiresAt", skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(rename = "decidedBy", skip_serializing_if = "Option::is_none")]
    pub decided_by: Option<Actor>,
    #[serde(rename = "decidedAt", skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<String>,
    #[serde(rename = "decisionReason", skip_serializing_if = "Option::is_none")]
    pub decision_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endorses: Option<Endorsement>,
}

impl NodeRecord {
    pub fn new(
        id: String,
        kind: NodeKind,
        slug: String,
        status: NodeStatus,
        author: Actor,
        created_at: String,
    ) -> Self {
        Self {
            record_type: "node".to_owned(),
            id,
            kind,
            slug,
            status,
            author,
            created_at: created_at.clone(),
            updated_at: Some(created_at),
            tier: None,
            expires_at: None,
            decided_by: None,
            decided_at: None,
            decision_reason: None,
            endorses: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EdgeRecord {
    #[serde(rename = "type")]
    pub record_type: String,
    pub id: String,
    pub from: String,
    pub to: String,
    pub rel: EdgeRel,
    pub by: Actor,
    #[serde(rename = "proposedBy", skip_serializing_if = "Option::is_none")]
    pub proposed_by: Option<Actor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposal: Option<String>,
    #[serde(rename = "createdAt")]
    pub created_at: String,
}

impl EdgeRecord {
    #[must_use]
    pub fn is_node(&self) -> bool {
        false
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GraphRecord {
    Node(NodeRecord),
    Edge(EdgeRecord),
}

impl GraphRecord {
    #[must_use]
    pub fn id(&self) -> &str {
        match self {
            Self::Node(record) => &record.id,
            Self::Edge(record) => &record.id,
        }
    }
}

impl crate::store::HasId for NodeRecord {
    fn id(&self) -> &str {
        &self.id
    }
}

impl crate::store::HasId for EdgeRecord {
    fn id(&self) -> &str {
        &self.id
    }
}

impl crate::store::HasId for HeadingRecord {
    fn id(&self) -> &str {
        &self.id
    }
}

impl crate::store::HasId for ProposalRecord {
    fn id(&self) -> &str {
        &self.id
    }
}

impl crate::store::HasId for QuestionRecord {
    fn id(&self) -> &str {
        &self.id
    }
}

impl Serialize for GraphRecord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Node(record) => record.serialize(serializer),
            Self::Edge(record) => record.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for GraphRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        let record_type = value
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| serde::de::Error::missing_field("type"))?;
        match record_type {
            "node" => serde_json::from_value(value)
                .map(Self::Node)
                .map_err(serde::de::Error::custom),
            "edge" => serde_json::from_value(value)
                .map(Self::Edge)
                .map_err(serde::de::Error::custom),
            other => Err(serde::de::Error::custom(format!(
                "unknown graph record type {other}"
            ))),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Graph {
    pub nodes: Vec<NodeRecord>,
    pub edges: Vec<EdgeRecord>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Citation {
    pub node: String,
    pub quote: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HeadingStatus {
    Active,
    Dismissed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HeadingRecord {
    pub id: String,
    pub flow: String,
    pub text: String,
    pub cites: Vec<Citation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    pub by: Actor,
    pub status: HeadingStatus,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "dismissedBy", skip_serializing_if = "Option::is_none")]
    pub dismissed_by: Option<Actor>,
    #[serde(rename = "dismissedAt", skip_serializing_if = "Option::is_none")]
    pub dismissed_at: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProposalKind {
    Edge,
    Split,
    Merge,
    Compost,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProposalStatus {
    Pending,
    Accepted,
    Rejected,
    Expired,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct ProposalRecord {
    pub id: String,
    pub kind: ProposalKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    pub rel: Option<EdgeRel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub cites: Vec<Citation>,
    pub by: Actor,
    pub status: ProposalStatus,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "expiresAt", skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(rename = "decidedBy", skip_serializing_if = "Option::is_none")]
    pub decided_by: Option<Actor>,
    #[serde(rename = "decidedAt", skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<String>,
    #[serde(rename = "decisionReason", skip_serializing_if = "Option::is_none")]
    pub decision_reason: Option<String>,
}

/// Proposal JSONL has one intentional nullable field: an edge proposal may
/// carry `"rel": null` (a mention proposal). Other proposal kinds omit it.
/// A regular derive cannot express that conditional wire shape.
impl Serialize for ProposalRecord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("id", &self.id)?;
        map.serialize_entry("kind", &self.kind)?;
        if let Some(value) = &self.from {
            map.serialize_entry("from", value)?;
        }
        if let Some(value) = &self.to {
            map.serialize_entry("to", value)?;
        }
        if self.kind == ProposalKind::Edge {
            map.serialize_entry("rel", &self.rel)?;
        }
        if let Some(value) = &self.reason {
            map.serialize_entry("reason", value)?;
        }
        map.serialize_entry("cites", &self.cites)?;
        map.serialize_entry("by", &self.by)?;
        map.serialize_entry("status", &self.status)?;
        map.serialize_entry("createdAt", &self.created_at)?;
        if let Some(value) = &self.expires_at {
            map.serialize_entry("expiresAt", value)?;
        }
        if let Some(value) = &self.decided_by {
            map.serialize_entry("decidedBy", value)?;
        }
        if let Some(value) = &self.decided_at {
            map.serialize_entry("decidedAt", value)?;
        }
        if let Some(value) = &self.decision_reason {
            map.serialize_entry("decisionReason", value)?;
        }
        map.end()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum QuestionStatus {
    Open,
    Answered,
    Dismissed,
    Snoozed,
    Delegated,
}

impl QuestionStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Answered => "answered",
            Self::Dismissed => "dismissed",
            Self::Snoozed => "snoozed",
            Self::Delegated => "delegated",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QuestionFinding {
    pub note: String,
    pub by: Actor,
    pub at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QuestionRecord {
    pub id: String,
    pub node: String,
    pub text: String,
    pub by: Actor,
    pub status: QuestionStatus,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#ref: Option<String>,
    #[serde(rename = "answeredBy", skip_serializing_if = "Option::is_none")]
    pub answered_by: Option<Actor>,
    #[serde(rename = "answeredAt", skip_serializing_if = "Option::is_none")]
    pub answered_at: Option<String>,
    #[serde(rename = "dismissedBy", skip_serializing_if = "Option::is_none")]
    pub dismissed_by: Option<Actor>,
    #[serde(rename = "dismissedAt", skip_serializing_if = "Option::is_none")]
    pub dismissed_at: Option<String>,
    #[serde(rename = "delegatedBy", skip_serializing_if = "Option::is_none")]
    pub delegated_by: Option<Actor>,
    #[serde(rename = "delegatedAt", skip_serializing_if = "Option::is_none")]
    pub delegated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub findings: Option<Vec<QuestionFinding>>,
    #[serde(rename = "snoozedUntil", skip_serializing_if = "Option::is_none")]
    pub snoozed_until: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

pub type EventType = String;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EventRecord {
    #[serde(rename = "type")]
    pub event_type: EventType,
    pub by: Actor,
    pub at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refs: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl EventRecord {
    #[must_use]
    pub fn new(event_type: impl Into<String>, by: Actor, at: String) -> Self {
        Self {
            event_type: event_type.into(),
            by,
            at,
            node: None,
            refs: None,
            extra: Map::new(),
        }
    }
}

#[must_use]
pub fn is_idea_status(value: &str) -> bool {
    IDEA_STATUSES.contains(&value)
}

#[must_use]
pub fn is_sprout_status(value: &str) -> bool {
    SPROUT_STATUSES.contains(&value)
}

#[must_use]
pub fn is_node_kind(value: &str) -> bool {
    NODE_KINDS.contains(&value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_records_keep_the_type_first_in_json() {
        let record = GraphRecord::Node(NodeRecord::new(
            "r-a1b2".to_owned(),
            NodeKind::Idea,
            "offline".to_owned(),
            NodeStatus::Idea(IdeaStatus::Planted),
            "human:jay".to_owned(),
            "2026-10-22T00:00:00Z".to_owned(),
        ));
        let text = serde_json::to_string(&record).expect("serialize graph record");
        assert!(text.starts_with(r#"{"type":"node""#));
    }

    #[test]
    fn event_extra_fields_round_trip() {
        let mut event = EventRecord::new("plant", "human:jay".to_owned(), "now".to_owned());
        event.extra.insert(
            "statement".to_owned(),
            Value::String("Offline first".to_owned()),
        );
        let back: EventRecord =
            serde_json::from_str(&serde_json::to_string(&event).expect("serialize event"))
                .expect("deserialize event");
        assert_eq!(back.extra["statement"], "Offline first");
    }

    #[test]
    fn mention_edge_keeps_its_explicit_null_relation() {
        let proposal = ProposalRecord {
            id: "p-a1b2".into(),
            kind: ProposalKind::Edge,
            from: Some("r-a1b2".into()),
            to: Some("r-b2c3".into()),
            rel: None,
            reason: Some("mentioned".into()),
            cites: vec![],
            by: "roots:mention".into(),
            status: ProposalStatus::Pending,
            created_at: "now".into(),
            expires_at: None,
            decided_by: None,
            decided_at: None,
            decision_reason: None,
        };
        assert!(
            serde_json::to_string(&proposal)
                .unwrap()
                .contains("\"rel\":null")
        );
        let split = ProposalRecord {
            kind: ProposalKind::Split,
            ..proposal
        };
        assert!(!serde_json::to_string(&split).unwrap().contains("\"rel\""));
    }
}
