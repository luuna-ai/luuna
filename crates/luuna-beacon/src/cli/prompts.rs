use super::*;
fn sections(a: &[String], key: &str) -> Vec<Section> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < a.len() {
        if a[i] != key || i + 1 >= a.len() {
            i += 1;
            continue;
        }
        i += 1;
        let raw = &a[i];
        let (name, mut body) = raw.split_once('=').unwrap_or((raw, ""));
        if !raw.contains('=') && i + 2 < a.len() && a[i + 1] == "--body" {
            body = &a[i + 2];
            i += 2;
        }
        if !name.is_empty() {
            out.push(Section {
                name: name.into(),
                body: body.into(),
                required: None,
            });
        }
        i += 1;
    }
    out
}
fn frontmatter(a: &[String]) -> Vec<(String, String)> {
    options(a, "--fm")
        .into_iter()
        .filter_map(|s| {
            s.split_once('=')
                .filter(|(k, _)| !k.is_empty())
                .map(|(k, v)| (k.into(), v.into()))
        })
        .collect()
}
fn prompt_fields(p: &Prompt) -> Vec<(&'static str, String)> {
    vec![
        ("name", quote(&p.name)),
        ("version", p.version.to_string()),
        ("prompt", p.to_go_json()),
    ]
}
fn validation(v: &Validation) -> String {
    object([
        ("name", quote(&v.name)),
        ("valid", v.valid.to_string()),
        ("errors", strings(&v.errors)),
        ("warnings", strings(&v.warnings)),
    ])
}
pub(super) fn dispatch(c: &mut Context<'_>, command: &str, a: &[String]) -> Result<()> {
    match command {
        "create" => {
            let p = result(c.beacon.create(&CreatePrompt {
                name: value(a, "--name").into(),
                description: value(a, "--description").into(),
                extends: value(a, "--extends").into(),
                mixins: options(a, "--mixin"),
                tags: options(a, "--tag"),
                schema: value(a, "--schema").into(),
                emit_as: value(a, "--emit-as").into(),
                emit_dir: value(a, "--emit-dir").into(),
                status: value(a, "--status").into(),
                sections: sections(a, "--section"),
                frontmatter: frontmatter(a),
            }))?;
            let mut f = prompt_fields(&p);
            f.push(("id", quote(&p.id)));
            c.respond(command, f, &format!("Created prompt {}", p.name));
        }
        "update" => {
            let name = required(a, 0, "prompt name required")?;
            let mut s = sections(a, "--section");
            s.extend(sections(a, "--add-section"));
            let p = result(c.beacon.update(
                &name,
                &UpdatePrompt {
                    name: option(a, "--name").map(str::to_owned),
                    description: option(a, "--description").map(str::to_owned),
                    status: option(a, "--status").map(str::to_owned),
                    extends: option(a, "--extends").map(str::to_owned),
                    schema: option(a, "--schema").map(str::to_owned),
                    emit_as: option(a, "--emit-as").map(str::to_owned),
                    emit_dir: option(a, "--emit-dir").map(str::to_owned),
                    add_tags: options(a, "--tag"),
                    remove_tags: options(a, "--untag"),
                    sections: s,
                    remove_sections: options(a, "--remove-section"),
                    add_mixins: options(a, "--mixin"),
                    remove_mixins: options(a, "--remove-mixin"),
                    frontmatter: frontmatter(a),
                    remove_frontmatter: options(a, "--remove-fm"),
                },
            ))?;
            c.respond(
                command,
                prompt_fields(&p),
                &format!("Updated prompt {}", p.name),
            );
        }
        "archive" => {
            let name = required(a, 0, "prompt name required")?;
            let p = result(c.beacon.archive(&name))?;
            c.respond(
                command,
                [("name", quote(&p.name)), ("version", p.version.to_string())],
                &format!("Archived prompt {}", p.name),
            );
        }
        "list" => {
            let items = result(c.beacon.list(&ListFilter {
                status: option(a, "--status").map(str::to_owned),
                tag: option(a, "--tag").map(str::to_owned),
                extends: option(a, "--extends").map(str::to_owned),
                mixin: option(a, "--mixin").map(str::to_owned),
            }))?;
            c.respond(
                command,
                [
                    ("count", items.len().to_string()),
                    ("prompts", array(items.iter().map(Prompt::to_go_json))),
                ],
                &format!("{} prompt(s)", items.len()),
            );
        }
        "show" => {
            let name = required(a, 0, "prompt name required")?;
            let p = result(c.beacon.show(&name))?;
            c.respond(command, [("prompt", p.to_go_json())], &p.name);
        }
        "history" => {
            let name = required(a, 0, "prompt name required")?;
            let limit = if value(a, "--limit").is_empty() {
                20
            } else {
                crate::gofmt::sscan_int(value(a, "--limit"))
                    .filter(|v| *v >= 0)
                    .ok_or_else(|| format!("invalid limit: {}", value(a, "--limit")))?
                    as usize
            };
            let p = result(c.beacon.history(&name, limit))?;
            c.respond(
                command,
                [
                    ("name", quote(&name)),
                    ("versions", array(p.iter().map(Prompt::to_go_json))),
                ],
                &format!("{} version(s)", p.len()),
            );
        }
        "render" => {
            let name = required(a, 0, "prompt name required")?;
            let rendered = result(c.beacon.render(&name))?;
            let p = &rendered.prompt;
            if value(a, "--format") == "json" {
                c.json(p.to_go_json());
            } else if c.json {
                let mut f = vec![("name", quote(&p.name)), ("version", p.version.to_string())];
                let mut fm = String::new();
                if p.frontmatter.is_empty() {
                    fm.push_str("null")
                } else {
                    crate::gojson::write_map(&mut fm, &p.frontmatter)
                };
                f.push(("frontmatter", fm));
                let shallow = Prompt {
                    sections: p.sections.clone(),
                    atlas: p.atlas.clone(),
                    ..Prompt::default()
                };
                let v: serde_json::Value =
                    serde_json::from_str(&shallow.to_go_json()).map_err(|e| e.to_string())?;
                let mut s = String::new();
                crate::gojson::write_any(&mut s, &v["sections"]);
                f.push(("sections", s));
                if let Some(v) = v.get("atlas") {
                    let mut s = String::new();
                    crate::gojson::write_any(&mut s, v);
                    f.push(("atlas", s));
                }
                c.respond(command, f, "");
            } else if !c.quiet {
                c.out
                    .stdout
                    .extend_from_slice(render_markdown(p).as_bytes());
            }
        }
        "schema" => schema(c, a)?,
        "validate" => {
            if a.is_empty() {
                return Err("prompt name or --all required".into());
            }
            let items = if has(a, "--all") {
                result(c.beacon.validate_all())?
            } else {
                vec![result(c.beacon.validate(&a[0]))?]
            };
            let valid = items.iter().all(|v| v.valid);
            c.respond_success(
                command,
                [("results", array(items.iter().map(validation)))],
                &format!("{} prompt(s) validated", items.len()),
                valid,
            );
            if !valid {
                return Err("validation failed".into());
            }
        }
        _ => unreachable!(),
    }
    Ok(())
}
fn schema(c: &mut Context<'_>, a: &[String]) -> Result<()> {
    let usage = "usage: luuna-beacon schema <create|show|list|rule add>";
    match a.first().map(String::as_str).unwrap_or("") {
        "create" => {
            let required = value(a, "--required");
            let optional = value(a, "--optional").trim_matches(',');
            let s = result(c.beacon.schema_create(&CreateSchema {
                name: value(a, "--name").into(),
                required: if required.is_empty() {
                    vec![]
                } else {
                    required.split(',').map(str::to_owned).collect()
                },
                optional: if optional.is_empty() {
                    vec![]
                } else {
                    optional.split(',').map(str::to_owned).collect()
                },
            }))?;
            c.respond(
                "schema create",
                [("id", quote(&s.id)), ("name", quote(&s.name))],
                &format!("Created schema {}", s.name),
            );
        }
        "list" => {
            let s = result(c.beacon.schema_list())?;
            c.respond(
                "schema list",
                [
                    ("schemas", array(s.iter().map(Schema::to_go_json))),
                    ("count", s.len().to_string()),
                ],
                &format!("{} schema(s)", s.len()),
            );
        }
        "show" => {
            let name = required(a, 1, "schema name required")?;
            let s = result(c.beacon.schema_show(&name))?;
            c.respond("schema show", [("schema", s.to_go_json())], &s.name);
        }
        "rule" => {
            if a.len() < 3 || a[1] != "add" {
                return Err("usage: luuna-beacon schema rule add <schema> --section <name> --pattern <regex> --message <text>".into());
            }
            let rule = Rule {
                section: value(a, "--section").into(),
                pattern: value(a, "--pattern").into(),
                message: value(a, "--message").into(),
            };
            let s = result(c.beacon.schema_rule_add(&a[2], &rule))?;
            c.respond(
                "schema rule add",
                [
                    ("schema", quote(&s.name)),
                    (
                        "rule",
                        object([
                            ("section", quote(&rule.section)),
                            ("pattern", quote(&rule.pattern)),
                            ("message", quote(&rule.message)),
                        ]),
                    ),
                ],
                &format!("Added rule to {}", s.name),
            );
        }
        _ => return Err(usage.into()),
    }
    Ok(())
}
