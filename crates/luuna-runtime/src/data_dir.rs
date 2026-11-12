//! The one place that turns `LUUNA_DATA_DIR`, or its absence, into the runtime
//! data folder. The service (`ServiceSettings::from_env`) and the admin
//! commands both call it, so they always agree on the folder.
//!
//! The function reads no environment variable and touches no file itself: the
//! variable and the current directory are injected. It does not create,
//! canonicalise or expand the folder.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::StartupError;

/// The environment variable that selects the runtime data folder.
pub const DATA_DIR_VARIABLE: &str = "LUUNA_DATA_DIR";

/// The data folder used when the variable is not set, relative to the current
/// directory.
pub const DEFAULT_DATA_DIR: &str = ".luuna";

/// Resolves the data folder. `var` reads one variable (`std::env::var_os` in
/// the binaries); `current_dir` is called only for a relative value. An
/// absolute value is returned as is; an empty or relative one is joined onto
/// the current directory.
pub fn resolve_data_dir(
    var: impl Fn(&str) -> Option<OsString>,
    current_dir: impl FnOnce() -> std::io::Result<PathBuf>,
) -> Result<PathBuf, StartupError> {
    let value = PathBuf::from(var(DATA_DIR_VARIABLE).unwrap_or_else(|| DEFAULT_DATA_DIR.into()));
    if value.is_absolute() {
        return Ok(value);
    }
    let base = current_dir().map_err(|error| {
        StartupError(format!(
            "{DATA_DIR_VARIABLE} {} is relative and the current directory cannot be read: {error}",
            value.display()
        ))
    })?;
    Ok(base.join(value))
}
