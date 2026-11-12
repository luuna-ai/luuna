//! The `.luuna/missions/` store: JSONL files, atomic replace and lock files
//! compatible with the Go tool's.
//!
//! The location is a parameter: [`Store::open`] takes the store directory
//! itself, and [`Store::discover`] walks up from a working directory to the
//! first `.luuna/missions/config.yaml`, as Go's `FindDir` does.
//!
//! The format layer is ported in task t2 and discovery in task t3.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result, THIS_PLAN};
use crate::model::Ticket;

/// A data file of the store, each guarded by its own `<file>.lock`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DataFile {
    /// `issues.jsonl`
    Issues,
    /// `plans.jsonl`
    Plans,
    /// `templates.jsonl`
    Templates,
    /// `config.yaml`
    Config,
}

impl DataFile {
    /// The file name inside the store directory.
    pub fn file_name(self) -> &'static str {
        match self {
            Self::Issues => "issues.jsonl",
            Self::Plans => "plans.jsonl",
            Self::Templates => "templates.jsonl",
            Self::Config => "config.yaml",
        }
    }
}

/// A `.luuna/missions/` store directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    /// The store whose directory is `dir` (the `.luuna/missions` directory
    /// itself).
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self> {
        let _ = dir.into();
        Err(Error::unsupported("Store::open", THIS_PLAN))
    }

    /// The store found by walking up from `cwd`, as Go's `FindDir` does.
    pub fn discover(cwd: &Path) -> Result<Self> {
        let _ = cwd;
        Err(Error::unsupported("Store::discover", THIS_PLAN))
    }

    /// The store directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The tickets in `issues.jsonl`, as Go reads them.
    pub fn read_tickets(&self) -> Result<Vec<Ticket>> {
        Err(Error::unsupported("Store::read_tickets", THIS_PLAN))
    }

    /// Takes the lock of `file` with Go's protocol: `<file>.lock` created
    /// exclusively, waiting up to 30 s and reclaiming a lock older than 30 s.
    /// The lock is released when the returned guard is dropped.
    pub fn lock(&self, file: DataFile) -> Result<StoreLock> {
        let _ = file;
        Err(Error::unsupported("Store::lock", THIS_PLAN))
    }
}

/// A held store lock; dropping it removes the lock file.
#[derive(Debug)]
pub struct StoreLock {
    path: PathBuf,
}

impl StoreLock {
    /// The lock file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_files_keep_the_go_names() {
        assert_eq!(DataFile::Issues.file_name(), "issues.jsonl");
        assert_eq!(DataFile::Plans.file_name(), "plans.jsonl");
        assert_eq!(DataFile::Templates.file_name(), "templates.jsonl");
        assert_eq!(DataFile::Config.file_name(), "config.yaml");
    }
}
