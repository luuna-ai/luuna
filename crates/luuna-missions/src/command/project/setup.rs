//! Project-scoped Claude hooks; map encoding and acknowledged scalar normalization follow Go.
use crate::{
    command::{options, output::Envelope, Context, Failure},
    config,
    error::Error,
    gopath,
    json::{self, Value as V},
    json_error,
};
use std::{
    collections::{BTreeMap, HashSet},
    fs, io,
};
const SPECS: [(&str, &str, &str); 2] = [
    ("SessionStart", "", "luuna-missions prime --compact"),
    (
        "PreToolUse",
        "Write|Edit|MultiEdit|NotebookEdit",
        "luuna-missions guard",
    ),
];
fn object(v: &V) -> Option<&BTreeMap<String, V>> {
    if let V::Object(m) = v {
        Some(m)
    } else {
        None
    }
}
fn array(v: &V) -> Option<&[V]> {
    if let V::Array(a) = v {
        Some(a)
    } else {
        None
    }
}
fn space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c')
}
fn ours(v: &V) -> bool {
    let Some(V::String(command)) = object(v).and_then(|m| m.get("command")) else {
        return false;
    };
    let command = command.trim_start_matches(space);
    for name in ["luuna-missions", "lm", "sd", "seeds"] {
        let Some(rest) = command.strip_prefix(name) else {
            continue;
        };
        let after = rest.trim_start_matches(space);
        if after.len() == rest.len() {
            continue;
        }
        for verb in ["prime", "guard"] {
            if let Some(tail) = after.strip_prefix(verb) {
                if tail.is_empty() || tail.chars().next().is_some_and(space) {
                    return true;
                }
            }
        }
    }
    false
}
fn hooks(v: &V) -> &[V] {
    object(v)
        .and_then(|m| m.get("hooks"))
        .and_then(array)
        .unwrap_or_default()
}
fn group(matcher: &str, command: &str) -> V {
    let mut m = BTreeMap::from([(
        "hooks".into(),
        V::Array(vec![V::Object(BTreeMap::from([
            ("type".into(), V::String("command".into())),
            ("command".into(), V::String(command.into())),
        ]))]),
    )]);
    if !matcher.is_empty() {
        m.insert("matcher".into(), V::String(matcher.into()));
    }
    V::Object(m)
}
fn sync(
    settings: &BTreeMap<String, V>,
    remove: bool,
) -> Result<(BTreeMap<String, V>, bool), Failure> {
    let mut next = settings.clone();
    let mut h = match settings.get("hooks") {
        None => BTreeMap::new(),
        Some(v) => object(v).cloned().ok_or_else(|| {
            Failure::message("`hooks` in Claude settings is not an object; nothing was written")
        })?,
    };
    let mut changed = false;
    for (event, matcher, command) in SPECS {
        let current = match h.get(event) {
            None => vec![],
            Some(v) => array(v).map(<[V]>::to_vec).ok_or_else(|| {
                Failure::message(format!(
                    "`hooks.{event}` in Claude settings is not an array"
                ))
            })?,
        };
        let expected = group(matcher, command);
        let installed: Vec<_> = current
            .iter()
            .filter(|v| hooks(v).iter().any(ours))
            .collect();
        if !remove && installed.len() == 1 && installed[0] == &expected {
            continue;
        }
        let mut updated = vec![];
        for g in &current {
            let old = hooks(g);
            let kept: Vec<_> = old.iter().filter(|v| !ours(v)).cloned().collect();
            if old.len() == kept.len() {
                updated.push(g.clone());
            } else if !kept.is_empty() {
                let mut m = object(g).unwrap().clone();
                m.insert("hooks".into(), V::Array(kept));
                updated.push(V::Object(m));
            }
        }
        if !remove {
            updated.push(expected)
        }
        if updated == current {
            continue;
        }
        changed = true;
        if updated.is_empty() {
            h.remove(event);
        } else {
            h.insert(event.into(), V::Array(updated));
        }
    }
    if changed {
        if h.is_empty() {
            next.remove("hooks");
        } else {
            next.insert("hooks".into(), V::Object(h));
        }
    }
    Ok((next, changed))
}
// Iterative token walk: compare decoded keys independently in every object.
fn repeated(raw: &[u8]) -> Option<String> {
    let mut stack: Vec<Option<HashSet<String>>> = vec![];
    let mut i = 0;
    while i < raw.len() {
        match raw[i] {
            b'{' => stack.push(Some(HashSet::new())),
            b'[' => stack.push(None),
            b'}' | b']' => {
                stack.pop();
            }
            b'"' => {
                let start = i;
                i += 1;
                while i < raw.len() {
                    if raw[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if raw[i] == b'"' {
                        break;
                    }
                    i += 1;
                }
                let end = i + 1;
                let mut j = end;
                while raw.get(j).is_some_and(u8::is_ascii_whitespace) {
                    j += 1
                }
                if raw.get(j) == Some(&b':') {
                    if let Some(Some(keys)) = stack.last_mut() {
                        let key = json::unquote(&raw[start..end]);
                        if !keys.insert(key.clone()) {
                            return Some(key);
                        }
                    }
                }
            }
            _ => (),
        }
        i += 1;
    }
    None
}
pub(super) fn run(c: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    let o = options::parse(args, &[], &["remove", "json"])?;
    let target = o
        .positionals
        .first()
        .map(String::as_str)
        .unwrap_or_default();
    if target != "claude" {
        return Err(Failure::message(format!(
            "Unknown setup target {}; supported: claude",
            crate::config_tree::emit::quote(target)
        )));
    }
    let store = c.store()?;
    let path = config::project_root(store.dir()).join(".claude/settings.json");
    let file = gopath::text(&path);
    let raw = match fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => vec![],
        Err(e) => return Err(Error::io("open", &path, e).into()),
    };
    let settings = if crate::store::trim_space(&raw).is_empty() {
        BTreeMap::new()
    } else {
        if let Some(error) = json_error::unmarshal_error(&String::from_utf8_lossy(&raw)) {
            return Err(Failure::message(format!(
                "Cannot parse {file} ({error}); fix it by hand, nothing was written"
            )));
        }
        V::decode(&raw)
            .ok()
            .and_then(V::into_object)
            .ok_or_else(|| {
                Failure::message(format!("{file} is not a JSON object; nothing was written"))
            })?
    };
    let (result, changed) = sync(&settings, o.has("remove"))?;
    if changed {
        if let Some(key) = repeated(&raw) {
            return Err(Failure::message(format!("Refusing to rewrite {file}: it repeats the key \"{key}\"; fix it by hand, nothing was written")));
        }
        let mut compact = vec![];
        json::encode_map(&mut compact, &result, true)
            .map_err(|e| Failure::message(e.to_string()))?;
        let bytes = json::reformat(
            &compact,
            json::Format {
                indent: Some("  "),
                ..json::Format::MARSHAL
            },
        )
        .map_err(|e| Failure::message(e.to_string()))?;
        fs::create_dir_all(path.parent().unwrap())
            .map_err(|e| Error::io("mkdir", path.parent().unwrap(), e))?;
        let mut bytes = bytes;
        bytes.push(b'\n');
        fs::write(&path, bytes).map_err(|e| Error::io("open", &path, e))?;
    }
    let action = if o.has("remove") { "remove" } else { "install" };
    if o.has("json") {
        c.json(
            Envelope::new(true, "setup")
                .string("target", target)
                .string("action", action)
                .boolean("changed", changed)
                .string("file", &file),
        );
    } else {
        let msg = if !changed {
            format!("{file}: already up to date")
        } else if o.has("remove") {
            format!("Removed missions hooks from {file}")
        } else {
            format!("Installed missions hooks in {file}")
        };
        c.success(msg.as_bytes());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_tree::tree::test_support::{invoke, project, store};
    fn settings(root: &std::path::Path, raw: &[u8]) -> std::path::PathBuf {
        let path = root.join(".claude/settings.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, raw).unwrap();
        path
    }
    #[test]
    fn install_is_idempotent_and_remove_cleans_up() {
        let t = project("project: demo\nversion: \"1\"\n");
        let p=settings(t.path(),br#"{"other":{"a":1},"hooks":{"SessionStart":[{"hooks":[{"command":"echo keep"},{"command":"sd prime"}]}]}}"#);
        assert_eq!(invoke(t.path(), &["setup", "claude"]).exit_code, 0);
        let bytes = fs::read(&p).unwrap();
        let out = invoke(t.path(), &["setup", "claude", "--json"]);
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["changed"], false);
        assert_eq!(fs::read(&p).unwrap(), bytes);
        assert_eq!(
            invoke(t.path(), &["setup", "claude", "--remove"]).exit_code,
            0
        );
        assert_eq!(fs::read(p).unwrap(),b"{\n  \"hooks\": {\n    \"SessionStart\": [\n      {\n        \"hooks\": [\n          {\n            \"command\": \"echo keep\"\n          }\n        ]\n      }\n    ]\n  },\n  \"other\": {\n    \"a\": 1\n  }\n}\n");
    }
    #[test]
    fn unparsable_and_non_object_settings_are_refused() {
        let t = project("project: demo\nversion: \"1\"\n");
        for raw in [
            b"{".as_slice(),
            b"null",
            b"[]",
            br#"{"hooks":null}"#,
            br#"{"hooks":{"SessionStart":null}}"#,
        ] {
            let p = settings(t.path(), raw);
            let out = invoke(t.path(), &["setup", "claude"]);
            assert_eq!(out.exit_code, 1);
            assert_eq!(fs::read(p).unwrap(), raw);
        }
    }
    #[test]
    fn setup_refuses_a_repeated_key_and_the_file_is_unchanged() {
        let t = project("project: demo\nversion: \"1\"\n");
        for raw in [
            br#"{"a":1,"a":2}"#.as_slice(),
            br#"{"n":9007199254740993,"x":[{"a":1,"a":2}]}"#,
        ] {
            let p = settings(t.path(), raw);
            let mode = fs::metadata(&p).unwrap().permissions();
            let out = invoke(t.path(), &["setup", "claude"]);
            assert_eq!(out.exit_code, 1);
            assert_eq!(out.stderr,format!("Error: Refusing to rewrite {}: it repeats the key \"a\"; fix it by hand, nothing was written\n",p.display()).as_bytes());
            assert_eq!(fs::read(&p).unwrap(), raw);
            assert_eq!(fs::metadata(&p).unwrap().permissions(), mode);
        }
        let p = settings(t.path(), br#"{"a":1,"a":2}"#);
        assert_eq!(
            invoke(t.path(), &["setup", "claude", "--remove"]).exit_code,
            0
        );
        assert_eq!(fs::read(p).unwrap(), br#"{"a":1,"a":2}"#);
    }
    #[test]
    fn setup_writes_large_numbers_through_float64_as_go_does() {
        let t = project("project: demo\nversion: \"1\"\n");
        let p = settings(t.path(), br#"{"n":9007199254740993}"#);
        assert_eq!(invoke(t.path(), &["setup", "claude"]).exit_code, 0);
        assert!(String::from_utf8(fs::read(&p).unwrap())
            .unwrap()
            .contains("\"n\": 9007199254740992"));
        assert_eq!(
            invoke(t.path(), &["setup", "claude", "--remove"]).exit_code,
            0
        );
        assert_eq!(fs::read(p).unwrap(), b"{\n  \"n\": 9007199254740992\n}\n");
    }
    #[test]
    fn setup_writes_invalid_utf8_and_unpaired_surrogates_as_u_fffd_as_go_does() {
        let t = project("project: demo\nversion: \"1\"\n");
        for (raw, expected) in [
            (b"{\"x\":\"\xff\"}".as_slice(), "{\n  \"x\": \"�\"\n}\n"),
            (br#"{"x":"\ud800"}"#, "{\n  \"x\": \"�\"\n}\n"),
            (br#"{"\ud800":"ok"}"#, "{\n  \"�\": \"ok\"\n}\n"),
        ] {
            let p = settings(t.path(), raw);
            assert_eq!(invoke(t.path(), &["setup", "claude"]).exit_code, 0);
            let first = fs::read(&p).unwrap();
            let out = invoke(t.path(), &["setup", "claude", "--json"]);
            let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
            assert_eq!(v["changed"], false);
            assert_eq!(fs::read(&p).unwrap(), first);
            assert_eq!(
                invoke(t.path(), &["setup", "claude", "--remove"]).exit_code,
                0
            );
            assert_eq!(fs::read(p).unwrap(), expected.as_bytes());
        }
    }
    #[test]
    fn setup_leaves_the_parent_of_the_temporary_project_unchanged() {
        let p = tempfile::tempdir().unwrap();
        let sentinel = p.path().join("sentinel");
        fs::write(&sentinel, b"parent").unwrap();
        let parent = settings(p.path(), b"{\"parent\":true}\n");
        let before = [
            (
                &sentinel,
                fs::read(&sentinel).unwrap(),
                fs::metadata(&sentinel).unwrap().permissions(),
            ),
            (
                &parent,
                fs::read(&parent).unwrap(),
                fs::metadata(&parent).unwrap().permissions(),
            ),
        ];
        let root = p.path().join("project");
        store(&root, "project: demo\nversion: \"1\"\n");
        for args in [vec!["setup", "claude"], vec!["setup", "claude", "--remove"]] {
            assert_eq!(invoke(&root, &args).exit_code, 0);
        }
        assert!(root.join(".claude/settings.json").is_file());
        for (path, bytes, mode) in before {
            assert_eq!(fs::read(path).unwrap(), bytes);
            assert_eq!(fs::metadata(path).unwrap().permissions(), mode);
        }
        assert_eq!(fs::read_dir(p.path()).unwrap().count(), 3);
    }
}
