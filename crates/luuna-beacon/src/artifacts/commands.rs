//! The artifact commands of `artifactCommand` (`cli.go:1016-1204`) other
//! than export: import, update, list, verify, show, history and read.
//!
//! Every method runs the store prologue first (legacy move, read, verify
//! every record, refuse conflicting versions) and reports Go's argument
//! errors after it (AS51).

use super::model::{Artifact, ArtifactFile};
use super::snapshot;
use super::{ArtifactKind, ArtifactsError};
use crate::gofmt;
use crate::paths;
use crate::store;
use crate::{Beacon, BeaconError};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The versions of one artifact, oldest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactHistory {
    pub name: String,
    pub versions: Vec<Artifact>,
}

/// One stored file of an artifact version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactRead {
    pub name: String,
    pub version: i64,
    pub file: ArtifactFile,
}

/// `selectRecord`: the highest version of `reference` (`name` or
/// `name@version`) among the records of `kind`.
fn select_record(
    records: &[Artifact],
    kind: ArtifactKind,
    reference: &str,
) -> Result<Artifact, ArtifactsError> {
    let (name, wanted) = match reference.split_once('@') {
        Some((name, raw)) => (name, Some(raw)),
        None => (reference, None),
    };
    let mut selected: Option<&Artifact> = None;
    for candidate in records {
        if candidate.kind != kind.as_str() || candidate.name != name {
            continue;
        }
        if let Some(raw) = wanted {
            match gofmt::sscan_int(raw) {
                Some(version) if version == candidate.version => {}
                _ => continue,
            }
        }
        if selected.is_none_or(|current| candidate.version > current.version) {
            selected = Some(candidate);
        }
    }
    selected.cloned().ok_or_else(|| ArtifactsError::NotFound {
        kind: kind.as_str().to_owned(),
        reference: reference.to_owned(),
    })
}

impl Beacon {
    /// The prologue, `cli.go:1020-1038`: the legacy move, the records read,
    /// every record verified, no two records of one version with different
    /// digests. Returns the file and its records.
    fn artifact_records(&self) -> Result<(PathBuf, Vec<Artifact>), BeaconError> {
        self.migrate_legacy()?;
        let path = paths::artifacts_file(self.project_root());
        let records = store::read_jsonl::<Artifact>(&path)?;
        let mut seen: HashMap<(&str, &str, i64), &str> = HashMap::new();
        for record in &records {
            if let Err(cause) = snapshot::verify(record) {
                return Err(ArtifactsError::InvalidRecord {
                    kind: record.kind.clone(),
                    name: record.name.clone(),
                    version: record.version,
                    cause: cause.to_string(),
                }
                .into());
            }
            let key = (record.kind.as_str(), record.name.as_str(), record.version);
            if let Some(previous) = seen.get(&key) {
                if *previous != record.sha256 {
                    return Err(ArtifactsError::ConflictingVersion {
                        kind: record.kind.clone(),
                        name: record.name.clone(),
                        version: record.version,
                    }
                    .into());
                }
            }
            seen.insert(key, record.sha256.as_str());
        }
        Ok((path, records))
    }

    /// The prologue alone, for a subcommand the caller does not know.
    pub fn artifact_store_check(&self) -> Result<(), BeaconError> {
        self.artifact_records().map(|_| ())
    }

    /// The shared tail of import and update: the next version, the stored
    /// record when the manifest digest is unchanged, else a new record
    /// appended with the first id kept.
    fn artifact_store_version(
        &self,
        path: &Path,
        records: &[Artifact],
        kind: ArtifactKind,
        name: &str,
        source: &Path,
    ) -> Result<Artifact, BeaconError> {
        let files = snapshot::snapshot(kind.as_str(), name, self.project_root(), source)?;
        let mut version: i64 = 1;
        let mut previous: Option<&Artifact> = None;
        for record in records {
            if record.kind == kind.as_str() && record.name == name && record.version >= version {
                version = record.version.wrapping_add(1);
                previous = Some(record);
            }
        }
        let mut next = snapshot::new_artifact(self, kind.as_str(), name, files, version);
        if let Some(previous) = previous {
            if previous.sha256 == next.sha256 {
                return Ok(previous.clone());
            }
            next.id = previous.id.clone();
        }
        store::append_jsonl(path, &next)?;
        Ok(next)
    }

    /// `<kind> import <source> --name <name>`.
    pub fn artifact_import(
        &self,
        kind: ArtifactKind,
        source: Option<&Path>,
        name: &str,
    ) -> Result<Artifact, BeaconError> {
        let (path, records) = self.artifact_records()?;
        let Some(source) = source else {
            return Err(ArtifactsError::SourcePathRequired.into());
        };
        if name.is_empty() {
            return Err(ArtifactsError::NameRequired.into());
        }
        self.artifact_store_version(&path, &records, kind, name, source)
    }

    /// `<kind> update <name> <source>`.
    pub fn artifact_update(
        &self,
        kind: ArtifactKind,
        name: Option<&str>,
        source: Option<&Path>,
    ) -> Result<Artifact, BeaconError> {
        let (path, records) = self.artifact_records()?;
        let (Some(name), Some(source)) = (name, source) else {
            return Err(ArtifactsError::NameAndSourceRequired.into());
        };
        self.artifact_store_version(&path, &records, kind, name, source)
    }

    /// `<kind> list`: the latest version of each name, sorted by name.
    pub fn artifact_list(&self, kind: ArtifactKind) -> Result<Vec<Artifact>, BeaconError> {
        let (_, records) = self.artifact_records()?;
        let mut latest: HashMap<&str, &Artifact> = HashMap::new();
        for record in &records {
            if record.kind != kind.as_str() {
                continue;
            }
            match latest.get(record.name.as_str()) {
                Some(prior) if record.version <= prior.version => {}
                _ => {
                    latest.insert(record.name.as_str(), record);
                }
            }
        }
        let mut items: Vec<Artifact> = latest.into_values().cloned().collect();
        items.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(items)
    }

    /// `<kind> verify [reference]`: how many versions were checked.
    pub fn artifact_verify(
        &self,
        kind: ArtifactKind,
        reference: Option<&str>,
    ) -> Result<usize, BeaconError> {
        let (_, records) = self.artifact_records()?;
        let selected = match reference {
            Some(reference) => vec![select_record(&records, kind, reference)?],
            None => records
                .into_iter()
                .filter(|record| record.kind == kind.as_str())
                .collect(),
        };
        for record in &selected {
            snapshot::verify(record)?;
        }
        Ok(selected.len())
    }

    /// The prologue, then the argument check and the selection shared by
    /// show, history, read and export.
    pub(crate) fn artifact_select(
        &self,
        kind: ArtifactKind,
        reference: Option<&str>,
    ) -> Result<(Vec<Artifact>, Artifact), BeaconError> {
        let (_, records) = self.artifact_records()?;
        let Some(reference) = reference else {
            return Err(ArtifactsError::ArtifactNameRequired.into());
        };
        let record = select_record(&records, kind, reference)?;
        Ok((records, record))
    }

    /// `<kind> show <reference>`.
    pub fn artifact_show(
        &self,
        kind: ArtifactKind,
        reference: Option<&str>,
    ) -> Result<Artifact, BeaconError> {
        self.artifact_select(kind, reference)
            .map(|(_, record)| record)
    }

    /// `<kind> history <reference>`: every version of the name, oldest first.
    pub fn artifact_history(
        &self,
        kind: ArtifactKind,
        reference: Option<&str>,
    ) -> Result<ArtifactHistory, BeaconError> {
        let (records, record) = self.artifact_select(kind, reference)?;
        let mut versions: Vec<Artifact> = records
            .into_iter()
            .filter(|candidate| candidate.kind == kind.as_str() && candidate.name == record.name)
            .collect();
        versions.sort_by_key(|candidate| candidate.version);
        Ok(ArtifactHistory {
            name: record.name,
            versions,
        })
    }

    /// `<kind> read <reference> <file>`.
    pub fn artifact_read(
        &self,
        kind: ArtifactKind,
        reference: Option<&str>,
        file: Option<&str>,
    ) -> Result<ArtifactRead, BeaconError> {
        let (_, record) = self.artifact_select(kind, reference)?;
        let Some(wanted) = file else {
            return Err(ArtifactsError::FilePathRequired.into());
        };
        match record.files.iter().find(|file| file.path == wanted) {
            Some(file) => Ok(ArtifactRead {
                name: record.name.clone(),
                version: record.version,
                file: file.clone(),
            }),
            None => Err(ArtifactsError::FileNotFound(wanted.to_owned()).into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(name: &str, version: i64) -> Artifact {
        Artifact {
            kind: "skill".into(),
            name: name.into(),
            version,
            ..Artifact::default()
        }
    }

    #[test]
    fn select_takes_the_highest_version_or_the_named_one() {
        let records = vec![
            record("a", 1),
            record("a", 3),
            record("a", 2),
            record("b", 9),
        ];
        let pick = |reference| select_record(&records, ArtifactKind::Skill, reference);
        assert_eq!(pick("a").unwrap().version, 3);
        assert_eq!(pick("a@2").unwrap().version, 2);
        // Sscanf reads the leading integer and ignores the rest.
        assert_eq!(pick("a@2x").unwrap().version, 2);
        assert_eq!(pick("a@ 1").unwrap().version, 1);
        assert_eq!(
            pick("a@4").unwrap_err().to_string(),
            "skill \"a@4\" not found"
        );
        assert!(pick("a@x").is_err());
        assert!(pick("a@").is_err());
        assert!(select_record(&records, ArtifactKind::Agent, "a").is_err());
        assert_eq!(pick("b").unwrap().version, 9);
    }

    #[test]
    fn next_version_wraps_instead_of_panicking() {
        assert_eq!(i64::MAX.wrapping_add(1), i64::MIN);
    }
}
