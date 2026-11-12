//! Project registry, immutable manifest revisions, machine-local repository
//! registrations and the project-scoped durable event stream.
//!
//! The manifest is machine-independent and hashed; local paths and observed
//! commits live only in this private registry. Confirmation re-checks the
//! registered identity and the expected revision inside one transaction. A
//! checkout is registered at most once, and only an explicit rebind changes
//! where a registered repository lives or which identity it carries.
use crate::{append_envelope, now, state_json, Result, StorageError, Store};
use luuna_domain::{
    remotes_equivalent, valid_project_id, Discovery, EventEnvelope, ProjectManifest,
    RepositoryObservation, RunState,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectRecord {
    pub id: String,
    pub name: String,
    /// Confirmed manifest revision; 0 until the first confirmation.
    pub revision: u64,
    pub manifest_hash: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Machine-registry entry: where a registered repository lives on this host and
/// the identity it had when registered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepositoryRegistration {
    pub project_id: String,
    pub repository_id: String,
    pub local_path: String,
    pub observation: RepositoryObservation,
    pub registered_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestRecord {
    pub project_id: String,
    pub revision: u64,
    pub hash: String,
    pub manifest: ProjectManifest,
    /// Exact HEAD commit of each repository when this revision was confirmed.
    pub base_commits: BTreeMap<String, String>,
    pub confirmed_at: String,
}

pub fn project_stream_id(project_id: &str) -> String {
    format!("project:{project_id}")
}

impl Store {
    /// Register a project and its repositories from fresh read-only observations.
    pub fn register_project(
        &mut self,
        id: &str,
        name: &str,
        repositories: &[(String, RepositoryObservation)],
    ) -> Result<ProjectRecord> {
        if !valid_project_id(id) {
            return Err(StorageError::Invalid(
                "project ID must match ^[a-z][a-z0-9-]{0,63}$".into(),
            ));
        }
        if name.trim().is_empty() || name.len() > 200 {
            return Err(StorageError::Invalid(
                "project name must be 1..200 bytes".into(),
            ));
        }
        if repositories.is_empty() || repositories.len() > 32 {
            return Err(StorageError::Invalid(
                "register between one and 32 repositories".into(),
            ));
        }
        let mut ids = BTreeSet::new();
        for (repository_id, observation) in repositories {
            if !valid_project_id(repository_id) || !ids.insert(repository_id.as_str()) {
                return Err(StorageError::Invalid(
                    "repository IDs must be unique and match ^[a-z][a-z0-9-]*$".into(),
                ));
            }
            observation.validate()?;
        }
        let now = now();
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if get_project(&tx, id)?.is_some() {
            return Err(StorageError::AlreadyExists(id.into()));
        }
        for (_, observation) in repositories {
            if let Some((project, repository)) = registration_at(&tx, &observation.local_path)? {
                return Err(StorageError::Invalid(already_registered(
                    &observation.local_path,
                    &project,
                    &repository,
                )));
            }
        }
        tx.execute(
            "INSERT INTO projects(id,name,revision,manifest_hash,created_at,updated_at) VALUES (?1,?2,0,NULL,?3,?3)",
            params![id, name, now],
        )?;
        append_project_event(
            &tx,
            id,
            "project.registered",
            json!({"name":name,"repositories":ids}),
        )?;
        for (repository_id, observation) in repositories {
            tx.execute(
                "INSERT INTO repositories(project_id,id,local_path,observation,registered_at) VALUES (?1,?2,?3,?4,?5)",
                params![
                    id,
                    repository_id,
                    observation.local_path,
                    serde_json::to_string(observation)?,
                    now
                ],
            )?;
            append_project_event(
                &tx,
                id,
                "repository.registered",
                json!({"repository_id":repository_id,"observation":observation}),
            )?;
        }
        let file_state =
            crate::project_index::write_project_file(&tx, self.data_dir.as_deref(), id)?;
        crate::project_index::commit_or_restore(tx, file_state)?;
        self.get_project(id)?
            .ok_or_else(|| StorageError::NotFound(id.into()))
    }

    pub fn get_project(&self, id: &str) -> Result<Option<ProjectRecord>> {
        get_project(&self.connection, id)
    }

    pub fn projects(&self) -> Result<Vec<ProjectRecord>> {
        let mut query = self.connection.prepare(
            "SELECT id,name,revision,manifest_hash,created_at,updated_at FROM projects ORDER BY created_at DESC,id",
        )?;
        let rows = query.query_map([], project_row)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn repositories(&self, project_id: &str) -> Result<Vec<RepositoryRegistration>> {
        repositories(&self.connection, project_id)
    }

    /// The project and repository IDs registered at exactly this local path.
    pub fn registration_at_path(&self, local_path: &str) -> Result<Option<(String, String)>> {
        registration_at(&self.connection, local_path)
    }

    /// Runs of the project whose work may be executing in a registered checkout
    /// (a queued or running baseline, an exploring or implementing workflow).
    pub fn active_checkout_runs(&self, project_id: &str) -> Result<Vec<String>> {
        let states = [
            RunState::Queued,
            RunState::Running,
            RunState::Exploring,
            RunState::Implementing,
        ]
        .into_iter()
        .map(state_json)
        .collect::<Result<Vec<_>>>()?;
        let mut statement = self.connection.prepare(
            "SELECT id FROM runs WHERE project_id=?1 AND state IN (?2,?3,?4,?5) ORDER BY created_at,id",
        )?;
        let rows = statement.query_map(
            params![project_id, states[0], states[1], states[2], states[3]],
            |row| row.get::<_, String>(0),
        )?;
        rows.map(|row| row.map_err(Into::into)).collect()
    }

    /// Explicitly re-point a registered repository at a checkout and identity
    /// observed now. Nothing changes and `None` is returned when the path and
    /// identity are already the registered ones; otherwise the registration is
    /// replaced and a `repository.rebound` event records the old and new
    /// bindings. The manifest is untouched: a confirmed manifest whose remote no
    /// longer matches must be confirmed again (confirmation enforces the match),
    /// and base commits, baselines and approvals stay bound to exact commits.
    pub fn rebind_repository(
        &mut self,
        project_id: &str,
        repository_id: &str,
        observation: &RepositoryObservation,
    ) -> Result<Option<EventEnvelope>> {
        observation.validate()?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        get_project(&tx, project_id)?.ok_or_else(|| StorageError::NotFound(project_id.into()))?;
        let previous = repositories(&tx, project_id)?
            .into_iter()
            .find(|r| r.repository_id == repository_id)
            .ok_or_else(|| StorageError::NotFound(repository_id.into()))?;
        if let Some((project, repository)) = registration_at(&tx, &observation.local_path)? {
            if project != project_id || repository != repository_id {
                return Err(StorageError::Invalid(already_registered(
                    &observation.local_path,
                    &project,
                    &repository,
                )));
            }
        }
        if previous.local_path == observation.local_path
            && previous.observation.identity == observation.identity
        {
            return Ok(None);
        }
        // Without a remote, a manifest carries an explicit local: identity.
        let manifest_remote_matches = current_manifest(&tx, project_id)?.map(|manifest| {
            manifest
                .repositories
                .iter()
                .find(|r| r.id == repository_id)
                .is_some_and(|r| match &observation.identity.remote {
                    Some(remote) => remotes_equivalent(&r.remote, remote),
                    None => r.remote.starts_with("local:"),
                })
        });
        tx.execute(
            "UPDATE repositories SET local_path=?1,observation=?2,registered_at=?3 WHERE project_id=?4 AND id=?5",
            params![
                observation.local_path,
                serde_json::to_string(observation)?,
                now(),
                project_id,
                repository_id
            ],
        )?;
        let manifest_confirmation_required = manifest_remote_matches == Some(false)
            || previous
                .observation
                .identity
                .matches(&observation.identity)
                .is_err();
        if manifest_confirmation_required {
            // Retain append-only manifest history and revision, but withdraw the
            // current confirmation so all approval/execution gates fail closed.
            tx.execute(
                "UPDATE projects SET manifest_hash=NULL WHERE id=?1",
                [project_id],
            )?;
        }
        let event = append_project_event(
            &tx,
            project_id,
            "repository.rebound",
            json!({
                "repository_id": repository_id,
                "old": {
                    "local_path": previous.local_path,
                    "identity": previous.observation.identity,
                },
                "new": {
                    "local_path": observation.local_path,
                    "identity": observation.identity,
                },
                "observation": observation,
                "manifest_remote_matches": manifest_remote_matches,
                "manifest_confirmation_required": manifest_confirmation_required,
            }),
        )?;
        let file_state =
            crate::project_index::write_project_file(&tx, self.data_dir.as_deref(), project_id)?;
        crate::project_index::commit_or_restore(tx, file_state)?;
        Ok(Some(event))
    }

    /// Persist a proposal. It is evidence for the developer, not a confirmation.
    pub fn record_discovery(&mut self, discovery: &Discovery) -> Result<()> {
        if discovery.id.trim().is_empty() {
            return Err(StorageError::Invalid("discovery ID is required".into()));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        get_project(&tx, &discovery.project_id)?
            .ok_or_else(|| StorageError::NotFound(discovery.project_id.clone()))?;
        tx.execute(
            "INSERT INTO discoveries(id,project_id,created_at,payload) VALUES (?1,?2,?3,?4)",
            params![
                discovery.id,
                discovery.project_id,
                discovery.created_at,
                serde_json::to_string(discovery)?
            ],
        )?;
        append_project_event(
            &tx,
            &discovery.project_id,
            "discovery.proposed",
            json!({
                "discovery_id": discovery.id,
                "proposed_manifest_hash": discovery.proposed_manifest.computed_hash()?,
                "manifest_problems": discovery.manifest_problems,
                "questions": discovery.questions.len(),
                "repositories": discovery.repositories.iter().map(|r| json!({
                    "repository_id": r.repository_id,
                    "head_commit": r.observation.head_commit,
                    "profiles": r.profiles,
                    "proposed_checks": r.proposed_checks.iter().map(|c| c.check.id.clone()).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
            }),
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Append a project-scoped fact (for example `project.scaffolded`) to the
    /// durable project event stream. The project must exist.
    pub fn record_project_event(
        &mut self,
        project_id: &str,
        event_type: &str,
        payload: Value,
    ) -> Result<EventEnvelope> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        get_project(&tx, project_id)?.ok_or_else(|| StorageError::NotFound(project_id.into()))?;
        let event = append_project_event(&tx, project_id, event_type, payload)?;
        tx.commit()?;
        Ok(event)
    }

    /// The most recently recorded proposal, by insertion order, not wall clock.
    pub fn latest_discovery(&self, project_id: &str) -> Result<Option<Discovery>> {
        let payload: Option<String> = self
            .connection
            .query_row(
                "SELECT payload FROM discoveries WHERE project_id=?1 ORDER BY rowid DESC LIMIT 1",
                [project_id],
                |r| r.get(0),
            )
            .optional()?;
        payload
            .map(|value| serde_json::from_str(&value).map_err(Into::into))
            .transpose()
    }

    /// Confirm a manifest revision. `observations` are fresh read-only
    /// observations for every registered repository; identity, remote and
    /// default branch must agree with them and the project revision must match.
    pub fn confirm_manifest(
        &mut self,
        project_id: &str,
        expected_revision: u64,
        manifest: &ProjectManifest,
        observations: &BTreeMap<String, RepositoryObservation>,
    ) -> Result<ProjectRecord> {
        self.confirm_manifest_with_remote_branches(
            project_id,
            expected_revision,
            manifest,
            observations,
            &BTreeMap::new(),
        )
    }

    /// [`Store::confirm_manifest`], where `remote_branches` additionally lists,
    /// per repository, default-branch names observed as `origin/<name>`
    /// remote-tracking branches (a fresh clone has only its HEAD branch locally).
    pub fn confirm_manifest_with_remote_branches(
        &mut self,
        project_id: &str,
        expected_revision: u64,
        manifest: &ProjectManifest,
        observations: &BTreeMap<String, RepositoryObservation>,
        remote_branches: &BTreeMap<String, BTreeSet<String>>,
    ) -> Result<ProjectRecord> {
        manifest.validate()?;
        if manifest.id != project_id {
            return Err(StorageError::Invalid(
                "manifest ID must equal the project ID".into(),
            ));
        }
        // Discovery proposes relationships with `confirmed: false`; the
        // developer confirms each one (set `confirmed: true`) or removes it
        // before the manifest is confirmed, so the runtime never schedules
        // against a graph nobody vouched for (ADR 0017).
        let unconfirmed = manifest.unconfirmed_relationships();
        if !unconfirmed.is_empty() {
            let named: Vec<String> = unconfirmed
                .iter()
                .map(|r| format!("{} -> {} ({})", r.producer, r.consumer, r.contract))
                .collect();
            return Err(StorageError::Invalid(format!(
                "manifest lists {} proposed relationship(s) that are not confirmed: {}; set confirmed to true for each relationship you vouch for, or remove it, before confirming the manifest",
                unconfirmed.len(),
                named.join(", ")
            )));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let project = get_project(&tx, project_id)?
            .ok_or_else(|| StorageError::NotFound(project_id.into()))?;
        if project.revision != expected_revision {
            return Err(StorageError::RevisionConflict {
                expected: expected_revision,
                actual: project.revision,
            });
        }
        if manifest.revision != expected_revision + 1 {
            return Err(StorageError::RevisionConflict {
                expected: expected_revision + 1,
                actual: manifest.revision,
            });
        }
        let registered = repositories(&tx, project_id)?;
        let registered_ids: BTreeSet<&str> = registered
            .iter()
            .map(|r| r.repository_id.as_str())
            .collect();
        if registered_ids != manifest.repository_ids() {
            return Err(StorageError::Invalid(
                "manifest repositories must equal the registered repositories".into(),
            ));
        }
        let mut base_commits = BTreeMap::new();
        for registration in &registered {
            let repository = manifest
                .repositories
                .iter()
                .find(|r| r.id == registration.repository_id)
                .expect("ids verified equal");
            let observed = observations
                .get(&registration.repository_id)
                .ok_or_else(|| {
                    StorageError::Invalid(format!(
                        "no fresh observation for repository {}",
                        registration.repository_id
                    ))
                })?;
            registration
                .observation
                .identity
                .matches(&observed.identity)?;
            match &observed.identity.remote {
                Some(remote) if !remotes_equivalent(&repository.remote, remote) => {
                    return Err(StorageError::Invalid(format!(
                        "repository {} remote {} does not match the observed origin",
                        repository.id, repository.remote
                    )));
                }
                None if !repository.remote.starts_with("local:") => {
                    return Err(StorageError::Invalid(format!(
                        "repository {} has no origin remote; its manifest identity must start with local:",
                        repository.id
                    )));
                }
                _ => {}
            }
            let remote_tracking = remote_branches
                .get(&repository.id)
                .is_some_and(|names| names.contains(&repository.default_branch));
            if !observed.has_branch(&repository.default_branch) && !remote_tracking {
                return Err(StorageError::Invalid(format!(
                    "default branch {} is not the current branch, a local branch, origin's HEAD or an origin/<branch> remote-tracking branch of repository {}",
                    repository.default_branch, repository.id
                )));
            }
            base_commits.insert(repository.id.clone(), observed.head_commit.clone());
        }
        let hash = manifest.computed_hash()?;
        let confirmed_at = now();
        tx.execute(
            "INSERT INTO manifest_revisions(project_id,revision,hash,payload,base_commits,confirmed_at) VALUES (?1,?2,?3,?4,?5,?6)",
            params![
                project_id,
                manifest.revision,
                hash,
                serde_json::to_string(manifest)?,
                serde_json::to_string(&base_commits)?,
                confirmed_at
            ],
        )?;
        // The project record follows the confirmed manifest so both names agree.
        tx.execute(
            "UPDATE projects SET revision=?1,manifest_hash=?2,updated_at=?3,name=?4 WHERE id=?5 AND revision=?6",
            params![
                manifest.revision,
                hash,
                confirmed_at,
                manifest.name,
                project_id,
                expected_revision
            ],
        )?;
        append_project_event(
            &tx,
            project_id,
            "manifest.confirmed",
            json!({
                "revision": manifest.revision,
                "manifest_hash": hash,
                "manifest": manifest,
                "base_commits": base_commits,
                "dirty": observations.iter().map(|(id, o)| (id.clone(), json!({
                    "clean": o.dirty.is_clean(),
                    "staged": o.dirty.staged,
                    "unstaged": o.dirty.unstaged,
                    "untracked": o.dirty.untracked,
                    "conflicted": o.dirty.conflicted,
                }))).collect::<BTreeMap<_, _>>(),
            }),
        )?;
        let file_state =
            crate::project_index::write_project_file(&tx, self.data_dir.as_deref(), project_id)?;
        crate::project_index::commit_or_restore(tx, file_state)?;
        self.get_project(project_id)?
            .ok_or_else(|| StorageError::NotFound(project_id.into()))
    }

    pub fn get_manifest(&self, project_id: &str, revision: u64) -> Result<Option<ManifestRecord>> {
        let row = self
            .connection
            .query_row(
                "SELECT hash,payload,base_commits,confirmed_at FROM manifest_revisions WHERE project_id=?1 AND revision=?2",
                params![project_id, revision],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?;
        row.map(|(hash, payload, base_commits, confirmed_at)| {
            Ok(ManifestRecord {
                project_id: project_id.into(),
                revision,
                hash,
                manifest: serde_json::from_str(&payload)?,
                base_commits: serde_json::from_str(&base_commits)?,
                confirmed_at,
            })
        })
        .transpose()
    }

    pub fn current_manifest(&self, project_id: &str) -> Result<Option<ManifestRecord>> {
        match self.get_project(project_id)? {
            Some(project) if project.revision > 0 && project.manifest_hash.is_some() => {
                self.get_manifest(project_id, project.revision)
            }
            _ => Ok(None),
        }
    }

    pub fn project_events_after(
        &self,
        project_id: &str,
        cursor: u64,
    ) -> Result<Vec<EventEnvelope>> {
        self.project_events_page(project_id, cursor, None)
    }

    /// Events after `cursor`, oldest first, at most `limit` of them. A cursor
    /// beyond any SQLite sequence is past every event, so the page is empty.
    pub fn project_events_page(
        &self,
        project_id: &str,
        cursor: u64,
        limit: Option<u64>,
    ) -> Result<Vec<EventEnvelope>> {
        if self.get_project(project_id)?.is_none() {
            return Err(StorageError::NotFound(project_id.into()));
        }
        let cursor = i64::try_from(cursor).unwrap_or(i64::MAX);
        // SQLite treats a negative LIMIT as no limit.
        let limit = limit.map_or(-1, |limit| i64::try_from(limit).unwrap_or(i64::MAX));
        let mut statement = self.connection.prepare(
            "SELECT envelope FROM project_events WHERE project_id=?1 AND sequence>?2 ORDER BY sequence LIMIT ?3",
        )?;
        let rows = statement.query_map(params![project_id, cursor, limit], |row| {
            row.get::<_, String>(0)
        })?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }
}

fn already_registered(local_path: &str, project_id: &str, repository_id: &str) -> String {
    format!(
        "checkout {local_path} is already registered as repository {repository_id} of project {project_id}; use that project, or rebind that repository to another checkout first"
    )
}

fn registration_at(connection: &Connection, local_path: &str) -> Result<Option<(String, String)>> {
    connection
        .query_row(
            "SELECT project_id,id FROM repositories WHERE local_path=?1 ORDER BY project_id,id LIMIT 1",
            [local_path],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(Into::into)
}

fn current_manifest(connection: &Connection, project_id: &str) -> Result<Option<ProjectManifest>> {
    let payload: Option<String> = connection
        .query_row(
            "SELECT m.payload FROM manifest_revisions m JOIN projects p ON p.id=m.project_id AND p.revision=m.revision AND p.manifest_hash=m.hash WHERE p.id=?1",
            [project_id],
            |r| r.get(0),
        )
        .optional()?;
    payload
        .map(|value| serde_json::from_str(&value).map_err(Into::into))
        .transpose()
}

pub(crate) fn get_project_in(connection: &Connection, id: &str) -> Result<Option<ProjectRecord>> {
    get_project(connection, id)
}

fn get_project(connection: &Connection, id: &str) -> Result<Option<ProjectRecord>> {
    connection
        .query_row(
            "SELECT id,name,revision,manifest_hash,created_at,updated_at FROM projects WHERE id=?1",
            [id],
            project_row,
        )
        .optional()
        .map_err(Into::into)
}

fn project_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ProjectRecord> {
    Ok(ProjectRecord {
        id: r.get(0)?,
        name: r.get(1)?,
        revision: r.get(2)?,
        manifest_hash: r.get(3)?,
        created_at: r.get(4)?,
        updated_at: r.get(5)?,
    })
}

pub(crate) fn repositories_in(
    connection: &Connection,
    project_id: &str,
) -> Result<Vec<RepositoryRegistration>> {
    repositories(connection, project_id)
}

fn repositories(connection: &Connection, project_id: &str) -> Result<Vec<RepositoryRegistration>> {
    let mut statement = connection.prepare(
        "SELECT id,local_path,observation,registered_at FROM repositories WHERE project_id=?1 ORDER BY id",
    )?;
    let rows = statement.query_map([project_id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
        ))
    })?;
    rows.map(|row| {
        let (repository_id, local_path, observation, registered_at) = row?;
        Ok(RepositoryRegistration {
            project_id: project_id.into(),
            repository_id,
            local_path,
            observation: serde_json::from_str(&observation)?,
            registered_at,
        })
    })
    .collect()
}

pub(crate) fn append_project_event(
    tx: &Transaction<'_>,
    project_id: &str,
    event_type: &str,
    payload: Value,
) -> Result<EventEnvelope> {
    let sequence: u64 = tx.query_row(
        "SELECT COALESCE(MAX(sequence),0)+1 FROM project_events WHERE project_id=?1",
        [project_id],
        |r| r.get(0),
    )?;
    let stream = project_stream_id(project_id);
    let event = append_envelope(project_id, &stream, sequence, event_type, payload)?;
    tx.execute(
        "INSERT INTO project_events(project_id,sequence,event_id,envelope) VALUES (?1,?2,?3,?4)",
        params![
            project_id,
            sequence,
            event.event_id,
            serde_json::to_string(&event)?
        ],
    )?;
    Ok(event)
}

#[cfg(test)]
mod tests {
    use super::*;
    use luuna_domain::{DirtyState, RepositoryIdentity};

    fn observation(path: &str, remote: &str) -> RepositoryObservation {
        RepositoryObservation {
            local_path: path.into(),
            identity: RepositoryIdentity {
                remote: Some(remote.into()),
                push_remote: None,
                root_commits: vec!["1".repeat(40)],
                shallow: false,
            },
            head_commit: "2".repeat(40),
            branch: Some("main".into()),
            remote_default_branch: None,
            local_branches: vec!["main".into()],
            dirty: DirtyState::default(),
            git_version: "git version 2.43.0".into(),
            observed_at: "2026-10-21T00:00:00.000Z".into(),
        }
    }

    #[test]
    fn a_checkout_has_one_registration_and_rebinds_are_explicit_and_evented() {
        let mut store = Store::open(":memory:").unwrap();
        let remote = "https://example.invalid/app.git";
        store
            .register_project(
                "first",
                "First",
                &[("app".into(), observation("/srv/app", remote))],
            )
            .unwrap();
        assert!(matches!(
            store.register_project(
                "second",
                "Second",
                &[("app".into(), observation("/srv/app", remote))]
            ),
            Err(StorageError::Invalid(detail)) if detail.contains("project first")
        ));
        assert!(store.get_project("second").unwrap().is_none());
        store
            .register_project(
                "second",
                "Second",
                &[("api".into(), observation("/srv/api", remote))],
            )
            .unwrap();
        assert_eq!(
            store.registration_at_path("/srv/app").unwrap(),
            Some(("first".into(), "app".into()))
        );
        // Unchanged path and identity: nothing is recorded.
        assert!(store
            .rebind_repository("first", "app", &observation("/srv/app", remote))
            .unwrap()
            .is_none());
        assert!(matches!(
            store.rebind_repository("first", "app", &observation("/srv/api", remote)),
            Err(StorageError::Invalid(_))
        ));
        assert!(matches!(
            store.rebind_repository("first", "web", &observation("/srv/web", remote)),
            Err(StorageError::NotFound(_))
        ));
        let event = store
            .rebind_repository(
                "first",
                "app",
                &observation("/srv/moved", "git@example.invalid:app.git"),
            )
            .unwrap()
            .expect("a changed binding is recorded");
        assert_eq!(event.event_type, "repository.rebound");
        assert_eq!(event.payload["old"]["local_path"], "/srv/app");
        assert_eq!(event.payload["new"]["local_path"], "/srv/moved");
        assert_eq!(
            event.payload["new"]["identity"]["remote"],
            "git@example.invalid:app.git"
        );
        let registration = &store.repositories("first").unwrap()[0];
        assert_eq!(registration.local_path, "/srv/moved");
        assert_eq!(store.registration_at_path("/srv/app").unwrap(), None);
    }

    #[test]
    fn event_pages_are_bounded_and_active_checkout_runs_are_listed() {
        let mut store = Store::open(":memory:").unwrap();
        store
            .register_project(
                "fixture",
                "Fixture",
                &[(
                    "app".into(),
                    observation("/srv/app", "https://example.invalid/app.git"),
                )],
            )
            .unwrap();
        assert_eq!(store.project_events_after("fixture", 0).unwrap().len(), 2);
        assert!(store
            .project_events_page("fixture", u64::MAX, None)
            .unwrap()
            .is_empty());
        let page = store.project_events_page("fixture", 0, Some(1)).unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].event_type, "project.registered");
        assert!(store.active_checkout_runs("fixture").unwrap().is_empty());
        store
            .create_run("baseline-1", "fixture", json!({"kind":"baseline"}))
            .unwrap();
        store
            .create_workflow_run("workflow-1", "fixture", json!({}))
            .unwrap();
        assert_eq!(
            store.active_checkout_runs("fixture").unwrap(),
            vec!["baseline-1".to_owned()]
        );
    }
}
