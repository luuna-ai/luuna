//! Explicit human initialization using the native Roots file format.
use crate::{internal, roots::HumanActor, ApiError, AppState};
use axum::http::StatusCode;
use roots_core::{
    config::{default_config, render_initial_config},
    events::{append_event, make_event, EventFields},
    paths::roots_paths,
};
use std::{
    fs,
    io::{self, Write},
    path::Path,
};

fn conflict() -> ApiError {
    ApiError(
        StatusCode::CONFLICT,
        "Roots initialization requires real directories and regular files".into(),
    )
}
fn link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}
fn inspect(path: &Path, directory: bool) -> Result<bool, ApiError> {
    match fs::symlink_metadata(path) {
        Ok(metadata)
            if link(&metadata)
                || if directory {
                    !metadata.is_dir()
                } else {
                    !metadata.is_file()
                } =>
        {
            Err(conflict())
        }
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(internal(error)),
    }
}
fn directory(path: &Path) -> Result<(), ApiError> {
    if inspect(path, true)? {
        return Ok(());
    }
    let mut builder = fs::DirBuilder::new();
    builder.recursive(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            inspect(path, true).map(|_| ())
        }
        Err(error) => Err(internal(error)),
    }
}
fn create(path: &Path, bytes: &[u8]) -> Result<(), ApiError> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|e| {
        if e.kind() == io::ErrorKind::AlreadyExists {
            conflict()
        } else {
            internal(e)
        }
    })?;
    file.write_all(bytes).map_err(internal)?;
    file.sync_all().map_err(internal)
}
pub(crate) async fn init(state: &AppState, id: &str, human: &HumanActor) -> Result<(), ApiError> {
    let workspace = crate::stores::project_store(state, id)?;
    // A repository store is never set up by the runtime (ADR 0061).
    if workspace.is_checkout() {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "This project's store mode is 'repository'; set up .luuna/roots in the checkout yourself, then retry. No component data was read or written".into(),
        ));
    }
    let workspace = workspace.path().to_owned();
    let _guard = state.component_locks.lock().await;
    let data = state.config.data_dir.clone();
    let id = id.to_owned();
    let by = human.as_str().to_owned();
    tokio::task::spawn_blocking(move || {
        let paths = roots_paths(&workspace);
        let directories = [
            data.join("components"),
            data.join("components/projects"),
            workspace.clone(),
            workspace.join(".luuna"),
            paths.dir.clone(),
            paths.human.clone(),
            paths.agent.clone(),
            paths.sprouts.clone(),
            paths.notes.clone(),
        ];
        // Inspect every existing entry before creating anything. Parent links
        // and existing file links must not redirect the initialization.
        for path in &directories {
            inspect(path, true)?;
        }
        match fs::symlink_metadata(&paths.config) {
            Ok(_) => {
                return Err(ApiError(
                    StatusCode::CONFLICT,
                    "Roots is already set up for this project".into(),
                ))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(internal(e)),
        }
        let files = [
            &paths.graph,
            &paths.proposals,
            &paths.questions,
            &paths.events,
            &paths.gitignore,
        ];
        for path in files {
            inspect(path, false)?;
        }
        for path in directories {
            directory(&path)?;
        }
        for path in [
            &paths.graph,
            &paths.proposals,
            &paths.questions,
            &paths.events,
        ] {
            if !inspect(path, false)? {
                create(path, b"")?;
            }
        }
        if !inspect(&paths.gitignore, false)? {
            create(&paths.gitignore, b"*.lock\n*.tmp.*\n")?;
        }
        if fs::symlink_metadata(&paths.events).map_err(internal)?.len() == 0 {
            let event = make_event(
                "init",
                by,
                EventFields {
                    extra: serde_json::Map::from_iter([(
                        "project".into(),
                        serde_json::Value::String(id.clone()),
                    )]),
                    ..EventFields::default()
                },
            );
            append_event(&paths, &event).map_err(internal)?;
        }
        let config = render_initial_config(&default_config(&id));
        create(&paths.config, config.as_bytes())
    })
    .await
    .map_err(internal)?
}
