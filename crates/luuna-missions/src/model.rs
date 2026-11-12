//! Ticket records as stored in `.luuna/missions/issues.jsonl`.
//!
//! Go calls these records issues; this API calls them tickets (D10). The
//! stored format is unchanged: a ticket keeps its raw JSON, its key order and
//! every unknown field, so rewriting an unchanged ticket gives the same bytes.
//!
//! The record layer is ported in task t2.

/// One ticket record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ticket {
    id: String,
}

impl Ticket {
    /// The ticket id, for example `project-1a2b`.
    pub fn id(&self) -> &str {
        &self.id
    }
}
