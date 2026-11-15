use super::*;
pub(super) fn dispatch(c: &mut Context<'_>, command: &str, a: &[String]) -> Result<()> {
    match command {
        "prime" => {
            let p = c.beacon.prime(&PrimeOptions {
                compact: has(a, "--compact"),
                export: has(a, "--export"),
            });
            if c.json {
                c.respond(command, [("content", quote(&p.go_json_text()))], "");
            } else if !c.quiet {
                c.out.stdout.extend_from_slice(&p.content);
            }
        }
        "onboard" => {
            let mode = if has(a, "--stdout") {
                OnboardMode::Stdout
            } else if has(a, "--check") {
                OnboardMode::Check
            } else {
                OnboardMode::Apply
            };
            match result(c.beacon.onboard(mode))? {
                Onboarded::Snippet(s) => {
                    if !c.quiet {
                        c.out.stdout.extend_from_slice(s.as_bytes());
                    }
                }
                Onboarded::Checked { file, status } => c.respond(
                    command,
                    [
                        ("file", quote(&file.to_string_lossy())),
                        ("status", quote(status.as_str())),
                    ],
                    status.as_str(),
                ),
                Onboarded::Applied { file, action } => c.respond(
                    command,
                    [
                        ("file", quote(&file.to_string_lossy())),
                        ("action", quote(action.as_str())),
                    ],
                    &format!(
                        "{} {}",
                        if action == OnboardAction::Unchanged {
                            "Current"
                        } else {
                            "Updated"
                        },
                        file.display()
                    ),
                ),
            }
        }
        "doctor" => {
            let r = result(c.beacon.doctor(has(a, "--fix")))?;
            let s = r.summary();
            let checks = array(r.checks.iter().map(|ch| {
                object([
                    ("name", quote(&ch.name)),
                    ("status", quote(ch.status.as_str())),
                    ("message", quote(&ch.message)),
                    (
                        "details",
                        ch.details.as_deref().map_or_else(|| "null".into(), strings),
                    ),
                    ("fixable", ch.fixable.to_string()),
                ])
            }));
            c.respond_success(
                command,
                [
                    ("checks", checks),
                    (
                        "summary",
                        object([
                            ("pass", s.pass.to_string()),
                            ("warn", s.warn.to_string()),
                            ("fail", s.fail.to_string()),
                        ]),
                    ),
                ],
                &format!("{} passed, {} warning, {} failed", s.pass, s.warn, s.fail),
                r.success(),
            );
            if !r.success() {
                return Err("doctor found failures".into());
            }
        }
        "sync" => {
            let git = GitRunner {
                program: find_git(c.env).unwrap_or_default().into_os_string(),
                environment: if c.managed {
                    GitEnvironment::ManagedExact(
                        c.env.iter().map(|(k, v)| (k.into(), v.into())).collect(),
                    )
                } else {
                    GitEnvironment::Exact(c.env.iter().map(|(k, v)| (k.into(), v.into())).collect())
                },
            };
            let s = result(c.beacon.sync(&SyncOptions {
                status_only: has(a, "--status"),
                git,
            }))?;
            match s {
                Synced::Status { files, uncommitted } => c.respond(
                    command,
                    [
                        ("files", strings(&files)),
                        ("uncommitted", uncommitted.to_string()),
                    ],
                    &format!("{} changed file(s)", files.len()),
                ),
                Synced::NothingToCommit => c.respond(
                    command,
                    [
                        ("committed", "false".into()),
                        ("message", quote("Nothing to commit")),
                    ],
                    "Nothing to commit in .luuna/beacon/",
                ),
                Synced::Committed { files, message } => c.respond(
                    command,
                    [
                        ("committed", "true".into()),
                        ("files", strings(&files)),
                        ("message", quote(&message)),
                    ],
                    &format!("Committed {} file(s): {message}", files.len()),
                ),
            }
        }
        "completions" => completions(c, a)?,
        "skill" | "agent" | "bundle" => artifact(c, command, a)?,
        _ => unreachable!(),
    }
    Ok(())
}
fn artifact(c: &mut Context<'_>, name: &str, a: &[String]) -> Result<()> {
    let kind = match name {
        "skill" => ArtifactKind::Skill,
        "agent" => ArtifactKind::Agent,
        _ => ArtifactKind::Bundle,
    };
    // Every artifact command validates the complete store before dispatch, even an
    // unknown command or missing argument, as the reference frontend does.
    if a.is_empty() {
        return Err("artifact subcommand required".into());
    }
    result(c.beacon.artifact_store_check())?;
    let sub = &a[0];
    let command = format!("{name} {sub}");
    let reference = a.get(1).map(String::as_str);
    match sub.as_str() {
        "import" | "update" => {
            let source_at = if sub == "import" { 1 } else { 2 };
            let source = a.get(source_at).map(|s| {
                let p = Path::new(s);
                if p.is_absolute() {
                    p.to_path_buf()
                } else {
                    c.beacon.project_root().join(p)
                }
            });
            let artifact = result(if sub == "import" {
                c.beacon
                    .artifact_import(kind, source.as_deref(), value(a, "--name"))
            } else {
                c.beacon.artifact_update(kind, reference, source.as_deref())
            })?;
            c.respond(
                &command,
                [("artifact", artifact.to_go_json())],
                &format!(
                    "{} {name} {}",
                    if sub == "import" { "Stored" } else { "Updated" },
                    artifact.name
                ),
            );
        }
        "list" => {
            let items = result(c.beacon.artifact_list(kind))?;
            c.respond(
                &command,
                [
                    ("artifacts", array(items.iter().map(Artifact::to_go_json))),
                    ("count", items.len().to_string()),
                ],
                &format!("{} {name} artifact(s)", items.len()),
            );
        }
        "show" => {
            let record = result(c.beacon.artifact_show(kind, reference))?;
            c.respond(&command, [("artifact", record.to_go_json())], &record.name);
        }
        "verify" => {
            let count = result(c.beacon.artifact_verify(kind, reference))?;
            c.respond(
                &command,
                [("checked", count.to_string())],
                &format!("Verified {count} {name} version(s)"),
            );
        }
        "history" => {
            let h = result(c.beacon.artifact_history(kind, reference))?;
            c.respond(
                &command,
                [
                    ("name", quote(&h.name)),
                    (
                        "versions",
                        array(h.versions.iter().map(Artifact::to_go_json)),
                    ),
                ],
                &format!("{} version(s)", h.versions.len()),
            );
        }
        "read" => {
            let r = result(
                c.beacon
                    .artifact_read(kind, reference, a.get(2).map(String::as_str)),
            )?;
            let file = &r.file;
            let raw = object([
                ("path", quote(&file.path)),
                ("encoding", quote(&file.encoding)),
                ("executable", file.executable.to_string()),
                ("content", quote(&file.content)),
                ("sha256", quote(&file.sha256)),
            ]);
            c.respond(
                &command,
                [
                    ("name", quote(&r.name)),
                    ("version", r.version.to_string()),
                    ("file", raw),
                ],
                &file.content,
            );
        }
        "export" => {
            let e = result(c.beacon.artifact_export(
                kind,
                reference,
                &ArtifactExport {
                    out: value(a, "--out").into(),
                    target: value(a, "--target").into(),
                    force: has(a, "--force"),
                },
            ))?;
            c.respond(
                &command,
                [
                    ("name", quote(&e.name)),
                    ("version", e.version.to_string()),
                    ("path", quote(&e.path.to_string_lossy())),
                ],
                &format!("Exported {}", e.name),
            );
        }
        _ => return Err(format!("unknown {name} subcommand: {sub}")),
    }
    Ok(())
}
fn completions(c: &mut Context<'_>, a: &[String]) -> Result<()> {
    let shell = required(a, 0, "missing shell argument")?;
    if !["bash", "zsh", "fish"].contains(&shell.as_str()) {
        return Err(format!("unknown shell: {shell}"));
    }
    if c.quiet {
        return Ok(());
    }
    let commands="init create update archive list show render emit stats tree pin unpin config schema skill agent bundle prime onboard doctor validate import diff sync upgrade history completions";
    let text = match shell.as_str() {
        "bash" => include_str!("completions.bash").replace("%s", commands),
        "zsh" => include_str!("completions.zsh").replace("%s", commands),
        _ => commands
            .split_whitespace()
            .map(|cmd| format!("complete -c luuna-beacon -c lbc -c lb -f -a {cmd}\n"))
            .collect(),
    };
    c.out.stdout.extend_from_slice(text.as_bytes());
    Ok(())
}

fn find_git(env: &BTreeMap<String, String>) -> Option<std::path::PathBuf> {
    for folder in std::env::split_paths(env.get("PATH")?) {
        let candidate = folder.join("git");
        let executable = std::fs::metadata(&candidate).is_ok_and(|m| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                m.is_file() && m.permissions().mode() & 0o111 != 0
            }
            #[cfg(not(unix))]
            {
                m.is_file()
            }
        });
        if executable {
            return candidate.is_absolute().then_some(candidate);
        }
    }
    None
}
