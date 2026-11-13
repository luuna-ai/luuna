//! `doctor` (`cli.go:1400-1496`): six checks of the store, in Go's order,
//! and `--fix` for stale lock files.

use super::model::Artifact;
use super::snapshot;
use crate::config;
use crate::model::{Prompt, Schema};
use crate::paths;
use crate::prompts;
use crate::resolve;
use crate::store;
use crate::{Beacon, BeaconError};
use std::ffi::OsString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoctorStatus {
    Pass,
    Warn,
    Fail,
}

impl DoctorStatus {
    /// The word Go prints.
    pub fn as_str(self) -> &'static str {
        match self {
            DoctorStatus::Pass => "pass",
            DoctorStatus::Warn => "warn",
            DoctorStatus::Fail => "fail",
        }
    }
}

/// One check. `details` is `None` where Go holds a nil slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorCheck {
    pub name: String,
    pub status: DoctorStatus,
    pub message: String,
    pub details: Option<Vec<String>>,
    pub fixable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DoctorSummary {
    pub pass: usize,
    pub warn: usize,
    pub fail: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorReport {
    pub checks: Vec<DoctorCheck>,
}

impl DoctorReport {
    pub fn summary(&self) -> DoctorSummary {
        let count = |status| self.checks.iter().filter(|c| c.status == status).count();
        let (warn, fail) = (count(DoctorStatus::Warn), count(DoctorStatus::Fail));
        DoctorSummary {
            pass: self.checks.len() - warn - fail,
            warn,
            fail,
        }
    }

    /// True when no check failed (a warning is not a failure).
    pub fn success(&self) -> bool {
        self.summary().fail == 0
    }
}

fn check(
    name: &str,
    status: DoctorStatus,
    message: impl Into<String>,
    details: Option<Vec<String>>,
    fixable: bool,
) -> DoctorCheck {
    DoctorCheck {
        name: name.to_owned(),
        status,
        message: message.into(),
        details,
        fixable,
    }
}

/// A lock file older than this is stale.
const STALE_AFTER: Duration = Duration::from_secs(30);

impl Beacon {
    /// Go's `doctor`. The only error is the refused legacy move; failed
    /// checks are in the report (the caller turns `!success()` into
    /// [`super::ArtifactsError::DoctorFailed`]).
    pub fn doctor(&self, fix: bool) -> Result<DoctorReport, BeaconError> {
        self.migrate_legacy()?;
        let root = self.project_root();
        let dir = self.store_dir();
        let mut checks = Vec::new();

        match config::load(root) {
            Err(error) => checks.push(check(
                "config",
                DoctorStatus::Fail,
                "Config is invalid",
                Some(vec![error.to_string()]),
                false,
            )),
            Ok(_) => checks.push(check(
                "config",
                DoctorStatus::Pass,
                "Config is valid",
                None,
                false,
            )),
        }

        let records = store::read_jsonl::<Prompt>(&paths::prompts_file(root));
        match &records {
            Err(error) => checks.push(check(
                "prompts-integrity",
                DoctorStatus::Fail,
                "Prompt store is invalid",
                Some(vec![error.to_string()]),
                false,
            )),
            Ok(records) => checks.push(check(
                "prompts-integrity",
                DoctorStatus::Pass,
                format!("{} records, all valid", records.len()),
                None,
                false,
            )),
        }

        match store::read_jsonl::<Schema>(&paths::schemas_file(root)) {
            Err(error) => checks.push(check(
                "schemas-integrity",
                DoctorStatus::Fail,
                "Schema store is invalid",
                Some(vec![error.to_string()]),
                false,
            )),
            Ok(schemas) => checks.push(check(
                "schemas-integrity",
                DoctorStatus::Pass,
                format!("{} records, all valid", schemas.len()),
                None,
                false,
            )),
        }

        match store::read_jsonl::<Artifact>(&paths::artifacts_file(root)) {
            Err(error) => checks.push(check(
                "artifacts-integrity",
                DoctorStatus::Fail,
                "Artifact store is invalid",
                Some(vec![error.to_string()]),
                false,
            )),
            Ok(artifacts) => {
                let details: Vec<String> = artifacts
                    .iter()
                    .filter_map(|record| snapshot::verify(record).err())
                    .map(|error| error.to_string())
                    .collect();
                if details.is_empty() {
                    checks.push(check(
                        "artifacts-integrity",
                        DoctorStatus::Pass,
                        format!("{} stored artifact version(s) verified", artifacts.len()),
                        None,
                        false,
                    ));
                } else {
                    checks.push(check(
                        "artifacts-integrity",
                        DoctorStatus::Fail,
                        "Artifact store is invalid",
                        Some(details),
                        false,
                    ));
                }
            }
        }

        if let Ok(records) = records {
            let current = prompts::current(records);
            let details: Vec<String> = current
                .iter()
                .filter(|prompt| !prompt.extends.is_empty() || !prompt.mixins.is_empty())
                .filter_map(|prompt| {
                    resolve::resolve(current.clone(), &prompt.name, None)
                        .err()
                        .map(|error| format!("{}: {error}", prompt.name))
                })
                .collect();
            if details.is_empty() {
                checks.push(check(
                    "inheritance",
                    DoctorStatus::Pass,
                    "No broken references",
                    None,
                    false,
                ));
            } else {
                checks.push(check(
                    "inheritance",
                    DoctorStatus::Fail,
                    "Broken inheritance references",
                    Some(details),
                    false,
                ));
            }
        }

        let mut stale: Vec<OsString> = Vec::new();
        if let Ok(entries) = fs::read_dir(&dir) {
            let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
            entries.sort_by_key(|entry| entry.file_name());
            let now = SystemTime::now();
            for entry in entries {
                let name = entry.file_name();
                if !name.as_bytes().ends_with(b".lock") {
                    continue;
                }
                // `DirEntry::metadata` does not follow a symbolic link, as
                // Go's `entry.Info()` does not.
                let modified = entry.metadata().and_then(|info| info.modified());
                if let Ok(modified) = modified {
                    if now
                        .duration_since(modified)
                        .is_ok_and(|age| age > STALE_AFTER)
                    {
                        stale.push(name);
                    }
                }
            }
        }
        if fix {
            for name in stale.drain(..) {
                let _ = fs::remove_file(dir.join(name));
            }
        }
        if stale.is_empty() {
            checks.push(check(
                "stale-locks",
                DoctorStatus::Pass,
                "No stale locks",
                None,
                false,
            ));
        } else {
            checks.push(check(
                "stale-locks",
                DoctorStatus::Warn,
                format!("{} stale lock file(s)", stale.len()),
                Some(
                    stale
                        .iter()
                        .map(|name| name.to_string_lossy().into_owned())
                        .collect(),
                ),
                true,
            ));
        }
        Ok(DoctorReport { checks })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(report: &DoctorReport) -> Vec<&str> {
        report.checks.iter().map(|c| c.name.as_str()).collect()
    }

    #[test]
    fn six_checks_in_order_and_inheritance_skipped_on_bad_prompts() {
        let dir = tempfile::tempdir().unwrap();
        let beacon = Beacon::open(dir.path()).unwrap();
        beacon.init().unwrap();
        let report = beacon.doctor(false).unwrap();
        assert_eq!(
            names(&report),
            [
                "config",
                "prompts-integrity",
                "schemas-integrity",
                "artifacts-integrity",
                "inheritance",
                "stale-locks"
            ]
        );
        assert!(report.success());
        assert_eq!(report.checks[1].message, "0 records, all valid");
        assert_eq!(
            report.checks[3].message,
            "0 stored artifact version(s) verified"
        );

        fs::write(beacon.store_dir().join("prompts.jsonl"), b"{bad\n").unwrap();
        let report = beacon.doctor(false).unwrap();
        assert_eq!(report.checks.len(), 5);
        assert_eq!(report.checks[1].status, DoctorStatus::Fail);
        assert!(!report.success());
        assert_eq!(report.summary().fail, 1);
    }

    #[test]
    fn stale_locks_warn_and_fix_removes_them() {
        let dir = tempfile::tempdir().unwrap();
        let beacon = Beacon::open(dir.path()).unwrap();
        beacon.init().unwrap();
        let old = beacon.store_dir().join("b.lock");
        let older = beacon.store_dir().join("a.lock");
        let fresh = beacon.store_dir().join("c.lock");
        for path in [&old, &older, &fresh] {
            fs::write(path, b"").unwrap();
        }
        for path in [&old, &older] {
            fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(SystemTime::now() - Duration::from_secs(60))
                .unwrap();
        }
        let report = beacon.doctor(false).unwrap();
        let stale = report.checks.last().unwrap();
        assert_eq!(stale.status, DoctorStatus::Warn);
        assert_eq!(stale.message, "2 stale lock file(s)");
        assert_eq!(
            stale.details.as_deref(),
            Some(&["a.lock".to_owned(), "b.lock".to_owned()][..])
        );
        assert!(stale.fixable);
        let report = beacon.doctor(true).unwrap();
        assert_eq!(report.checks.last().unwrap().status, DoctorStatus::Pass);
        assert!(!old.exists() && !older.exists() && fresh.exists());
    }
}
