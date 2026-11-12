//! `init`: creates `.luuna/missions/` and the merge=union lines in the
//! project's `.gitattributes` (`internal/cli/issue_create.go:19-50`,
//! `internal/config/config.go:115-172`).
//!
//! As in Go, `init` works in the working directory and never discovers a
//! store above it. It accepts only `--json` and the global switches, so
//! `--format` is an unknown option here.
//!
//! An invocation with an explicit store directory is accepted only when
//! that directory is `<cwd>/.luuna/missions`, the one Go's `init` creates;
//! any other is [`Error::Unsupported`](crate::Error::Unsupported), because Go
//! has no way to initialise a store elsewhere and this crate does not guess
//! one.

use std::fs;
use std::io;

use super::output::Envelope;
use super::{options, Context, Failure};
use crate::config::{self, CONFIG_FILE, MISSIONS_DIR};
use crate::error::Error;
use crate::gopath;

/// Go's `runInit`.
pub(super) fn run(context: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    let options = options::parse(args, &[], &["json"])?;
    if !options.positionals.is_empty() {
        return Err(Failure::message("too many arguments for 'init'"));
    }
    let root = context.cwd.clone();
    let dir = root.join(MISSIONS_DIR);
    if let Some(explicit) = context.explicit_store_dir() {
        let explicit =
            gopath::abs(explicit).map_err(|error| Error::io("getwd", explicit, error))?;
        if explicit != dir {
            return Err(Error::unsupported(
                format!(
                    "init with the explicit store directory {} (Go's init creates only {})",
                    explicit.display(),
                    dir.display()
                ),
                "a later plan that defines it (Go has no such option)",
            )
            .into());
        }
    }
    config::refuse_legacy_layout(&root)?;
    let config_path = dir.join(CONFIG_FILE);
    let already = match fs::metadata(&config_path) {
        Ok(_) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(Error::io("stat", config_path, error).into()),
    };
    config::init(&root)?;
    if options.has("json") {
        context.json(Envelope::new(true, "init").string("dir", &gopath::text(&dir)));
    } else if already {
        let mut message = b"Already initialized: ".to_vec();
        message.extend_from_slice(&gopath::bytes(&dir));
        context.success(&message);
    } else {
        let mut message = b"Initialized .luuna/missions/ in ".to_vec();
        message.extend_from_slice(&gopath::bytes(&root));
        context.success(&message);
    }
    Ok(())
}
