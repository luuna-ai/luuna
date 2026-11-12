//! Durable component activation and uncertain-side-effect reconciliation.
use crate::{now, Result, StorageError, Store};
use luuna_domain::hash_value;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentOperation {
    pub record_id: String,
    pub response: Option<Value>,
    pub attempted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryBinding {
    pub id: String,
    pub project_id: String,
    pub scope: String,
    pub source_id: String,
    pub domain: String,
    pub created_at: String,
    pub deleted: bool,
}

/// A project's memory retention policy: the longest age, in days, a record of
/// each scope may reach before `POST …/memory/retention/apply` deletes it.
/// `None` keeps records of that scope indefinitely. User memory is personal and
/// never subject to a project policy.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryRetentionPolicy {
    #[serde(default)]
    pub process_max_age_days: Option<u32>,
    #[serde(default)]
    pub project_max_age_days: Option<u32>,
}

impl MemoryRetentionPolicy {
    /// The policy's limit for one scope, if any.
    pub fn max_age_days(&self, scope: &str) -> Option<u32> {
        match scope {
            "process" => self.process_max_age_days,
            "project" => self.project_max_age_days,
            _ => None,
        }
    }
}

impl Store {
    pub fn initialize_component_state(&self) -> Result<()> {
        self.connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS component_projects (
                project_id TEXT PRIMARY KEY REFERENCES projects(id), state TEXT NOT NULL,
                updated_at TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS component_operations (
                project_id TEXT NOT NULL REFERENCES projects(id), operation_key TEXT NOT NULL,
                kind TEXT NOT NULL, request_hash TEXT NOT NULL, record_id TEXT NOT NULL,
                response TEXT, created_at TEXT NOT NULL, attempted INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(project_id,operation_key));
             CREATE TABLE IF NOT EXISTS component_memories (
                id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES projects(id),
                scope TEXT NOT NULL, source_id TEXT NOT NULL, domain TEXT NOT NULL,
                created_at TEXT NOT NULL, deleted INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE IF NOT EXISTS memory_retention (
                project_id TEXT PRIMARY KEY REFERENCES projects(id), policy TEXT NOT NULL,
                updated_at TEXT NOT NULL);",
        )?;
        let columns = self
            .connection
            .prepare("PRAGMA table_info(component_operations)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if !columns.iter().any(|column| column == "attempted") {
            self.connection.execute_batch(
                "ALTER TABLE component_operations ADD COLUMN attempted INTEGER NOT NULL DEFAULT 0;",
            )?;
        }
        Ok(())
    }

    pub fn component_state(&self, project_id: &str) -> Result<Option<String>> {
        Ok(self
            .connection
            .query_row(
                "SELECT state FROM component_projects WHERE project_id=?1",
                [project_id],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn begin_component_enable(&self, project_id: &str) -> Result<()> {
        self.connection.execute(
            "INSERT INTO component_projects(project_id,state,updated_at) VALUES (?1,'initializing',?2)
             ON CONFLICT(project_id) DO NOTHING", params![project_id, now()],
        )?;
        Ok(())
    }

    pub fn finish_component_enable(&self, project_id: &str) -> Result<()> {
        self.connection.execute(
            "UPDATE component_projects SET state='enabled',updated_at=?2 WHERE project_id=?1",
            params![project_id, now()],
        )?;
        Ok(())
    }

    pub fn begin_component_operation(
        &self,
        project_id: &str,
        key: &str,
        kind: &str,
        request: &Value,
        proposed_record_id: &str,
    ) -> Result<ComponentOperation> {
        if key.is_empty() || key.len() > 200 || key.chars().any(char::is_control) {
            return Err(StorageError::Invalid(
                "a bounded idempotency key is required".into(),
            ));
        }
        let hash = hash_value(request)?;
        let previous: Option<(String, String, String, Option<String>, bool)> = self.connection.query_row(
            "SELECT kind,request_hash,record_id,response,attempted FROM component_operations WHERE project_id=?1 AND operation_key=?2",
            params![project_id,key], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)),
        ).optional()?;
        if let Some((old_kind, old_hash, record_id, response, attempted)) = previous {
            if old_kind != kind || old_hash != hash {
                return Err(StorageError::Invalid(
                    "idempotency key already used for a different component operation".into(),
                ));
            }
            return Ok(ComponentOperation {
                record_id,
                response: response
                    .map(|text| serde_json::from_str(&text))
                    .transpose()?,
                attempted,
            });
        }
        self.connection.execute(
            "INSERT INTO component_operations(project_id,operation_key,kind,request_hash,record_id,created_at) VALUES (?1,?2,?3,?4,?5,?6)",
            params![project_id,key,kind,hash,proposed_record_id,now()],
        )?;
        Ok(ComponentOperation {
            record_id: proposed_record_id.into(),
            response: None,
            attempted: false,
        })
    }

    pub fn mark_component_operation_attempted(&self, project_id: &str, key: &str) -> Result<()> {
        self.connection.execute(
            "UPDATE component_operations SET attempted=1 WHERE project_id=?1 AND operation_key=?2",
            params![project_id, key],
        )?;
        Ok(())
    }

    pub fn complete_component_operation(
        &self,
        project_id: &str,
        key: &str,
        response: &Value,
    ) -> Result<()> {
        self.connection.execute(
            "UPDATE component_operations SET response=?3 WHERE project_id=?1 AND operation_key=?2",
            params![project_id, key, serde_json::to_string(response)?],
        )?;
        Ok(())
    }

    pub fn record_memory_binding(&self, binding: &MemoryBinding) -> Result<()> {
        self.connection.execute(
            "INSERT INTO component_memories(id,project_id,scope,source_id,domain,created_at) VALUES (?1,?2,?3,?4,?5,?6)
             ON CONFLICT(id) DO NOTHING",
            params![binding.id,binding.project_id,binding.scope,binding.source_id,binding.domain,binding.created_at],
        )?;
        Ok(())
    }

    pub fn memory_bindings(
        &self,
        project_id: &str,
        include_user: bool,
    ) -> Result<Vec<MemoryBinding>> {
        let mut statement = self.connection.prepare(
            "SELECT id,project_id,scope,source_id,domain,created_at,deleted FROM component_memories
             WHERE deleted=0 AND ((project_id=?1 AND scope!='user') OR (?2=1 AND scope='user')) ORDER BY created_at,id",
        )?;
        let rows = statement.query_map(params![project_id, include_user], |row| {
            Ok(MemoryBinding {
                id: row.get(0)?,
                project_id: row.get(1)?,
                scope: row.get(2)?,
                source_id: row.get(3)?,
                domain: row.get(4)?,
                created_at: row.get(5)?,
                deleted: row.get(6)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// The recorded intent under `key`, if one was begun, without creating one.
    /// A multi-step operation that died between steps resumes from it; a key
    /// the ledger has never seen is `None`.
    pub fn component_operation(
        &self,
        project_id: &str,
        key: &str,
    ) -> Result<Option<ComponentOperation>> {
        let row: Option<(String, Option<String>, bool)> = self
            .connection
            .query_row(
                "SELECT record_id,response,attempted FROM component_operations WHERE project_id=?1 AND operation_key=?2",
                params![project_id, key],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        row.map(|(record_id, response, attempted)| {
            Ok(ComponentOperation {
                record_id,
                response: response
                    .map(|text| serde_json::from_str(&text))
                    .transpose()?,
                attempted,
            })
        })
        .transpose()
    }

    /// The recorded response of a finished component operation, if any; used to
    /// resume a multi-step operation whose later step died before completion.
    pub fn component_operation_response(
        &self,
        project_id: &str,
        key: &str,
    ) -> Result<Option<Value>> {
        let response: Option<Option<String>> = self
            .connection
            .query_row(
                "SELECT response FROM component_operations WHERE project_id=?1 AND operation_key=?2",
                params![project_id, key],
                |row| row.get(0),
            )
            .optional()?;
        Ok(response
            .flatten()
            .map(|text| serde_json::from_str(&text))
            .transpose()?)
    }

    pub fn delete_memory_binding(&self, id: &str) -> Result<()> {
        self.connection
            .execute("UPDATE component_memories SET deleted=1 WHERE id=?1", [id])?;
        Ok(())
    }

    /// Every live user-scope memory binding, whichever project created it:
    /// user memory lives in the one private user store, not in a project.
    pub fn user_memory_bindings(&self) -> Result<Vec<MemoryBinding>> {
        let mut statement = self.connection.prepare(
            "SELECT id,project_id,scope,source_id,domain,created_at,deleted FROM component_memories
             WHERE deleted=0 AND scope='user' ORDER BY created_at,id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(MemoryBinding {
                id: row.get(0)?,
                project_id: row.get(1)?,
                scope: row.get(2)?,
                source_id: row.get(3)?,
                domain: row.get(4)?,
                created_at: row.get(5)?,
                deleted: row.get(6)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// The project's retention policy; the default keeps everything.
    pub fn memory_retention(&self, project_id: &str) -> Result<MemoryRetentionPolicy> {
        let policy: Option<String> = self
            .connection
            .query_row(
                "SELECT policy FROM memory_retention WHERE project_id=?1",
                [project_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(policy
            .map(|text| serde_json::from_str(&text))
            .transpose()?
            .unwrap_or_default())
    }

    /// Replace the project's retention policy and record `memory.retention_set`.
    pub fn set_memory_retention(
        &mut self,
        project_id: &str,
        policy: &MemoryRetentionPolicy,
    ) -> Result<MemoryRetentionPolicy> {
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        crate::projects::get_project_in(&tx, project_id)?
            .ok_or_else(|| StorageError::NotFound(project_id.into()))?;
        tx.execute(
            "INSERT INTO memory_retention(project_id,policy,updated_at) VALUES (?1,?2,?3)
             ON CONFLICT(project_id) DO UPDATE SET policy=excluded.policy,updated_at=excluded.updated_at",
            params![project_id, serde_json::to_string(policy)?, now()],
        )?;
        crate::projects::append_project_event(
            &tx,
            project_id,
            "memory.retention_set",
            serde_json::to_value(policy)?,
        )?;
        let file_state =
            crate::project_index::write_project_file(&tx, self.data_dir.as_deref(), project_id)?;
        crate::project_index::commit_or_restore(tx, file_state)?;
        Ok(policy.clone())
    }

    /// Test seam: rewrite a binding's recorded creation time so retention can
    /// be exercised without waiting. Not a product operation.
    #[doc(hidden)]
    pub fn set_memory_binding_created_at(&self, id: &str, created_at: &str) -> Result<()> {
        self.connection.execute(
            "UPDATE component_memories SET created_at=?2 WHERE id=?1",
            params![id, created_at],
        )?;
        Ok(())
    }

    pub fn memory_binding(&self, project_id: &str, id: &str) -> Result<Option<MemoryBinding>> {
        Ok(self.connection.query_row(
            "SELECT id,project_id,scope,source_id,domain,created_at,deleted FROM component_memories
             WHERE id=?2 AND (project_id=?1 OR scope='user')",
            params![project_id,id], |row| Ok(MemoryBinding {
                id:row.get(0)?,project_id:row.get(1)?,scope:row.get(2)?,source_id:row.get(3)?,domain:row.get(4)?,created_at:row.get(5)?,deleted:row.get(6)?,
            }),
        ).optional()?)
    }
}
