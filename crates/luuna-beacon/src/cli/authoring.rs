use super::*;
pub(super) fn dispatch(c: &mut Context<'_>, command: &str, a: &[String]) -> Result<()> {
    match command {
        "config" => config(c, a)?,
        "pin" | "unpin" => {
            let name = required(a, 0, "prompt name required")?;
            let p = result(if command == "pin" {
                c.beacon.pin(&name)
            } else {
                c.beacon.unpin(&name)
            })?;
            c.respond(
                command,
                [
                    ("name", quote(&p.name)),
                    (
                        "pinned",
                        p.pinned.map_or_else(|| "null".into(), |v| v.to_string()),
                    ),
                ],
                &format!("{command}ned {}", p.name),
            );
        }
        "tree" => {
            let name = required(a, 0, "prompt name required")?;
            let t = result(c.beacon.tree(&name))?;
            c.respond(
                command,
                [
                    ("name", quote(&t.name)),
                    ("ancestors", strings(&t.ancestors)),
                    (
                        "mixins",
                        t.mixins.as_deref().map_or_else(|| "null".into(), strings),
                    ),
                    (
                        "tree",
                        object([
                            ("name", quote(&t.name)),
                            ("children", strings(&t.children)),
                            ("mixinUsers", strings(&t.mixin_users)),
                        ]),
                    ),
                ],
                &t.name,
            );
        }
        "stats" => {
            let s = result(c.beacon.stats())?;
            c.respond(
                command,
                [(
                    "prompts",
                    object([
                        ("total", s.total.to_string()),
                        (
                            "byStatus",
                            object(s.by_status.iter().map(|(k, v)| (k, v.to_string()))),
                        ),
                        (
                            "byTag",
                            object(s.by_tag.iter().map(|(k, v)| (k, v.to_string()))),
                        ),
                    ]),
                )],
                &format!("{} prompt(s)", s.total),
            );
        }
        "diff" => {
            if a.len() < 3 {
                return Err("name and two versions required".into());
            }
            let changes = result(c.beacon.diff(&a[0], &a[1], &a[2]))?;
            c.respond(
                command,
                [
                    ("name", quote(&a[0])),
                    ("from", quote(&a[1])),
                    ("to", quote(&a[2])),
                    (
                        "changes",
                        array(changes.iter().map(|ch| {
                            object([
                                ("section", quote(&ch.section)),
                                ("type", quote(ch.kind.as_str())),
                            ])
                        })),
                    ),
                ],
                &format!("{} change(s)", changes.len()),
            );
        }
        "import" => {
            let path = required(a, 0, "markdown path required")?;
            let imported = result(c.beacon.import_markdown(&ImportMarkdown {
                path,
                name: value(a, "--name").into(),
                tags: options(a, "--tag"),
                no_split: has(a, "--no-split"),
            }))?;
            c.respond(
                command,
                [
                    ("id", quote(&imported.prompt.id)),
                    ("name", quote(&imported.prompt.name)),
                    ("sections", imported.sections.to_string()),
                ],
                &format!("Imported {}", imported.prompt.name),
            );
        }
        "emit" => {
            let report = result(c.beacon.emit(&EmitOptions {
                selection: if has(a, "--all") || has(a, "--check") {
                    EmitSelection::All
                } else {
                    EmitSelection::Prompt(a.first().cloned().unwrap_or_default())
                },
                check: has(a, "--check"),
                dry_run: has(a, "--dry-run"),
                force: has(a, "--force"),
                out: option(a, "--out").map(str::to_owned),
                out_dir: option(a, "--out-dir").map(str::to_owned),
            }))?;
            match &report {
                EmitReport::Files(files) => c.respond(
                    command,
                    [
                        (
                            "files",
                            array(files.iter().map(|f| {
                                let mut fields = vec![
                                    ("name", quote(&f.name)),
                                    ("path", quote(&f.path)),
                                    ("version", f.version.to_string()),
                                ];
                                if f.skipped {
                                    fields.push(("skipped", "true".into()));
                                }
                                object(fields)
                            })),
                        ),
                        ("count", files.len().to_string()),
                    ],
                    &format!("Emitted {} file(s)", files.len()),
                ),
                EmitReport::Check { stale } => c.respond_success(
                    command,
                    [
                        ("check", "true".into()),
                        ("stale", strings(stale)),
                        ("upToDate", stale.is_empty().to_string()),
                    ],
                    &format!("{} stale file(s)", stale.len()),
                    stale.is_empty(),
                ),
            };
            if let Some(error) = report.failure() {
                return Err(error.to_string());
            }
        }
        _ => unreachable!(),
    }
    Ok(())
}
fn config(c: &mut Context<'_>, a: &[String]) -> Result<()> {
    let usage = "usage: luuna-beacon config <schema|show|set|unset>";
    let sub = a.first().map(String::as_str).unwrap_or("");
    if sub == "schema" {
        let mut raw = String::new();
        crate::gojson::write_any(&mut raw, &config_schema());
        c.json(raw);
        return Ok(());
    }
    if sub.is_empty() {
        return Err(usage.into());
    }
    // Go reads the original document before checking a subcommand or arity.
    let original = result(c.beacon.config_show())?;
    match sub {
        "show" => {
            let path = value(a, "--path");
            if path.is_empty() {
                if c.json {
                    c.respond(
                        "config show",
                        [("config", result(original.raw.to_go_json())?)],
                        "",
                    );
                } else {
                    c.out.stdout.extend_from_slice(&original.text);
                }
            } else {
                let v = result(c.beacon.config_get(path))?;
                if c.json {
                    c.respond(
                        "config show",
                        [("path", quote(path)), ("value", result(v.to_go_json())?)],
                        "",
                    );
                } else {
                    c.out.stdout.extend_from_slice(v.to_yaml().as_bytes());
                }
            }
        }
        "set" => {
            if a.len() < 3 {
                return Err("config path and value required".into());
            }
            let v = result(c.beacon.config_set(&a[1], &a[2]))?;
            c.respond(
                "config set",
                [("path", quote(&a[1])), ("value", result(v.to_go_json())?)],
                &format!("Set {}", a[1]),
            );
        }
        "unset" => {
            let path = required(a, 1, "config path required")?;
            let removed = result(c.beacon.config_unset(&path))?;
            c.respond(
                "config unset",
                [("path", quote(&path)), ("removed", removed.to_string())],
                &format!("Unset {path}"),
            );
        }
        _ => return Err(usage.into()),
    }
    Ok(())
}
