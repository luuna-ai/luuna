//! The handle on one project root, and `init` (`cli.go:144-194`).
//!
//! The library takes the project root as an absolute path and never reads the
//! process's working directory or environment (assumption AS1). It prints
//! nothing (AS9): where Go prints a line, a method returns a value.

use crate::error::BeaconError;
use crate::{config, gofmt, gotime, paths, Clock, SystemClock};
use std::fmt;
use std::fs;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

/// The three record files `init` creates, in Go's order.
const RECORD_FILES: [&str; 3] = ["prompts.jsonl", "schemas.jsonl", "artifacts.jsonl"];

/// The last nanosecond count handed out as an id, so that ids are strictly
/// increasing within one process even when the clock is coarse or fixed.
static LAST_ID_NANOS: AtomicI64 = AtomicI64::new(i64::MIN);

/// What `init` created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Init {
    /// `<root>/.luuna/beacon`.
    pub dir: PathBuf,
}

/// A handle on one project root. Cheap to clone; holds no open files.
#[derive(Clone)]
pub struct Beacon {
    root: PathBuf,
    clock: Arc<dyn Clock>,
}

impl fmt::Debug for Beacon {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Beacon").field("root", &self.root).finish()
    }
}

impl Beacon {
    /// A handle on `project_root`, which must be an absolute path. Touches no
    /// file.
    pub fn open(project_root: &Path) -> Result<Beacon, BeaconError> {
        if !project_root.is_absolute() {
            return Err(BeaconError::RelativeRoot(project_root.to_path_buf()));
        }
        Ok(Beacon {
            root: project_root.to_path_buf(),
            clock: Arc::new(SystemClock),
        })
    }

    /// The same handle reading the time from `clock`.
    #[must_use]
    pub fn with_clock(self, clock: Arc<dyn Clock>) -> Beacon {
        Beacon {
            root: self.root,
            clock,
        }
    }

    /// The project root given to [`Beacon::open`].
    pub fn project_root(&self) -> &Path {
        &self.root
    }

    /// `<root>/.luuna/beacon`. Pure: looks at no file.
    pub fn store_dir(&self) -> PathBuf {
        paths::store_dir(&self.root)
    }

    /// Moves a legacy `<root>/.canopy` store to `.luuna/beacon` (the move of
    /// `paths.go:34-75`). Returns `true` where Go prints
    /// `Migrated .canopy to .luuna/beacon`. Every other method runs it first.
    pub fn migrate_legacy(&self) -> Result<bool, BeaconError> {
        paths::migrate_legacy(&self.root)
    }

    /// Go's `initProject`: creates `.luuna/beacon` with the default
    /// `config.yaml`, a `.gitignore` of `*.lock`, three empty record files, and
    /// the `merge=union` lines in `<root>/.gitattributes`.
    pub fn init(&self) -> Result<Init, BeaconError> {
        self.migrate_legacy()?;
        let dir = self.store_dir();
        if fs::metadata(&dir).is_ok() {
            return Err(BeaconError::AlreadyInitialized);
        }
        paths::make_dirs(&dir)?;
        paths::write_file(
            &dir.join("config.yaml"),
            config::DEFAULT_CONFIG_YAML.as_bytes(),
        )?;
        paths::write_file(&dir.join(".gitignore"), b"*.lock\n")?;
        for name in RECORD_FILES {
            paths::write_file(&dir.join(name), b"")?;
        }

        let attributes = self.root.join(".gitattributes");
        let existing = read_if_present(&attributes)?;
        let mut additions: Vec<u8> = Vec::new();
        for record in RECORD_FILES {
            let entry = format!(".luuna/beacon/{record} merge=union");
            let mut seen = existing.clone();
            seen.extend_from_slice(&additions);
            if !contains_bytes(&seen, entry.as_bytes()) {
                additions.extend_from_slice(entry.as_bytes());
                additions.push(b'\n');
            }
        }
        if !additions.is_empty() {
            let mut content = existing;
            if !content.is_empty() && !content.ends_with(b"\n") {
                content.push(b'\n');
            }
            content.extend_from_slice(&additions);
            paths::write_file(&attributes, &content)?;
        }
        Ok(Init { dir })
    }

    /// The time on this handle's clock as Go's `RFC3339Nano` text.
    pub(crate) fn now_text(&self) -> String {
        gotime::rfc3339_nano(self.clock.now())
    }

    /// The hexadecimal tail of a new id: Go's `%x` of `UnixNano()`, forced to
    /// be greater than any value this process handed out before.
    pub(crate) fn next_id_hex(&self) -> String {
        let nanos = gotime::unix_nano(self.clock.now());
        let previous = LAST_ID_NANOS
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |last| {
                Some(nanos.max(last.saturating_add(1)))
            })
            .unwrap_or(i64::MIN);
        gofmt::hex_i64(nanos.max(previous.saturating_add(1)))
    }

    /// `paths.Prompts`: runs the legacy move, then names the prompts file.
    pub(crate) fn prompts_path(&self) -> Result<PathBuf, BeaconError> {
        self.migrate_legacy()?;
        Ok(paths::prompts_file(&self.root))
    }
}

/// `os.ReadFile` where a missing file is empty and any other failure is an
/// error.
fn read_if_present(path: &Path) -> Result<Vec<u8>, BeaconError> {
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(BeaconError::os("open", path, error)),
    };
    let mut content = Vec::new();
    file.read_to_end(&mut content)
        .map_err(|error| BeaconError::os("read", path, error))?;
    Ok(content)
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_refuses_a_relative_root_and_touches_nothing() {
        assert!(matches!(
            Beacon::open(Path::new("relative/path")),
            Err(BeaconError::RelativeRoot(_))
        ));
        let beacon = Beacon::open(Path::new("/nonexistent-root-for-test")).unwrap();
        assert_eq!(
            beacon.store_dir(),
            Path::new("/nonexistent-root-for-test/.luuna/beacon")
        );
    }

    #[test]
    fn ids_strictly_increase_with_a_fixed_clock() {
        struct Fixed;
        impl Clock for Fixed {
            fn now(&self) -> std::time::SystemTime {
                std::time::UNIX_EPOCH + std::time::Duration::from_secs(5)
            }
        }
        let beacon = Beacon::open(Path::new("/x"))
            .unwrap()
            .with_clock(Arc::new(Fixed));
        let a = i64::from_str_radix(&beacon.next_id_hex(), 16).unwrap();
        let b = i64::from_str_radix(&beacon.next_id_hex(), 16).unwrap();
        assert!(b > a);
    }

    #[test]
    fn init_writes_go_layout_and_appends_attribute_lines() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        fs::write(root.join(".gitattributes"), "*.png binary").unwrap();
        let beacon = Beacon::open(&root).unwrap();
        let init = beacon.init().unwrap();
        assert_eq!(init.dir, root.join(".luuna/beacon"));
        assert_eq!(
            fs::read_to_string(init.dir.join(".gitignore")).unwrap(),
            "*.lock\n"
        );
        for name in RECORD_FILES {
            assert!(fs::read(init.dir.join(name)).unwrap().is_empty());
        }
        assert_eq!(
            fs::read_to_string(root.join(".gitattributes")).unwrap(),
            "*.png binary\n.luuna/beacon/prompts.jsonl merge=union\n\
             .luuna/beacon/schemas.jsonl merge=union\n\
             .luuna/beacon/artifacts.jsonl merge=union\n"
        );
        assert!(matches!(
            beacon.init(),
            Err(BeaconError::AlreadyInitialized)
        ));
    }
}
