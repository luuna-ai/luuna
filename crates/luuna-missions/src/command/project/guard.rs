//! Pure stdin/cwd hook decision; Go intentionally guards stores in any project.
use crate::{
    command::{Context, Failure},
    gopath,
    json::{self, Value},
};
use std::path::Path;
fn field<'a>(v: &'a Value, k: &str) -> Option<&'a Value> {
    if let Value::Object(m) = v {
        m.get(k)
    } else {
        None
    }
}
fn text(v: Option<&Value>) -> &str {
    if let Some(Value::String(s)) = v {
        s
    } else {
        ""
    }
}
pub(super) fn run(c: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    if !args.is_empty() {
        return Err(Failure::message("too many arguments for 'guard'"));
    }
    let Ok(input) = Value::decode(c.stdin()) else {
        return Ok(());
    };
    let tool = text(field(&input, "tool_name"));
    let key = match tool {
        "Write" | "Edit" | "MultiEdit" => "file_path",
        "NotebookEdit" => "notebook_path",
        _ => return Ok(()),
    };
    let target = text(field(&input, "tool_input").and_then(|v| field(v, key)));
    if target.is_empty() {
        return Ok(());
    }
    let cwd = text(field(&input, "cwd"));
    let cwd = if cwd.is_empty() {
        c.cwd.as_path()
    } else {
        Path::new(cwd)
    };
    let path = if Path::new(target).is_absolute() {
        gopath::clean(Path::new(target))
    } else {
        gopath::clean(&cwd.join(target))
    };
    let file = gopath::text(&gopath::base(&path));
    let dir = gopath::dir(&path);
    if gopath::base(&dir) != Path::new("missions")
        || gopath::base(&gopath::dir(&dir)) != Path::new(".luuna")
        || !file.ends_with(".jsonl")
    {
        return Ok(());
    }
    let advice=match file.as_str(){"issues.jsonl"=>"luuna-missions create / luuna-missions update / luuna-missions close / luuna-missions dep / luuna-missions label","plans.jsonl"=>"luuna-missions plan (submit / edit / adopt / reorder)","templates.jsonl"=>"luuna-missions tpl",_=>"the luuna-missions CLI"};
    let reason=format!("missions: do not edit .luuna/missions/{file} by hand; use {advice} (luuna-missions commands lock, validate, and dedup the store).");
    c.stdout.extend_from_slice(b"{\"hookSpecificOutput\":{\"hookEventName\":\"PreToolUse\",\"permissionDecision\":\"deny\",\"permissionDecisionReason\":");
    json::quote(&mut c.stdout, &reason, false);
    c.stdout.extend_from_slice(b"}}\n");
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{
        command::{run, Invocation},
        config_tree::tree::test_support::project,
    };
    #[test]
    fn denies_only_store_jsonl_files() {
        let t = project("project: demo\nversion: \"1\"\n");
        for (tool, path, deny) in [
            ("Write", ".luuna/missions/issues.jsonl", true),
            ("Edit", "other/.luuna/missions/unknown.jsonl", true),
            ("MultiEdit", ".luuna/missions/../missions/plans.jsonl", true),
            ("NotebookEdit", ".luuna/missions/templates.jsonl", true),
            ("Read", ".luuna/missions/issues.jsonl", false),
            ("Write", ".luuna/missions/config.yaml", false),
            ("Write", "issues.jsonl", false),
        ] {
            let key = if tool == "NotebookEdit" {
                "notebook_path"
            } else {
                "file_path"
            };
            let input = serde_json::json!({"tool_name":tool,"tool_input":{key:path}});
            let out = run(&Invocation::new(t.path(), ["guard"])
                .with_stdin(serde_json::to_vec(&input).unwrap()))
            .unwrap();
            assert_eq!(out.exit_code, 0);
            assert_eq!(!out.stdout.is_empty(), deny);
            if deny {
                let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
                assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny");
            }
        }
        for raw in [b"".as_slice(), b"not json", b"null"] {
            assert!(run(&Invocation::new(t.path(), ["guard"]).with_stdin(raw))
                .unwrap()
                .stdout
                .is_empty());
        }
    }
}
