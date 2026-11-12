//! Per-project store mode and the readable file that records it.
//!
//! The types hold no path. A project is stored in the runtime data folder by
//! default; the file only exists once a mode has been chosen.

use crate::{valid_project_id, DomainError};
use serde::{Deserialize, Serialize};

/// Version of the project store file written by this build.
pub const PROJECT_STORE_FILE_VERSION: u32 = 1;

/// One of the two places component data can live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreSide {
    Runtime,
    Repository,
}

impl StoreSide {
    pub fn name(&self) -> &'static str {
        match self {
            StoreSide::Runtime => "runtime",
            StoreSide::Repository => "repository",
        }
    }
}

/// Where a project's component data is stored. `Both` has exactly one
/// writable side by construction.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum StoreMode {
    #[default]
    Runtime,
    Repository {
        repository: String,
    },
    Both {
        repository: String,
        writable: StoreSide,
    },
}

impl StoreMode {
    pub fn name(&self) -> &'static str {
        match self {
            StoreMode::Runtime => "runtime",
            StoreMode::Repository { .. } => "repository",
            StoreMode::Both { .. } => "both",
        }
    }

    pub fn writable_side(&self) -> StoreSide {
        match self {
            StoreMode::Runtime => StoreSide::Runtime,
            StoreMode::Repository { .. } => StoreSide::Repository,
            StoreMode::Both { writable, .. } => *writable,
        }
    }

    pub fn repository(&self) -> Option<&str> {
        match self {
            StoreMode::Runtime => None,
            StoreMode::Repository { repository } | StoreMode::Both { repository, .. } => {
                Some(repository)
            }
        }
    }
}

/// The contents of `<data_dir>/projects/<project_id>/store.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectStoreFile {
    pub project_id: String,
    pub mode: StoreMode,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    schema_version: u32,
    project_id: String,
    mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    repository: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    writable: Option<String>,
}

fn invalid<T>(message: &str) -> Result<T, DomainError> {
    Err(DomainError::Invalid(message.to_string()))
}

fn side(value: &str) -> Result<StoreSide, DomainError> {
    match value {
        "runtime" => Ok(StoreSide::Runtime),
        "repository" => Ok(StoreSide::Repository),
        _ => invalid("store file writable side must be `runtime` or `repository`"),
    }
}

impl ProjectStoreFile {
    pub fn parse(bytes: &[u8]) -> Result<ProjectStoreFile, DomainError> {
        let value: serde_json::Value = serde_json::from_slice(bytes)?;
        if let Some(version) = value.get("schema_version").and_then(|v| v.as_u64()) {
            if version > u64::from(PROJECT_STORE_FILE_VERSION) {
                return invalid(
                    "store file was written by a newer Luuna; upgrade Luuna to read it",
                );
            }
        }
        // Serde data errors quote keys from the file; keep messages content-free.
        let raw: Raw = match serde_json::from_value(value) {
            Ok(raw) => raw,
            Err(_) => return invalid("store file has a missing, unknown or mistyped key"),
        };
        if raw.schema_version != PROJECT_STORE_FILE_VERSION {
            return invalid("store file schema_version is not supported");
        }
        if !valid_project_id(&raw.project_id) {
            return invalid("store file project_id is not a valid project id");
        }
        let mode = match raw.mode.as_str() {
            "runtime" => {
                if raw.repository.is_some() || raw.writable.is_some() {
                    return invalid("runtime store mode takes no repository or writable side");
                }
                StoreMode::Runtime
            }
            "repository" => {
                if raw.writable.is_some() {
                    return invalid("repository store mode takes no writable side");
                }
                let Some(repository) = raw.repository else {
                    return invalid("repository store mode needs a repository");
                };
                StoreMode::Repository { repository }
            }
            "both" => {
                let Some(repository) = raw.repository else {
                    return invalid("both store mode needs a repository");
                };
                let Some(writable) = raw.writable else {
                    return invalid("both store mode needs exactly one writable side");
                };
                StoreMode::Both {
                    repository,
                    writable: side(&writable)?,
                }
            }
            _ => return invalid("store mode must be `runtime`, `repository` or `both`"),
        };
        let file = ProjectStoreFile {
            project_id: raw.project_id,
            mode,
        };
        file.validate()?;
        Ok(file)
    }

    pub fn to_json(&self) -> Result<String, DomainError> {
        self.validate()?;
        let (repository, writable) = match &self.mode {
            StoreMode::Runtime => (None, None),
            StoreMode::Repository { repository } => (Some(repository.clone()), None),
            StoreMode::Both {
                repository,
                writable,
            } => (Some(repository.clone()), Some(writable.name().to_string())),
        };
        let raw = Raw {
            schema_version: PROJECT_STORE_FILE_VERSION,
            project_id: self.project_id.clone(),
            mode: self.mode.name().to_string(),
            repository,
            writable,
        };
        let mut text = serde_json::to_string_pretty(&raw)?;
        text.push('\n');
        Ok(text)
    }

    fn validate(&self) -> Result<(), DomainError> {
        if !valid_project_id(&self.project_id) {
            return invalid("store file project_id is not a valid project id");
        }
        if let Some(repository) = self.mode.repository() {
            if !valid_project_id(repository) {
                return invalid("store file repository is not a valid repository id");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(mode: StoreMode) -> ProjectStoreFile {
        ProjectStoreFile {
            project_id: "app".into(),
            mode,
        }
    }

    #[test]
    fn store_mode_defaults_to_runtime() {
        assert_eq!(StoreMode::default(), StoreMode::Runtime);
        assert_eq!(StoreMode::default().name(), "runtime");
        assert_eq!(StoreMode::default().repository(), None);
    }

    #[test]
    fn each_mode_has_exactly_one_writable_side() {
        let repo = || "api".to_string();
        assert_eq!(StoreMode::Runtime.writable_side(), StoreSide::Runtime);
        let only = StoreMode::Repository { repository: repo() };
        assert_eq!(only.writable_side(), StoreSide::Repository);
        assert_eq!(only.repository(), Some("api"));
        for side in [StoreSide::Runtime, StoreSide::Repository] {
            let both = StoreMode::Both {
                repository: repo(),
                writable: side,
            };
            assert_eq!(both.name(), "both");
            assert_eq!(both.writable_side(), side);
        }
        assert_eq!(StoreSide::Runtime.name(), "runtime");
        assert_eq!(StoreSide::Repository.name(), "repository");
    }

    #[test]
    fn store_files_round_trip_and_refuse_every_other_shape() {
        let both = file(StoreMode::Both {
            repository: "api".into(),
            writable: StoreSide::Repository,
        });
        assert_eq!(
            both.to_json().unwrap(),
            "{\n  \"schema_version\": 1,\n  \"project_id\": \"app\",\n  \"mode\": \"both\",\n  \"repository\": \"api\",\n  \"writable\": \"repository\"\n}\n"
        );
        assert_eq!(
            file(StoreMode::Runtime).to_json().unwrap(),
            "{\n  \"schema_version\": 1,\n  \"project_id\": \"app\",\n  \"mode\": \"runtime\"\n}\n"
        );
        for original in [
            file(StoreMode::Runtime),
            file(StoreMode::Repository {
                repository: "api".into(),
            }),
            both,
            file(StoreMode::Both {
                repository: "api".into(),
                writable: StoreSide::Runtime,
            }),
        ] {
            let text = original.to_json().unwrap();
            assert_eq!(ProjectStoreFile::parse(text.as_bytes()).unwrap(), original);
        }

        let head = r#""schema_version":1,"project_id":"app""#;
        let refused = [
            "not json".to_string(),
            "{}".to_string(),
            "[]".to_string(),
            format!("{{{head},\"mode\":\"runtime\",\"extra\":1}}"),
            r#"{"project_id":"app","mode":"runtime"}"#.to_string(),
            r#"{"schema_version":2,"project_id":"app","mode":"runtime"}"#.to_string(),
            r#"{"schema_version":0,"project_id":"app","mode":"runtime"}"#.to_string(),
            r#"{"schema_version":1,"project_id":"other-app!","mode":"runtime"}"#.to_string(),
            format!("{{{head},\"mode\":\"shared\"}}"),
            format!("{{{head},\"mode\":\"repository\"}}"),
            format!("{{{head},\"mode\":\"both\",\"repository\":\"api\"}}"),
            format!("{{{head},\"mode\":\"runtime\",\"writable\":\"runtime\"}}"),
            format!("{{{head},\"mode\":\"runtime\",\"repository\":\"api\"}}"),
            format!("{{{head},\"mode\":\"repository\",\"repository\":\"api\",\"writable\":\"runtime\"}}"),
            format!("{{{head},\"mode\":\"both\",\"repository\":\"api\",\"writable\":\"neither\"}}"),
            format!("{{{head},\"mode\":\"repository\",\"repository\":\"../api\"}}"),
        ];
        for text in &refused {
            assert!(ProjectStoreFile::parse(text.as_bytes()).is_err(), "{text}");
        }
        // A file for another project parses; the caller compares project_id.
        let other = r#"{"schema_version":1,"project_id":"other","mode":"runtime"}"#;
        assert_eq!(
            ProjectStoreFile::parse(other.as_bytes())
                .unwrap()
                .project_id,
            "other"
        );

        let newer = ProjectStoreFile::parse(
            br#"{"schema_version":2,"project_id":"app","mode":"runtime","x":1}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(newer.contains("newer Luuna"));
        let secret = ProjectStoreFile::parse(br#"{"secret-key":1}"#)
            .unwrap_err()
            .to_string();
        assert!(!secret.contains("secret-key"));

        let bad = file(StoreMode::Repository {
            repository: "Bad/id".into(),
        });
        assert!(bad.to_json().is_err());
    }
}
