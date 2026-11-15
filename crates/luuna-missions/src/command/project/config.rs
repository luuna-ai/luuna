//! Complete config commands, with Go's nested option boundaries.
use crate::command::{options, output::Envelope, Context, Failure};
use crate::{
    config_tree::{self, emit, tree, validate, yaml, TreeValue as V},
    json,
    store::DataFile,
};
pub(super) fn nested_path(args: &[String], depth: usize) -> Result<Vec<String>, Failure> {
    let mut out = Vec::new();
    let mut after = false;
    for arg in args {
        if out.len() == depth {
            break;
        }
        if !after && arg == "--" {
            after = true;
            continue;
        }
        if !after && arg.starts_with('-') {
            options::parse(std::slice::from_ref(arg), &[], &[])?;
            continue;
        }
        out.push(arg.clone());
    }
    Ok(out)
}
fn parts(path: &str) -> Result<Vec<&str>, Failure> {
    let parts = path
        .split('.')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    if parts.is_empty() {
        Err(Failure::message("Config path must be non-empty"))
    } else {
        Ok(parts)
    }
}
fn at<'a>(mut value: &'a V, parts: &[&str]) -> Option<&'a V> {
    for part in parts {
        value = value.get(part)?;
    }
    Some(value)
}
fn describe(value: &V) -> &str {
    match value {
        V::Null => "null",
        V::Str(_) => "string",
        V::Bool(_) => "boolean",
        V::Int(_) | V::Float(_) => "number",
        V::Seq(_) => "array",
        _ => "object",
    }
}
fn set(root: &mut V, parts: &[&str], value: V) -> Result<(), Failure> {
    let mut current = root;
    for (i, part) in parts[..parts.len() - 1].iter().enumerate() {
        if current.get(part).is_none_or(|v| matches!(v, V::Null)) {
            current.set((*part).into(), V::Map(Vec::new()));
        }
        let next = current.get_mut(part).expect("mapping member created");
        if !matches!(next, V::Map(_)) {
            return Err(Failure::message(format!(
                "Cannot set: '{}' is not an object (got: {})",
                parts[..=i].join("."),
                describe(next)
            )));
        }
        current = next;
    }
    current.set(parts[parts.len() - 1].into(), value);
    Ok(())
}
fn unset(root: &mut V, parts: &[&str]) -> bool {
    let mut current = root;
    for part in &parts[..parts.len() - 1] {
        let Some(v) = current.get_mut(part) else {
            return false;
        };
        if !matches!(v, V::Map(_)) {
            return false;
        }
        current = v;
    }
    if let V::Map(entries) = current {
        if let Some(i) = entries
            .iter()
            .position(|(k, _)| k == parts[parts.len() - 1])
        {
            entries.remove(i);
            return true;
        }
    }
    false
}
pub(super) fn parse_value(raw: &str) -> Result<V, Failure> {
    if ["", "null", "~"].contains(&raw) {
        return Ok(V::Null);
    }
    if raw == "true" {
        return Ok(V::Bool(true));
    }
    if raw == "false" {
        return Ok(V::Bool(false));
    }
    let unsigned = raw.strip_prefix('-').unwrap_or(raw);
    let numbers = unsigned.split('.').collect::<Vec<_>>();
    if numbers.len() <= 2
        && numbers
            .iter()
            .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
    {
        if numbers.len() == 2 {
            let n = raw.parse::<f64>().map_err(|_| {
                Failure::message(format!(
                    "strconv.ParseFloat: parsing {}: value out of range",
                    emit::quote(raw)
                ))
            })?;
            if !n.is_finite() {
                return Err(Failure::message(format!(
                    "strconv.ParseFloat: parsing {}: value out of range",
                    emit::quote(raw)
                )));
            }
            return Ok(V::Float(n));
        }
        return raw.parse::<i64>().map(V::Int).map_err(|_| {
            Failure::message(format!(
                "strconv.Atoi: parsing {}: value out of range",
                emit::quote(raw)
            ))
        });
    }
    if raw.starts_with('[') || raw.starts_with('{') {
        if raw.starts_with('[') && !raw.ends_with(']') {
            return Err(Failure::message(format!("Unclosed flow sequence: {raw}")));
        }
        if raw.starts_with('{') && !raw.ends_with('}') {
            return Err(Failure::message(format!("Unclosed flow mapping: {raw}")));
        }
        return Ok(yaml::decode_flow_value(raw.as_bytes())?);
    }
    if raw.starts_with('"') {
        if let Some(s) = json::Value::decode(raw.as_bytes())
            .ok()
            .and_then(json::Value::into_string)
        {
            return Ok(V::Str(s));
        }
        if raw.len() > 1 && raw.ends_with('"') {
            return Ok(V::Str(raw[1..raw.len() - 1].into()));
        }
    }
    if raw.starts_with('\'') && raw.ends_with('\'') && raw.len() > 1 {
        return Ok(V::Str(raw[1..raw.len() - 1].replace("''", "'")));
    }
    Ok(V::Str(raw.into()))
}
pub(super) fn run(context: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    let path = nested_path(args, 1)?;
    if path.is_empty() {
        return Err(Failure::message(
            "Usage: luuna-missions config <schema|show|set|unset>",
        ));
    }
    let cmd = path[0].as_str();
    let opts = options::parse(
        args,
        if cmd == "show" { &["path"][..] } else { &[] },
        &["json"],
    )?;
    if cmd == "schema" {
        let bytes = include_bytes!("../../../assets/s4-missions-lib-3/schema.json");
        let data = if opts.has("json") {
            bytes.to_vec()
        } else {
            json::reformat(
                bytes,
                json::Format {
                    indent: Some("  "),
                    ..json::Format::COMPACT
                },
            )
            .map_err(|e| Failure::message(e.to_string()))?
        };
        context.stdout.extend_from_slice(&data);
        context.stdout.push(b'\n');
        return Ok(());
    }
    let store = context.store()?;
    let dir = store.dir();
    if cmd == "show" {
        let tree = config_tree::read_tree(dir)?;
        if opts.has("path") {
            let path = opts.value("path");
            let parts = parts(path)?;
            let value = at(&tree, &parts)
                .ok_or_else(|| Failure::message(format!("Path not found: {path}")))?;
            if opts.has("json") {
                context.json(
                    Envelope::new(true, "config show")
                        .string("path", path)
                        .raw("value", &tree::json_bytes(value, false)?),
                );
            } else {
                match value {
                    V::Map(_) => context.stdout.extend_from_slice(
                        emit::stringify_tree(value, &Default::default())?.as_bytes(),
                    ),
                    V::Str(s) => {
                        context.stdout.extend_from_slice(s.as_bytes());
                        context.stdout.push(b'\n');
                    }
                    v => {
                        context
                            .stdout
                            .extend_from_slice(&tree::json_bytes(v, false)?);
                        context.stdout.push(b'\n');
                    }
                }
            }
            return Ok(());
        }
        if opts.has("json") {
            context.json(
                Envelope::new(true, "config show").raw("config", &tree::json_bytes(&tree, false)?),
            );
        } else {
            context
                .stdout
                .extend_from_slice(tree::display_tree(dir, &tree)?.as_bytes());
        }
        return Ok(());
    }
    if cmd == "set" {
        if opts.positionals.len() < 3 {
            return Err(Failure::message(
                "Usage: luuna-missions config set <path> <value>",
            ));
        }
        let path = &opts.positionals[1];
        let parts = parts(path)?;
        let value = parse_value(&opts.positionals[2])?;
        {
            let _lock = store.lock(DataFile::Config)?;
            let mut tree = config_tree::read_tree(dir)?;
            set(&mut tree, &parts, value.clone())?;
            validate::validate_tree(&tree)?;
            tree::write_tree(dir, &tree)?;
        }
        if opts.has("json") {
            context.json(
                Envelope::new(true, "config set")
                    .string("path", path)
                    .raw("value", &tree::json_bytes(&value, false)?),
            );
        } else {
            let display = if let Some(text) = value.text() {
                text.to_owned()
            } else if matches!(value, V::Null) {
                "null".into()
            } else {
                String::from_utf8(tree::json_bytes(&value, true)?).unwrap()
            };
            context.success(format!("Set {path} = {display}").as_bytes());
        }
        return Ok(());
    }
    if cmd == "unset" {
        if opts.positionals.len() < 2 {
            return Err(Failure::message(
                "Usage: luuna-missions config unset <path>",
            ));
        }
        let path = &opts.positionals[1];
        let parts = parts(path)?;
        let removed;
        {
            let _lock = store.lock(DataFile::Config)?;
            let mut tree = config_tree::read_tree(dir)?;
            removed = unset(&mut tree, &parts);
            if removed {
                validate::validate_tree(&tree)?;
                tree::write_tree(dir, &tree)?;
            }
        }
        if opts.has("json") {
            context.json(
                Envelope::new(true, "config unset")
                    .string("path", path)
                    .boolean("removed", removed),
            );
        } else if removed {
            context.success(format!("Unset {path}").as_bytes());
        } else {
            context
                .stdout
                .extend_from_slice(format!("No such path: {path}\n").as_bytes());
        }
        return Ok(());
    }
    Err(Failure::message(format!(
        "Unknown config subcommand: {cmd}"
    )))
}

#[cfg(test)]
pub(super) mod tests {
    use crate::config_tree::tree::test_support::*;
    use std::fs;
    const BASIC: &str = "project: \"demo\"\nversion: \"1\"\n";
    fn refusal(content: &str, args: &[&str], expected: impl Fn(&str) -> String) {
        for mode in [0o644, 0o600] {
            let t = project(content);
            let file = config(t.path());
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&file, fs::Permissions::from_mode(mode)).unwrap();
            }
            #[cfg(not(unix))]
            let _ = mode;
            let before = fs::read(&file).unwrap();
            let permissions = fs::metadata(&file).unwrap().permissions();
            let out = invoke(t.path(), args);
            assert_eq!(out.exit_code, 1);
            assert!(out.stdout.is_empty());
            assert_eq!(
                out.stderr,
                format!("Error: {}\n", expected(&file.to_string_lossy())).as_bytes()
            );
            assert_eq!(fs::read(&file).unwrap(), before);
            assert_eq!(fs::metadata(&file).unwrap().permissions(), permissions);
            assert_eq!(fs::read_dir(file.parent().unwrap()).unwrap().count(), 1);
        }
    }
    #[test]
    fn a_second_document_is_refused_and_the_file_is_unchanged() {
        refusal(
            "project: \"demo\"\nversion: \"1\"\n---\nkeep: this\n",
            &["config", "set", "project", "other"],
            |path| {
                format!("Refusing to rewrite {path}: it holds more than one YAML document, and the Go tool would keep only the first. Nothing was written.")
            },
        );
    }
    #[test]
    fn mulch_source_beside_atlas_source_is_refused_and_the_file_is_unchanged() {
        refusal("project: \"demo\"\nversion: \"1\"\nplan_templates:\n  x:\n    sections:\n      context:\n        required: true\n        kind: text\n        prompt: explain\n        mulch_source: old\n        atlas_source: new\n",&["config","set","project","other"],|path|format!("Refusing to rewrite {path}: plan_templates.x.sections.context holds both mulch_source and atlas_source, and the Go tool would drop mulch_source. Nothing was written."));
    }
    #[test]
    fn a_value_that_reads_back_differently_is_refused_and_the_file_is_unchanged() {
        refusal(BASIC, &["config", "set", "project", "True"], |path| {
            format!("Refusing to write {path}: the value at project would be written as True, which reads back as a different value. Nothing was written.")
        });
    }
    #[test]
    fn a_key_that_reads_back_differently_is_refused_and_the_file_is_unchanged() {
        refusal(
            BASIC,
            &["config", "set", "plan_templates.null", "{sections: {}}"],
            |path| {
                format!("Refusing to write {path}: the key at plan_templates.null would be written as null, which reads back as a different value. Nothing was written.")
            },
        );
    }
    #[test]
    fn comments_are_dropped_as_go_drops_them() {
        let t = project("# comment\nproject: demo # comment\nversion: \"1\"\n");
        let out = invoke(t.path(), &["config", "set", "project", "other"]);
        assert_eq!(out.stdout, "✓ Set project = other\n".as_bytes());
        assert_eq!(
            fs::read_to_string(config(t.path())).unwrap(),
            "project: other\nversion: \"1\"\n"
        );
    }
    #[test]
    fn config_set_flow_values_go_through_the_alias_guard() {
        // Literal below 1 KiB; expansion is refused before values are built.
        const BOMB:&str="{a: &a [x,x,x,x,x,x,x,x,x], b: &b [*a,*a,*a,*a,*a,*a,*a,*a,*a], c: &c [*b,*b,*b,*b,*b,*b,*b,*b,*b], d: &d [*c,*c,*c,*c,*c,*c,*c,*c,*c], e: &e [*d,*d,*d,*d,*d,*d,*d,*d,*d], f: &f [*e,*e,*e,*e,*e,*e,*e,*e,*e], g: &g [*f,*f,*f,*f,*f,*f,*f,*f,*f], h: &h [*g,*g,*g,*g,*g,*g,*g,*g,*g], i: [*h,*h,*h,*h,*h,*h,*h,*h,*h]}";
        let t = project(BASIC);
        let out = invoke(t.path(), &["config", "set", "plan_templates", BOMB]);
        assert_eq!(
            out.stderr,
            b"Error: yaml: document contains excessive aliasing\n"
        );
        assert_eq!(out.exit_code, 1);
        assert_eq!(fs::read_to_string(config(t.path())).unwrap(), BASIC);
    }
    #[test]
    fn nested_leaves_reject_sibling_options() {
        let t = project(BASIC);
        for args in [
            &["config", "schema", "--path", "project"][..],
            &["config", "set", "project", "x", "--path", "project"],
            &["config", "unset", "project", "--path", "project"],
        ] {
            let out = invoke(t.path(), args);
            assert_eq!(out.stderr, b"error: unknown option '--path'\n");
            assert_eq!(out.exit_code, 1);
        }
        assert_eq!(fs::read_to_string(config(t.path())).unwrap(), BASIC);
    }
    #[test]
    fn nested_leaves_keep_their_own_options() {
        let t = project(BASIC);
        let out = invoke(t.path(), &["config", "show", "--path", "project", "--json"]);
        assert_eq!(out.stdout,b"{\n  \"success\": true,\n  \"command\": \"config show\",\n  \"path\": \"project\",\n  \"value\": \"demo\"\n}\n");
        assert_eq!(
            invoke(t.path(), &["config", "set", "project", "next", "--json"]).exit_code,
            0
        );
        assert_eq!(
            invoke(t.path(), &["config", "unset", "plan_templates", "--json"]).exit_code,
            0
        );
        assert_eq!(
            invoke(t.path(), &["config", "schema", "--json"]).stdout,
            [
                include_bytes!("../../../assets/s4-missions-lib-3/schema.json").as_slice(),
                b"\n"
            ]
            .concat()
        );
    }
    #[test]
    fn config_set_and_unset_leave_the_parent_of_the_temporary_project_unchanged() {
        let parent = tempfile::tempdir().unwrap();
        let sentinel = parent.path().join("sentinel");
        fs::write(&sentinel, b"keep").unwrap();
        let permissions = fs::metadata(&sentinel).unwrap().permissions();
        let root = parent.path().join("project");
        store(&root, BASIC);
        assert_eq!(
            invoke(&root, &["config", "set", "project", "parity"]).exit_code,
            0
        );
        let out = invoke(&root, &["config", "unset", "project"]);
        assert_eq!(out.exit_code, 1);
        assert_eq!(
            out.stderr,
            b"Error: Config validation failed:\n  project: add a 'project' field\n"
        );
        assert_eq!(fs::read(&sentinel).unwrap(), b"keep");
        assert_eq!(fs::metadata(&sentinel).unwrap().permissions(), permissions);
        assert_eq!(fs::read_dir(parent.path()).unwrap().count(), 2);
        assert!(!parent.path().join(".luuna").exists());
    }
}
