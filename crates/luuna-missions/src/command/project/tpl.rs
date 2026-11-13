//! Go's template commands; raw store rewrites use targeted A8 preflight.
use super::{
    config::nested_path,
    lossy,
    template_record::{Template, TemplateStep},
};
use crate::{
    command::{
        options,
        output::Envelope,
        read::{closed_ids, effectively_blocked, one_line_ticket, println},
        shared::{parse_priority, validate_type},
        Context, Failure,
    },
    config,
    error::Error,
    json,
    model::{self, Ticket},
    store::{self, DataFile},
};
use std::collections::HashSet;
pub(super) fn strings(items: &[String]) -> Vec<u8> {
    let mut out = vec![b'['];
    for (i, s) in items.iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        json::quote(&mut out, s, false);
    }
    out.push(b']');
    out
}
fn subcommand(p: &[String]) -> Result<&str, Failure> {
    p.first().map(String::as_str).ok_or_else(|| {
        Error::unsupported(
            "tpl with no subcommand after its options (Go panics, SD10)",
            "none (Go panics, SD10)",
        )
        .into()
    })
}
fn find<'a>(templates: &'a [Template], id: &str) -> Result<&'a Template, Failure> {
    templates
        .iter()
        .find(|t| t.id == id)
        .ok_or_else(|| Failure::message(format!("Template not found: {id}")))
}
pub(super) fn run(context: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    let path = nested_path(args, 1)?;
    if path.is_empty() {
        return Err(Failure::message(
            "Usage: luuna-missions tpl <create|step|list|show|pour|status>",
        ));
    }
    let flags = match path[0].as_str() {
        "create" => &["name"][..],
        "pour" => &["prefix"][..],
        "step" => {
            let p = nested_path(args, 2)?;
            if p.get(1).map(String::as_str) == Some("add") {
                &["title", "type", "priority"][..]
            } else {
                &[]
            }
        }
        _ => &[],
    };
    let opts = options::parse(args, flags, &["json"])?;
    let p = &opts.positionals;
    let store = context.store()?;
    let template_path = store.path(DataFile::Templates);
    let cmd = subcommand(p)?;
    match cmd {
        "create" => {
            let name = opts.value("name");
            if name.is_empty() {
                return Err(Failure::message("--name is required"));
            }
            let id;
            {
                let _lock = store.lock(DataFile::Templates)?;
                let templates = store::read_records::<Template>(&template_path)?;
                let ids = templates.iter().map(|t| t.id.clone()).collect();
                id = model::generate_id("tpl", &ids);
                store::append_record(&template_path, &Template::new(id.clone(), name.into()))?;
            }
            if opts.has("json") {
                context.json(Envelope::new(true, "tpl create").string("id", &id));
            } else {
                context.success(format!("Created template {id}: {name}").as_bytes());
            }
        }
        "step" => {
            if p.len() < 2 || p[1] != "add" {
                return Err(Failure::message(
                    "Usage: luuna-missions tpl step add <id> --title ...",
                ));
            }
            if p.len() < 3 {
                return Err(Failure::message("Template ID is required"));
            }
            let id = &p[2];
            let title = opts.value("title");
            if title.is_empty() {
                return Err(Failure::message("--title is required"));
            }
            let kind = if opts.value("type").is_empty() {
                "task"
            } else {
                opts.value("type")
            };
            validate_type(kind).map_err(Failure::message)?;
            let priority = parse_priority(opts.value("priority")).map_err(Failure::message)?;
            let count;
            {
                let _lock = store.lock(DataFile::Templates)?;
                let mut templates = store::read_records::<Template>(&template_path)?;
                let index = templates
                    .iter()
                    .position(|t| &t.id == id)
                    .ok_or_else(|| Failure::message(format!("Template not found: {id}")))?;
                let steps = templates[index].steps.get_or_insert_with(Vec::new);
                steps.push(TemplateStep::new(title.into(), kind.into(), priority));
                count = steps.len();
                lossy::refuse_lossy_rewrite::<Template>(&template_path)?;
                lossy::refuse_replaced_steps(&template_path, id)?;
                store::write_records(&template_path, &templates)?;
            }
            if opts.has("json") {
                context.json(
                    Envelope::new(true, "tpl step add")
                        .string("id", id)
                        .integer("stepCount", count as i64),
                );
            } else {
                context.success(
                    format!(
                        "Added step {count} to {id}: {}",
                        crate::config_tree::emit::quote(title)
                    )
                    .as_bytes(),
                );
            }
        }
        "list" => {
            let templates = store::read_records::<Template>(&template_path)?;
            if opts.has("json") {
                let mut raw = vec![b'['];
                for (i, t) in templates.iter().enumerate() {
                    if i > 0 {
                        raw.push(b',');
                    }
                    raw.extend(t.marshal()?);
                }
                raw.push(b']');
                context.json_with_records(
                    Envelope::new(true, "tpl list")
                        .raw("templates", &raw)
                        .integer("count", templates.len() as i64),
                    "model.Template",
                )?;
            } else if templates.is_empty() {
                println(context, "No templates.");
            } else {
                for t in templates {
                    println(
                        context,
                        &format!("{}  {}  ({} steps)", t.id, t.name, t.steps().len()),
                    );
                }
            }
        }
        "show" => {
            if p.len() < 2 {
                return Err(Failure::message("Usage: luuna-missions tpl show <id>"));
            }
            let templates = store::read_records::<Template>(&template_path)?;
            let t = find(&templates, &p[1])?;
            if opts.has("json") {
                context.json_with_records(
                    Envelope::new(true, "tpl show").raw("template", &t.marshal()?),
                    "*model.Template",
                )?;
            } else {
                println(
                    context,
                    &format!("{}  {}\nSteps ({}):", t.id, t.name, t.steps().len()),
                );
                for (i, step) in t.steps().iter().enumerate() {
                    let kind = if step.ticket_type.is_empty() {
                        "task"
                    } else {
                        &step.ticket_type
                    };
                    println(
                        context,
                        &format!(
                            "  {}. {}  [{} P{}]",
                            i + 1,
                            step.title,
                            kind,
                            step.priority.unwrap_or(2)
                        ),
                    );
                }
            }
        }
        "pour" => {
            if p.len() < 2 {
                return Err(Failure::message(
                    "Usage: luuna-missions tpl pour <id> --prefix ...",
                ));
            }
            if !opts.has("prefix") {
                return Err(Failure::message("--prefix is required"));
            }
            let id = &p[1];
            let prefix = opts.value("prefix");
            let templates = store::read_records::<Template>(&template_path)?;
            let tpl = find(&templates, id)?;
            if tpl.steps().is_empty() {
                return Err(Failure::message(format!("Template {id} has no steps")));
            }
            let project = config::read_config(store.dir())?;
            let mut created = Vec::new();
            {
                let _lock = store.lock(DataFile::Issues)?;
                let mut issues = store.read_tickets()?;
                let mut ids = issues.iter().map(|t| t.id.clone()).collect::<HashSet<_>>();
                let now = model::now_iso();
                let mut fresh = Vec::<Ticket>::new();
                for step in tpl.steps() {
                    let id_new = model::generate_id(&project.project, &ids);
                    ids.insert(id_new.clone());
                    created.push(id_new.clone());
                    let mut ticket = Ticket::default();
                    ticket.id = id_new;
                    ticket.title = step.title.replace("{prefix}", prefix);
                    ticket.status = "open".into();
                    ticket.ticket_type = if step.ticket_type.is_empty() {
                        "task".into()
                    } else {
                        step.ticket_type.clone()
                    };
                    ticket.priority = step.priority.unwrap_or(2);
                    ticket.created_at = now.clone();
                    ticket.updated_at = now.clone();
                    ticket.convoy = id.clone();
                    fresh.push(ticket);
                }
                for i in 1..fresh.len() {
                    fresh[i].blocked_by = Some(vec![fresh[i - 1].id.clone()]);
                    fresh[i - 1].blocks = Some(vec![fresh[i].id.clone()]);
                }
                issues.extend(fresh);
                lossy::refuse_lossy_rewrite::<Ticket>(&store.path(DataFile::Issues))?;
                store.write_tickets(&issues)?;
            }
            if opts.has("json") {
                context.json(Envelope::new(true, "tpl pour").raw("ids", &strings(&created)));
            } else {
                context.success(
                    format!("Poured template {id} — created {} issues", created.len()).as_bytes(),
                );
                for id in created {
                    println(context, &format!("  {id}"));
                }
            }
        }
        "status" => {
            if p.len() < 2 {
                return Err(Failure::message("Usage: luuna-missions tpl status <id>"));
            }
            let id = &p[1];
            let issues = store.read_tickets()?;
            let convoy = issues
                .iter()
                .filter(|t| &t.convoy == id)
                .collect::<Vec<_>>();
            if convoy.is_empty() {
                if opts.has("json") {
                    context.json(
                        Envelope::new(true, "tpl status")
                            .string("templateId", id)
                            .integer("total", 0)
                            .raw("issues", b"[]"),
                    );
                } else {
                    println(context, &format!("No issues found for convoy {id}"));
                }
                return Ok(());
            }
            let closed = closed_ids(&issues);
            let (mut completed, mut progress, mut blocked) = (0, 0, 0);
            let mut ids = Vec::new();
            for t in &convoy {
                ids.push(t.id.clone());
                if t.status == "closed" {
                    completed += 1;
                } else if t.status == "in_progress" {
                    progress += 1;
                }
                if t.status != "closed" && effectively_blocked(t, &closed) {
                    blocked += 1;
                }
            }
            if opts.has("json") {
                let status = Envelope::object()
                    .string("templateId", id)
                    .integer("total", convoy.len() as i64)
                    .integer("completed", completed)
                    .integer("inProgress", progress)
                    .integer("blocked", blocked)
                    .raw("issues", &strings(&ids))
                    .render();
                context.json(Envelope::new(true, "tpl status").raw("status", &status));
            } else {
                println(context,&format!("Convoy: {id}\n  Total:       {}\n  Completed:   {completed}\n  In progress: {progress}\n  Blocked:     {blocked}\n  Issues:",convoy.len()));
                for t in convoy {
                    println(context, &format!("    {}", one_line_ticket(t, &closed)));
                }
            }
        }
        other => {
            return Err(Failure::message(format!(
                "Unknown tpl subcommand: {other}. Use create, step, list, show, pour, or status."
            )))
        }
    }
    Ok(())
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::config_tree::tree::test_support::{invoke, project, store as make_store};
    use std::{fs, path::Path};
    const CONFIG: &str = "project: demo\nversion: \"1\"\n";
    const TEMPLATE: &[u8] =
        b"{\"id\":\"tpl-1\",\"name\":\"Test\",\"steps\":[{\"title\":\"old\"}]}\n";
    fn file(root: &Path, name: &str) -> std::path::PathBuf {
        root.join(".luuna/missions").join(name)
    }
    fn project_with(template: &[u8]) -> tempfile::TempDir {
        let t = project(CONFIG);
        fs::write(file(t.path(), "templates.jsonl"), template).unwrap();
        t
    }
    fn names(root: &Path) -> Vec<String> {
        let mut names = fs::read_dir(root.join(".luuna/missions"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        names.sort();
        names
    }
    pub fn refuse_file(
        name: &str,
        content: &[u8],
        args: &[&str],
        message: impl Fn(&str) -> String,
    ) {
        for mode in [0o644, 0o600] {
            let t = project_with(TEMPLATE);
            let path = file(t.path(), name);
            fs::write(&path, content).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
            }
            #[cfg(not(unix))]
            let _ = mode;
            let old = fs::read(&path).unwrap();
            let permissions = fs::metadata(&path).unwrap().permissions();
            let entries = names(t.path());
            let out = invoke(t.path(), args);
            assert_eq!(out.exit_code, 1);
            assert!(out.stdout.is_empty());
            assert_eq!(
                out.stderr,
                format!("Error: {}\n", message(&path.to_string_lossy())).as_bytes()
            );
            assert_eq!(fs::read(&path).unwrap(), old);
            assert_eq!(fs::metadata(&path).unwrap().permissions(), permissions);
            assert_eq!(names(t.path()), entries);
        }
    }
    fn file_cases(name: &str, args: &[&str], target: &[u8]) {
        for (bad, reason) in [
            (
                b"bad\n".as_slice(),
                "line 1 is not a record the tool can read",
            ),
            (
                b"{\"id\":\"x\"}\n{\"id\":\"x\"}\n",
                "line 2 repeats the id \"x\" of line 1",
            ),
            (
                b"{\"id\":\"x\",\"id\":\"y\"}\n",
                "line 1 repeats the key \"id\"",
            ),
        ] {
            let bytes = [bad, target].concat();
            refuse_file(name, &bytes, args, |path| {
                format!("Refusing to rewrite {path}: {reason}, and the Go tool would drop it without a message. Nothing was written.")
            });
        }
    }
    #[test]
    fn step_add_refuses_a_lossy_rewrite_and_the_file_is_unchanged() {
        file_cases(
            "templates.jsonl",
            &["tpl", "step", "add", "tpl-1", "--title", "new"],
            TEMPLATE,
        );
        let t = project_with(TEMPLATE);
        assert_eq!(
            invoke(t.path(), &["tpl", "step", "add", "tpl-1", "--title", "new"]).exit_code,
            0
        );
        // Go keeps an unrelated raw step with duplicate keys; changing it loses the first.
        let bytes=b"{\"id\":\"tpl-1\",\"name\":\"Test\",\"steps\":[{\"title\":\"one\",\"title\":\"two\"}]}\n";
        refuse_file(
            "templates.jsonl",
            bytes,
            &["tpl", "step", "add", "tpl-1", "--title", "new"],
            |path| {
                format!("Refusing to rewrite {path}: line 1 repeats the key \"title\", and the Go tool would drop it without a message. Nothing was written.")
            },
        );
    }
    #[test]
    fn pour_refuses_a_lossy_rewrite_and_the_file_is_unchanged() {
        file_cases(
            "issues.jsonl",
            &["tpl", "pour", "tpl-1", "--prefix", "P"],
            b"{\"id\":\"kept\"}\n",
        );
        let t = project_with(TEMPLATE);
        fs::write(file(t.path(), "issues.jsonl"), b"").unwrap();
        assert_eq!(
            invoke(t.path(), &["tpl", "pour", "tpl-1", "--prefix", "P"]).exit_code,
            0
        );
    }
    #[test]
    fn step_add_refuses_steps_that_are_not_a_list_and_the_file_is_unchanged() {
        for bytes in [
            b"{\"id\":\"tpl-1\",\"steps\":\"keep me\"}\n".as_slice(),
            b"{\"id\":\"tpl-1\",\"steps\":{\"title\":\"old\"}}\n",
        ] {
            refuse_file(
                "templates.jsonl",
                bytes,
                &["tpl", "step", "add", "tpl-1", "--title", "new"],
                |path| {
                    format!("Refusing to rewrite {path}: line 1 holds template \"tpl-1\" whose steps is not a list, and the Go tool would replace it without a message. Nothing was written.")
                },
            );
        }
    }
    #[test]
    fn step_add_refuses_a_step_that_is_not_an_object_and_the_file_is_unchanged() {
        refuse_file(
            "templates.jsonl",
            b"{\"id\":\"tpl-1\",\"steps\":[{\"title\":\"old\"},5,{\"title\":\"last\"}]}\n",
            &["tpl", "step", "add", "tpl-1", "--title", "new"],
            |path| {
                format!("Refusing to rewrite {path}: line 1 holds template \"tpl-1\" whose steps item 2 is not an object, and the Go tool would replace it without a message. Nothing was written.")
            },
        );
    }
    #[test]
    fn step_add_refuses_a_key_go_would_reencode_unchanged() {
        for raw in [b"{\"id\":\"tpl-1\",\"name\":\"Test\",\"steps\":[{\"title\":\"old\",\"\xff\":\"keep\"}]}\n".as_slice(),b"{\"id\":\"tpl-1\",\"name\":\"Test\",\"steps\":[{\"title\":\"old\",\"\\ud800\":\"keep\"}]}\n"]{refuse_file("templates.jsonl",raw,&["tpl","step","add","tpl-1","--title","new"],|path|format!("Refusing to rewrite {path}: line 1 holds template \"tpl-1\" whose steps item 1 has a key with invalid UTF-8 or an unpaired surrogate escape, and the Go tool would write it as U+FFFD. Nothing was written."));
            let input=[raw,b"{\"id\":\"other\",\"name\":\"Other\",\"steps\":[]}\n"].concat();let t=project_with(&input);assert_eq!(invoke(t.path(),&["tpl","step","add","other","--title","new"]).exit_code,0);assert!(fs::read(file(t.path(),"templates.jsonl")).unwrap().starts_with(raw));
        }
    }
    #[test]
    fn step_add_keeps_existing_steps_byte_for_byte_as_go_does() {
        let cases:[(&[u8],&[u8]);6]=[
        (b"{\"id\":\"tpl-1\",\"name\":\"n\",\"steps\":[{\"title\":\"old\"}]}\n",b"{\"id\":\"tpl-1\",\"name\":\"n\",\"steps\":[{\"title\":\"old\"},{\"title\":\"new\",\"type\":\"task\",\"priority\":2}]}\n"),
        (b"{\"id\":\"tpl-1\",\"name\":\"n\",\"steps\":[{\"title\":\"old\",\"priority\":\"high\"}]}\n",b"{\"id\":\"tpl-1\",\"name\":\"n\",\"steps\":[{\"title\":\"old\",\"priority\":\"high\"},{\"title\":\"new\",\"type\":\"task\",\"priority\":2}]}\n"),
        (b"{\"id\":\"tpl-1\",\"name\":\"n\",\"steps\":[{\"title\":\"old\",\"extra\":{\"x\":1,\"x\":2}}]}\n",b"{\"id\":\"tpl-1\",\"name\":\"n\",\"steps\":[{\"title\":\"old\",\"extra\":{\"x\":1,\"x\":2}},{\"title\":\"new\",\"type\":\"task\",\"priority\":2}]}\n"),
        (b"{\"id\":\"tpl-1\",\"name\":\"n\",\"steps\":[{\"title\":\"x\xff\"}]}\n",b"{\"id\":\"tpl-1\",\"name\":\"n\",\"steps\":[{\"title\":\"x\xff\"},{\"title\":\"new\",\"type\":\"task\",\"priority\":2}]}\n"),
        (b"{\"id\":\"tpl-1\",\"name\":\"n\",\"steps\":[{\"title\":\"x\\ud800\"}]}\n",b"{\"id\":\"tpl-1\",\"name\":\"n\",\"steps\":[{\"title\":\"x\\ud800\"},{\"title\":\"new\",\"type\":\"task\",\"priority\":2}]}\n"),
        (b"{\"id\":\"tpl-1\",\"name\":\"x\xff\",\"steps\":[]}\n",b"{\"id\":\"tpl-1\",\"name\":\"x\xff\",\"steps\":[{\"title\":\"new\",\"type\":\"task\",\"priority\":2}]}\n")];
        for (before, after) in cases {
            let t = project_with(before);
            assert_eq!(
                invoke(t.path(), &["tpl", "step", "add", "tpl-1", "--title", "new"]).exit_code,
                0
            );
            assert_eq!(fs::read(file(t.path(), "templates.jsonl")).unwrap(), after);
        }
        let raw=b"{\"id\":\"other\",\"name\":\"Other\",\"steps\":[{\"title\":\"one\",\"title\":\"two\"}]}\n";
        let t = project_with(&[TEMPLATE, raw].concat());
        assert_eq!(
            invoke(t.path(), &["tpl", "step", "add", "tpl-1", "--title", "new"]).exit_code,
            0
        );
        assert!(fs::read(file(t.path(), "templates.jsonl"))
            .unwrap()
            .ends_with(raw));
    }
    #[test]
    fn nested_leaves_reject_sibling_options() {
        let t = project_with(TEMPLATE);
        for args in [
            &["tpl", "create", "--name", "new", "--prefix", "x"][..],
            &["tpl", "step", "add", "tpl-1", "--name", "x"],
            &["tpl", "pour", "tpl-1", "--name", "x"],
            &["tpl", "list", "--title", "x"],
            &["tpl", "show", "tpl-1", "--priority", "1"],
            &["tpl", "status", "tpl-1", "--type", "bug"],
        ] {
            let out = invoke(t.path(), args);
            assert_eq!(out.exit_code, 1);
            assert!(String::from_utf8_lossy(&out.stderr).starts_with("error: unknown option '"));
        }
        assert_eq!(
            fs::read(file(t.path(), "templates.jsonl")).unwrap(),
            TEMPLATE
        );
    }
    #[test]
    fn nested_leaves_keep_their_own_options() {
        let t = project_with(TEMPLATE);
        let out = invoke(
            t.path(),
            &[
                "tpl",
                "step",
                "add",
                "tpl-1",
                "--title",
                "next",
                "--type",
                "bug",
                "--priority",
                "P1",
                "--json",
            ],
        );
        assert_eq!(out.stdout,b"{\n  \"success\": true,\n  \"command\": \"tpl step add\",\n  \"id\": \"tpl-1\",\n  \"stepCount\": 2\n}\n");
        assert_eq!(
            invoke(t.path(), &["tpl", "show", "tpl-1"]).stdout,
            b"tpl-1  Test\nSteps (2):\n  1. old  [task P2]\n  2. next  [bug P1]\n"
        );
        assert_eq!(
            invoke(t.path(), &["tpl", "list"]).stdout,
            b"tpl-1  Test  (2 steps)\n"
        );
        assert_eq!(invoke(t.path(),&["tpl","status","tpl-1","--json"]).stdout,b"{\n  \"success\": true,\n  \"command\": \"tpl status\",\n  \"templateId\": \"tpl-1\",\n  \"total\": 0,\n  \"issues\": []\n}\n");
    }
    #[test]
    fn template_records_round_trip_in_go_order() {
        use crate::store::Record;
        let raw=br#"{"name":"n","id":"t","steps":[{"priority":"high","title":"old","extra":7}],"unknown":4}"#;
        let t = Template::from_line(raw).unwrap();
        assert_eq!(t.steps()[0].priority, Some(0));
        assert_eq!(t.to_line().unwrap(), raw);
        let null = project_with(b"{\"id\":\"tpl-1\",\"name\":\"Test\",\"steps\":null}\n");
        assert_eq!(
            invoke(
                null.path(),
                &["tpl", "step", "add", "tpl-1", "--title", "new"]
            )
            .exit_code,
            0
        );
        assert_eq!(fs::read(file(null.path(),"templates.jsonl")).unwrap(),b"{\"id\":\"tpl-1\",\"name\":\"Test\",\"steps\":[{\"title\":\"new\",\"type\":\"task\",\"priority\":2}]}\n");
    }
    #[test]
    fn pour_wires_dependencies_and_convoy() {
        let t=project_with(b"{\"id\":\"tpl-1\",\"name\":\"Test\",\"steps\":[{\"title\":\"{prefix} first\",\"type\":\"bug\",\"priority\":1},{\"title\":\"next\"}]}\n");
        let out = invoke(
            t.path(),
            &["tpl", "pour", "tpl-1", "--prefix", "P", "--json"],
        );
        assert_eq!(out.exit_code, 0);
        let rows = crate::store::read_records::<Ticket>(&file(t.path(), "issues.jsonl")).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].title, "P first");
        assert_eq!(rows[0].ticket_type, "bug");
        assert_eq!(rows[0].priority, 1);
        assert_eq!(rows[0].convoy, "tpl-1");
        assert_eq!(rows[0].blocks, Some(vec![rows[1].id.clone()]));
        assert_eq!(rows[1].blocked_by, Some(vec![rows[0].id.clone()]));
        assert_eq!(rows[0].created_at, rows[1].updated_at);
        let status = invoke(t.path(), &["tpl", "status", "tpl-1", "--json"]);
        let v: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
        assert_eq!(v["status"]["total"], 2);
        assert_eq!(v["status"]["blocked"], 1);
    }
    #[test]
    fn a_panicking_input_is_unsupported_naming_no_plan() {
        match subcommand(&[]).unwrap_err() {
            Failure::Library(Error::Unsupported { feature, plan }) => {
                assert_eq!(
                    feature,
                    "tpl with no subcommand after its options (Go panics, SD10)"
                );
                assert_eq!(plan, "none (Go panics, SD10)");
            }
            other => panic!("wrong refusal {other:?}"),
        };
        let t = project_with(TEMPLATE);
        assert_eq!(
            invoke(t.path(), &["tpl", "--json"]).stderr,
            b"error: unknown option '--json'\n"
        );
    }
    #[test]
    fn step_add_and_pour_leave_the_parent_of_the_temporary_project_unchanged() {
        let parent = tempfile::tempdir().unwrap();
        let sentinel = parent.path().join("sentinel");
        fs::write(&sentinel, b"keep").unwrap();
        let mode = fs::metadata(&sentinel).unwrap().permissions();
        let root = parent.path().join("project");
        make_store(&root, CONFIG);
        let out = invoke(&root, &["tpl", "create", "--name", "Test", "--json"]);
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let id = v["id"].as_str().unwrap();
        assert!(id.starts_with("tpl-"));
        assert_eq!(
            invoke(&root, &["tpl", "step", "add", id, "--title", "first"]).exit_code,
            0
        );
        assert_eq!(
            invoke(&root, &["tpl", "pour", id, "--prefix", "P"]).exit_code,
            0
        );
        assert_eq!(fs::read(&sentinel).unwrap(), b"keep");
        assert_eq!(fs::metadata(&sentinel).unwrap().permissions(), mode);
        assert_eq!(fs::read_dir(parent.path()).unwrap().count(), 2);
        assert!(!parent.path().join(".luuna").exists());
    }
}
