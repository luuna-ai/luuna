//! Durable Missions delivery intents and revision-aware ticket projections.
//! The intent is written before the external CLI call; a retry searches Missions
//! by the stable Luuna identity before it is allowed to create another record.
use crate::{append_project_event, now, Result, StorageError, Store};
use luuna_domain::{Ticket, TicketSource};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissionsBinding {
    pub ticket_id: String,
    pub source_id: Option<String>,
    pub source_revision: Option<String>,
    /// An attempted create cannot be repeated without finding its source record.
    pub delivery_started: bool,
    /// Original delivery content remains immutable for idempotency comparison.
    pub original: Ticket,
}

pub(crate) fn binding_in(connection: &Connection, id: &str) -> Result<Option<MissionsBinding>> {
    let row: Option<(Option<String>, Option<String>, String, bool)> = connection
        .query_row(
            "SELECT source_id,source_revision,original_payload,EXISTS(SELECT 1 FROM ticket_delivery_attempts WHERE ticket_id=?1) FROM ticket_sources WHERE ticket_id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    row.map(|(source_id, source_revision, payload, delivery_started)| {
        Ok(MissionsBinding {
            ticket_id: id.into(),
            source_id,
            source_revision,
            delivery_started,
            original: serde_json::from_str(&payload)?,
        })
    })
    .transpose()
}

impl Store {
    pub fn missions_binding(&self, id: &str) -> Result<Option<MissionsBinding>> {
        binding_in(&self.connection, id)
    }

    /// Reserve an external delivery without changing the ticket's authority.
    /// A missing source ID is deliberately durable across interrupted processes.
    pub fn reserve_missions_delivery(&mut self, id: &str) -> Result<MissionsBinding> {
        let ticket = self
            .get_ticket(id)?
            .ok_or_else(|| StorageError::NotFound(id.into()))?;
        self.connection.execute(
            "INSERT OR IGNORE INTO ticket_sources(ticket_id,project_id,original_payload) VALUES (?1,?2,?3)",
            params![id, ticket.project_id, serde_json::to_string(&ticket)?],
        )?;
        binding_in(&self.connection, id)?
            .ok_or_else(|| StorageError::Invalid("Missions delivery reservation missing".into()))
    }

    /// Persist before invoking create. A crash or timeout after this point is
    /// an uncertain external side effect, so a retry must locate that record.
    pub fn begin_missions_delivery(&mut self, id: &str) -> Result<()> {
        self.connection.execute(
            "INSERT INTO ticket_delivery_attempts(ticket_id) VALUES (?1)",
            [id],
        )?;
        Ok(())
    }

    /// Finish the initial external delivery. Identity and numeric revision are
    /// preserved so migration does not invalidate an otherwise exact old plan.
    pub fn complete_missions_delivery(
        &mut self,
        id: &str,
        expected_revision: u64,
        source_id: &str,
        source_revision: &str,
    ) -> Result<Ticket> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let binding = binding_in(&tx, id)?
            .ok_or_else(|| StorageError::Invalid("Missions delivery was not reserved".into()))?;
        let payload: String =
            tx.query_row("SELECT payload FROM tickets WHERE id=?1", [id], |r| {
                r.get(0)
            })?;
        let mut ticket: Ticket = serde_json::from_str(&payload)?;
        if ticket.revision != expected_revision {
            return Err(StorageError::RevisionConflict {
                expected: expected_revision,
                actual: ticket.revision,
            });
        }
        if let Some(ref existing) = binding.source_id {
            if existing != source_id || binding.source_revision.as_deref() != Some(source_revision)
            {
                return Err(StorageError::Invalid(
                    "Missions binding is immutable; reconcile changed source".into(),
                ));
            }
            return Ok(ticket);
        }
        if source_id.is_empty() || source_id.len() > 200 || !valid_revision(source_revision) {
            return Err(StorageError::Invalid(
                "invalid Missions source identity".into(),
            ));
        }
        ticket.source = TicketSource::Missions;
        ticket.validate()?;
        tx.execute(
            "UPDATE ticket_sources SET source_id=?1,source_revision=?2 WHERE ticket_id=?3 AND source_id IS NULL",
            params![source_id, source_revision, id],
        )?;
        tx.execute(
            "UPDATE tickets SET payload=?1 WHERE id=?2",
            params![serde_json::to_string(&ticket)?, id],
        )?;
        append_project_event(
            &tx,
            &ticket.project_id,
            "ticket.missions_bound",
            json!({
                "ticket_id":id,"revision":ticket.revision,"source_id":source_id,"source_revision":source_revision
            }),
        )?;
        tx.commit()?;
        self.mirror_logs(&[&ticket.project_id]);
        Ok(ticket)
    }

    /// Reconcile explicitly, advancing the projection revision even when a
    /// status/dependency edit left the visible intent unchanged. No local API
    /// independently writes Missions-owned fields.
    pub fn reconcile_missions_ticket(
        &mut self,
        projected: &Ticket,
        expected_revision: u64,
        source_id: &str,
        source_revision: &str,
    ) -> Result<Ticket> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let payload: String = tx.query_row(
            "SELECT payload FROM tickets WHERE id=?1",
            [&projected.id],
            |r| r.get(0),
        )?;
        let current: Ticket = serde_json::from_str(&payload)?;
        if current.revision != expected_revision {
            return Err(StorageError::RevisionConflict {
                expected: expected_revision,
                actual: current.revision,
            });
        }
        let binding = binding_in(&tx, &projected.id)?
            .ok_or_else(|| StorageError::Invalid("Missions binding missing".into()))?;
        if current.source != TicketSource::Missions
            || binding.source_id.as_deref() != Some(source_id)
            || projected.project_id != current.project_id
            || !valid_revision(source_revision)
        {
            return Err(StorageError::Invalid(
                "Missions projection identity mismatch".into(),
            ));
        }
        if binding.source_revision.as_deref() == Some(source_revision) {
            return Ok(current);
        }
        let mut ticket = projected.clone();
        ticket.source = TicketSource::Missions;
        ticket.revision = current.revision + 1;
        ticket.created_at = current.created_at;
        ticket.updated_at = now();
        ticket.validate()?;
        let registered: Vec<_> = crate::projects::repositories_in(&tx, &ticket.project_id)?
            .into_iter()
            .map(|r| r.repository_id)
            .collect();
        if ticket
            .repository_ids
            .iter()
            .any(|id| !registered.contains(id))
        {
            return Err(StorageError::Invalid(
                "Missions ticket names an unregistered repository".into(),
            ));
        }
        tx.execute(
            "UPDATE tickets SET revision=?1,payload=?2,updated_at=?3 WHERE id=?4 AND revision=?5",
            params![
                ticket.revision,
                serde_json::to_string(&ticket)?,
                ticket.updated_at,
                ticket.id,
                expected_revision
            ],
        )?;
        tx.execute(
            "UPDATE ticket_sources SET source_revision=?1 WHERE ticket_id=?2",
            params![source_revision, ticket.id],
        )?;
        append_project_event(
            &tx,
            &ticket.project_id,
            "ticket.reconciled",
            json!({
                "ticket_id":ticket.id,"revision":ticket.revision,"source_revision":source_revision
            }),
        )?;
        tx.commit()?;
        self.mirror_logs(&[&ticket.project_id]);
        Ok(ticket)
    }
}

fn valid_revision(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
