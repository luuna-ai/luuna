//! Emission resolves all candidates before checking the store overwrite boundary.
use super::{gopath, render_markdown, AuthoringError as E, ProjectConfig};
use crate::{paths, prompts, resolve, store, Beacon, BeaconError, Prompt};
use std::{
    fs,
    path::{Path, PathBuf},
};
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmitSelection {
    All,
    Prompt(String),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmitOptions {
    pub selection: EmitSelection,
    pub check: bool,
    pub dry_run: bool,
    pub force: bool,
    pub out: Option<String>,
    pub out_dir: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmittedFile {
    pub name: String,
    pub path: String,
    pub version: i64,
    pub skipped: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmitReport {
    Files(Vec<EmittedFile>),
    Check { stale: Vec<String> },
}
impl EmitReport {
    pub fn failure(&self) -> Option<BeaconError> {
        match self {
            Self::Check { stale } if !stale.is_empty() => Some(E::StaleFiles(stale.len()).into()),
            _ => None,
        }
    }
}
fn directory<'a>(prompt: &'a Prompt, config: &'a ProjectConfig) -> &'a str {
    if !prompt.emit_dir.is_empty() {
        return &prompt.emit_dir;
    }
    for target in config.targets.as_deref().unwrap_or(&[]) {
        if prompt.tags.iter().any(|tag| target.tags.contains(tag)) {
            return &target.dir;
        }
    }
    for target in config.targets.as_deref().unwrap_or(&[]) {
        if target.default {
            return &target.dir;
        }
    }
    "agents"
}
impl Beacon {
    pub fn emit(&self, options: &EmitOptions) -> Result<EmitReport, BeaconError> {
        let records: Vec<Prompt> = store::read_jsonl(&self.prompts_path()?)?;
        let records = prompts::current(records);
        let selected = if options.check || matches!(options.selection, EmitSelection::All) {
            records
                .iter()
                .filter(|p| p.status == "active")
                .cloned()
                .collect::<Vec<_>>()
        } else {
            let EmitSelection::Prompt(name) = &options.selection else {
                unreachable!()
            };
            if name.is_empty() {
                return Err(BeaconError::PromptNameOrAllRequired);
            }
            vec![records[prompts::find(&records, name)?].clone()]
        };
        let config = self.config_targets()?;
        let mut ready = Vec::new();
        for prompt in selected {
            let resolved = resolve::resolve(records.clone(), &prompt.name, prompt.pinned)?.prompt;
            let filename = if prompt.emit_as.is_empty() {
                format!("{}.md", prompt.name)
            } else {
                prompt.emit_as.clone()
            };
            let report_path = options
                .out
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    let dir = options
                        .out_dir
                        .as_deref()
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| directory(&prompt, &config));
                    gopath::join(&[&self.project_root().to_string_lossy(), dir, &filename])
                });
            let actual = if Path::new(&report_path).is_absolute() {
                PathBuf::from(&report_path)
            } else {
                self.project_root().join(&report_path)
            };
            ready.push((
                EmittedFile {
                    name: prompt.name,
                    path: report_path,
                    version: resolved.version,
                    skipped: false,
                },
                actual,
                render_markdown(&resolved),
            ))
        }
        if options.check {
            let stale = ready
                .iter()
                .filter(|(_, path, text)| {
                    fs::read(path).map_or(true, |bytes| bytes != text.as_bytes())
                })
                .map(|(file, _, _)| file.name.clone())
                .collect();
            return Ok(EmitReport::Check { stale });
        }
        if options.dry_run {
            return Ok(EmitReport::Files(
                ready.into_iter().map(|(file, _, _)| file).collect(),
            ));
        }
        let boundary = gopath::canonical_future(&self.project_root().join(".luuna"))
            .map_err(|e| BeaconError::os("stat", self.project_root().join(".luuna"), e))?;
        for (file, path, text) in &ready {
            if fs::metadata(path).is_ok()
                && fs::read(path).map_or(true, |bytes| bytes != text.as_bytes())
            {
                let canonical =
                    gopath::canonical_future(path).map_err(|e| BeaconError::os("stat", path, e))?;
                if canonical.starts_with(&boundary) {
                    return Err(E::EmitIntoStore(file.path.clone()).into());
                }
            }
        }
        let mut files = Vec::new();
        for (mut file, path, text) in ready {
            if let Some(parent) = path.parent() {
                paths::make_dirs(parent)?
            }
            if !options.force && fs::read(&path).is_ok_and(|bytes| bytes == text.as_bytes()) {
                file.skipped = true
            } else {
                paths::write_file(&path, text.as_bytes())?
            }
            files.push(file)
        }
        Ok(EmitReport::Files(files))
    }
}
