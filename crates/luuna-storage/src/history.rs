use crate::*;

impl Store {
    /// Stable keyset paging; a new run does not shift already visited pages.
    pub fn runs_page(
        &self,
        before: Option<&str>,
        project: Option<&str>,
        limit: usize,
    ) -> Result<Vec<RunRecord>> {
        let anchor = before
            .map(|id| {
                self.get_run(id)?
                    .ok_or_else(|| StorageError::NotFound(id.into()))
            })
            .transpose()?;
        let mut query = self.connection.prepare("SELECT id,project_id,state,revision,metadata,created_at,updated_at FROM runs WHERE NOT EXISTS (SELECT 1 FROM projects p WHERE p.id=runs.project_id AND p.removed_at IS NOT NULL) AND (?1 IS NULL OR project_id=?1) AND (?2 IS NULL OR created_at<?2 OR (created_at=?2 AND id>?3)) ORDER BY created_at DESC,id LIMIT ?4")?;
        let rows = query.query_map(
            params![
                project,
                anchor.as_ref().map(|r| &r.created_at),
                before,
                limit.clamp(1, 200)
            ],
            run_row,
        )?;
        rows.map(|row| decode_run(row?)).collect()
    }

    /// Bound response allocation by both count and encoded bytes. One oversized
    /// event is allowed so a historical record can never make paging stall.
    pub fn events_page(
        &self,
        run_id: &str,
        after: u64,
        before: Option<u64>,
        limit: usize,
    ) -> Result<Vec<EventEnvelope>> {
        let sql = if before.is_some() {
            "SELECT envelope FROM events WHERE run_id=?1 AND sequence>?2 AND sequence<?3 ORDER BY sequence DESC LIMIT ?4"
        } else {
            "SELECT envelope FROM events WHERE run_id=?1 AND sequence>?2 AND sequence<?3 ORDER BY sequence LIMIT ?4"
        };
        let mut statement = self.connection.prepare(sql)?;
        let rows = statement.query_map(
            params![
                run_id,
                after,
                before.unwrap_or(i64::MAX as u64),
                limit.clamp(1, 200)
            ],
            |row| row.get::<_, String>(0),
        )?;
        let mut events = Vec::new();
        let mut bytes = 0;
        for row in rows {
            let value = row?;
            if !events.is_empty() && bytes + value.len() > 1024 * 1024 {
                break;
            }
            bytes += value.len();
            events.push(serde_json::from_str::<EventEnvelope>(&value)?);
        }
        if before.is_some() {
            events.reverse();
        }
        Ok(events)
    }
}
