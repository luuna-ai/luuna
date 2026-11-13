//! Go-backed A8/A10 refusals and successful controls.
//!
//! LUUNA_MISSIONS_REFERENCE=<reference-clone> LUUNA_MISSIONS_GO=<go>
//! LUUNA_MISSIONS_GOPROXY=<proxy list>
//! cargo test --locked -p luuna-missions --test refuse -- --ignored
mod support;
use std::{fs, path::PathBuf};
use support::reference::{self, GoTool};
use support::{
    args, child_env, exact_difference, guarded, run_differences, run_go, run_rust,
    tree_differences, Normaliser, TempProject,
};
const ATTRS:&[u8]=b".luuna/missions/issues.jsonl merge=union\n.luuna/missions/templates.jsonl merge=union\n.luuna/missions/plans.jsonl merge=union\n";
fn record(id: &str, extra: &[u8]) -> Vec<u8> {
    [format!(r#"{{"id":"p-{id}","title":"tkt-{}","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z""#,id.to_uppercase()).as_bytes(),extra,b"}\n"].concat()
}
fn content(extra_t: &[u8], extra_b: &[u8], tail: &[u8]) -> Vec<u8> {
    [
        record("t", extra_t),
        record("b", extra_b),
        record("c", b""),
        tail.to_vec(),
    ]
    .concat()
}
fn store(p: &TempProject, data: Vec<u8>) {
    p.write_files([
        (".gitattributes", ATTRS.to_vec()),
        (
            ".luuna/missions/config.yaml",
            b"project: \"p\"\nversion: \"1\"\nmax_plan_depth: 3\n".to_vec(),
        ),
        (".luuna/missions/.gitignore", b"*.lock\n".to_vec()),
        (".luuna/missions/issues.jsonl", data),
        (".luuna/missions/plans.jsonl", vec![]),
        (".luuna/missions/templates.jsonl", vec![]),
    ]);
}
fn field(key: &str, raw: &[u8]) -> Vec<u8> {
    [format!(",\"{key}\":").as_bytes(), raw].concat()
}
fn envelope(command: &str, text: &str) -> Vec<u8> {
    format!(
        "{{\n  \"success\": false,\n  \"command\": {} ,\n  \"error\": {}\n}}\n",
        serde_json::to_string(command).unwrap(),
        serde_json::to_string(text).unwrap()
    )
    .replace(" ,\n", ",\n")
    .into_bytes()
}
fn line_error(n: usize) -> String {
    format!("Refusing to rewrite issues.jsonl: line {n} is not a ticket record the Go tool can read, and rewriting the file would drop it. Nothing was written.")
}
fn list_error(n: usize, id: &str, key: &str, kind: char) -> String {
    let why=match kind{'d'=>format!("a \"{key}\" value that is not a list of strings, and changing the field would replace that value"),'e'=>format!("an element of \"{key}\" that is not a string, and changing the field would write it as an empty string"),'f'=>format!("an element of \"{key}\" with invalid UTF-8 or a lone surrogate escape, and changing the field would write it as U+FFFD"),_=>unreachable!()};
    format!("Refusing to rewrite issues.jsonl: line {n} (ticket \"{id}\") holds {why}. Nothing was written.")
}
fn extension_error(kind: char) -> String {
    let why=match kind{'g'=>"an \"extensions\" value that is not an object, and changing the field would replace that value",'h'=>"a key or string in \"extensions\" with invalid UTF-8 or a lone surrogate escape, and changing the field would write it as U+FFFD",'i'=>"the number 9007199254740993 in \"extensions\", and changing the field would write it as 9007199254740992",'j'=>"an object in \"extensions\" that repeats the key \"a\", and changing the field would keep only its last value",_=>unreachable!()};
    format!("Refusing to rewrite issues.jsonl: line 1 (ticket \"p-t\") holds {why}. Nothing was written.")
}
struct Refusal {
    name: String,
    data: Vec<u8>,
    command: Vec<String>,
    text: String,
    witness: Vec<u8>,
}
fn file_cases(command: &[String]) -> Vec<Refusal> {
    let mut out = vec![];
    for (name, tail) in [
        ("a-not-json", b"{not json\n".as_slice()),
        ("a-array", b"[1]\n"),
        ("a-null", b"null\n"),
        (
            "a-1e999-other",
            b"{\"id\":\"extra\",\"extensions\":{\"n\":1e999}}\n",
        ),
    ] {
        out.push(Refusal {
            name: name.into(),
            data: content(b"", b"", tail),
            command: command.to_vec(),
            text: line_error(4),
            witness: tail.to_vec(),
        });
    }
    let base = record("c", b"");
    out.push(Refusal{name:"b-same-id".into(),data:content(b"",b"",b"{\"id\":\"p-c\",\"title\":\"newer\"}\n"),command:command.to_vec(),text:"Refusing to rewrite issues.jsonl: lines 3 and 4 both hold the ticket \"p-c\", and rewriting the file would keep only the last one. Nothing was written.".into(),witness:base});
    out.push(Refusal{name:"b-no-id".into(),data:content(b"",b"",b"{\"title\":\"x\"}\n{\"title\":\"y\"}\n"),command:command.to_vec(),text:"Refusing to rewrite issues.jsonl: lines 4 and 5 both hold a record without an id, and rewriting the file would keep only the last one. Nothing was written.".into(),witness:b"\"title\":\"x\"".to_vec()});
    for (name, own) in [("c-other", false), ("c-own", true)] {
        let mut data = if own {
            let t = record("t", b"");
            let text = String::from_utf8(t).unwrap().replace(
                "\"title\":\"tkt-T\"",
                "\"title\":\"first\",\"title\":\"tkt-T\"",
            );
            [text.into_bytes(), record("b", b""), record("c", b"")].concat()
        } else {
            [
                record("t", b""),
                record("b", b""),
                b"{\"id\":\"p-c\",\"title\":\"first\",\"title\":\"second\"}\n".to_vec(),
            ]
            .concat()
        };
        let n = if own { 1 } else { 3 };
        out.push(Refusal{name:name.into(),data:std::mem::take(&mut data),command:command.to_vec(),text:format!("Refusing to rewrite issues.jsonl: the record on line {n} repeats the key \"title\", and rewriting the file would keep only its last value. Nothing was written."),witness:b"\"title\":\"first\"".to_vec()});
    }
    out
}
fn compare_refusal(go: &GoTool, case: &Refusal) -> Vec<String> {
    let gp = TempProject::new();
    let rp = TempProject::new();
    store(&gp, case.data.clone());
    store(&rp, case.data.clone());
    let before = rp.snapshot();
    let env = child_env(None);
    let g = run_go(go.binary(), gp.root(), &case.command, b"", &env).unwrap();
    assert_eq!(g.exit, 0, "{}: {:?}", case.name, g);
    let after = fs::read(gp.root().join(".luuna/missions/issues.jsonl")).unwrap();
    assert!(
        !after.windows(case.witness.len()).any(|s| s == case.witness),
        "{}: Go did not lose the designated bytes",
        case.name
    );
    let r = run_rust(rp.root(), &case.command, b"", &env).unwrap();
    let mut diff = vec![];
    if r.exit != 1 {
        diff.push(format!("Rust exit {}, want1", r.exit));
    }
    diff.extend(exact_difference(
        "stdout",
        &envelope(&case.command[0], &case.text),
        &r.stdout,
    ));
    diff.extend(exact_difference("stderr", b"", &r.stderr));
    if rp.snapshot() != before {
        diff.push("Rust changed project bytes, modes, links or entries".into());
    }
    diff
}
#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn update_refusals_where_go_loses_data() {
    let clone = reference::reference_from_env();
    guarded("update_refusals_where_go_loses_data", &clone, |report| {
        let go = reference::go_tool(&clone);
        report.record_go(go);
        let mut cases = file_cases(&args(&["update", "p-t", "--title=Changed", "--json"]));
        for (name, raw, kind) in [
            ("d-labels-string", br#""urgent""#.as_slice(), 'd'),
            ("d-labels-number", b"5", 'd'),
            ("d-labels-object", br#"{"x":1}"#, 'd'),
            ("e-labels-number", br#"["a",5]"#, 'e'),
            ("e-labels-object", br#"["a",{}]"#, 'e'),
            ("e-labels-null", br#"["a",null]"#, 'e'),
            ("f-labels-byte", b"[\"a\",\"\xff\"]", 'f'),
            ("f-labels-surrogate", br#"["a","\ud800"]"#, 'f'),
        ] {
            for remove in [false, true] {
                if remove && kind == 'd' {
                    continue;
                }
                let extra = field("labels", raw);
                cases.push(Refusal {
                    name: if kind == 'd' {
                        name.into()
                    } else {
                        format!("{name}-{}", if remove { "remove" } else { "add" })
                    },
                    data: content(&extra, b"", b""),
                    command: args(&[
                        "update",
                        "p-t",
                        if remove {
                            "--remove-label"
                        } else {
                            "--add-label"
                        },
                        if remove { "a" } else { "zz" },
                        "--json",
                    ]),
                    text: list_error(1, "p-t", "labels", kind),
                    witness: extra[1..].to_vec(),
                });
            }
        }
        for (name, raw, kind) in [
            ("g-string", br#""x""#.as_slice(), 'g'),
            ("g-array", b"[1,2]", 'g'),
            ("g-number", b"5", 'g'),
            ("h-value-byte", b"{\"a\":\"\xff\"}", 'h'),
            ("h-key-byte", b"{\"\xff\":1}", 'h'),
            ("h-value-surrogate", br#"{"a":"\ud800"}"#, 'h'),
            ("h-key-surrogate", br#"{"\ud800":1}"#, 'h'),
            ("i-top", br#"{"n":9007199254740993}"#, 'i'),
            ("i-nested", br#"{"o":{"n":9007199254740993}}"#, 'i'),
            ("j-top", br#"{"a":1,"a":2}"#, 'j'),
        ] {
            let extra = field("extensions", raw);
            cases.push(Refusal {
                name: name.into(),
                data: content(&extra, b"", b""),
                command: args(&["update", "p-t", "--extensions", r#"{"k":1}"#, "--json"]),
                text: extension_error(kind),
                witness: extra[1..].to_vec(),
            });
        }
        for case in cases {
            report.case(
                &case.name,
                serde_json::Map::new(),
                compare_refusal(go, &case),
            );
        }
    });
}
#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn block_refusals_where_go_loses_data() {
    let clone = reference::reference_from_env();
    guarded("block_refusals_where_go_loses_data", &clone, |report| {
        let go = reference::go_tool(&clone);
        report.record_go(go);
        let cmd = args(&["block", "p-t", "--by", "p-b", "--json"]);
        let mut cases = file_cases(&cmd);
        for (side, key, n, id) in [
            ("by", "blockedBy", 1, "p-t"),
            ("blocks", "blocks", 2, "p-b"),
        ] {
            for (name, raw, kind) in [
                ("number", b"5".as_slice(), 'd'),
                ("object", br#"{"a":1}"#, 'd'),
                ("string", br#""x""#, 'd'),
                ("number", br#"["p-c",5]"#, 'e'),
                ("null", br#"["p-c",null]"#, 'e'),
                ("byte", b"[\"p-c\",\"\xff\"]", 'f'),
                ("surrogate", br#"["p-c","\ud800"]"#, 'f'),
            ] {
                let extra = field(key, raw);
                let (t, b) = if n == 1 {
                    (extra.as_slice(), b"".as_slice())
                } else {
                    (b"".as_slice(), extra.as_slice())
                };
                cases.push(Refusal {
                    name: format!("{kind}-{side}-{name}"),
                    data: content(t, b, b""),
                    command: cmd.clone(),
                    text: list_error(n, id, key, kind),
                    witness: extra[1..].to_vec(),
                });
            }
        }
        for case in cases {
            report.case(
                &case.name,
                serde_json::Map::new(),
                compare_refusal(go, &case),
            );
        }
    });
}
fn init_setup(p: &TempProject, case: &str) -> (Option<TempProject>, PathBuf) {
    let mut outside = None;
    let mut loss = p.root().join(".luuna/missions");
    fs::create_dir_all(&loss).unwrap();
    match case {
        "issues" | "templates" | "plans" => {
            loss.push(format!("{case}.jsonl"));
            fs::write(&loss, b"{\"id\":\"x\",\"title\":\"keep\"}\n").unwrap();
        }
        "gitignore" => {
            loss.push(".gitignore");
            fs::write(&loss, b"*.lock\nmine\n").unwrap();
        }
        _ => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::symlink;
                let ext = TempProject::new();
                let path = ext.root().join("kept");
                if case == "linked-folder" {
                    fs::remove_dir(&loss).unwrap();
                    ext.write_files([("issues.jsonl", b"{\"id\":\"x\"}\n".to_vec())]);
                    symlink(ext.root(), &loss).unwrap();
                    loss = ext.root().join("issues.jsonl");
                } else {
                    fs::write(
                        &path,
                        if case == "linked-issues" {
                            b"{\"id\":\"x\"}\n".as_slice()
                        } else {
                            b"mine\n"
                        },
                    )
                    .unwrap();
                    let name = if case == "linked-issues" {
                        "issues.jsonl"
                    } else {
                        ".gitignore"
                    };
                    symlink(&path, loss.join(name)).unwrap();
                    loss = path;
                }
                outside = Some(ext);
            }
            #[cfg(not(unix))]
            panic!("symlink case is Unix-only");
        }
    }
    (outside, loss)
}
#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn init_refusals_where_go_loses_data() {
    let clone = reference::reference_from_env();
    guarded("init_refusals_where_go_loses_data", &clone, |report| {
        let go = reference::go_tool(&clone);
        report.record_go(go);
        let mut cases = vec!["issues", "templates", "plans", "gitignore"];
        #[cfg(unix)]
        cases.extend(["linked-issues", "linked-gitignore", "linked-folder"]);
        for case in cases {
            let gp = TempProject::new();
            let rp = TempProject::new();
            let (_gx, go_loss) = init_setup(&gp, case);
            let (rx, _) = init_setup(&rp, case);
            let before = rp.snapshot();
            let outside = rx.as_ref().map(TempProject::snapshot);
            let cmd = args(&["init", "--json"]);
            let g = run_go(go.binary(), gp.root(), &cmd, b"", &child_env(None)).unwrap();
            assert_eq!(g.exit, 0);
            let gitignore = case.contains("gitignore");
            assert_eq!(
                fs::read(go_loss).unwrap(),
                if gitignore {
                    b"*.lock\n".to_vec()
                } else {
                    vec![]
                }
            );
            let name = match case {
                "linked-issues" | "linked-folder" => "issues.jsonl",
                "linked-gitignore" => ".gitignore",
                _ if gitignore => ".gitignore",
                _ => match case {
                    "issues" => "issues.jsonl",
                    "templates" => "templates.jsonl",
                    _ => "plans.jsonl",
                },
            };
            let text = if gitignore {
                format!("Refusing to initialize: line {} of .luuna/missions/.gitignore holds an entry other than *.lock, and initializing would replace the file with *.lock alone. Nothing was written.",if case=="gitignore"{2}else{1})
            } else {
                format!("Refusing to initialize: line 1 of .luuna/missions/{name} holds data and config.yaml is missing, and initializing would empty the file. Nothing was written.")
            };
            let r = run_rust(rp.root(), &cmd, b"", &child_env(None)).unwrap();
            let mut diff = vec![];
            if r.exit != 1 {
                diff.push(format!("exit {}, want1", r.exit))
            }
            diff.extend(exact_difference(
                "stdout",
                &envelope("init", &text),
                &r.stdout,
            ));
            diff.extend(exact_difference("stderr", b"", &r.stderr));
            if rp.snapshot() != before || rx.as_ref().map(TempProject::snapshot) != outside {
                diff.push("Rust changed project or linked target".into());
            }
            report.case(case, serde_json::Map::new(), diff);
        }
    });
}
fn copied(go: &GoTool, data: Vec<u8>, command: Vec<String>) -> Vec<String> {
    let expected_exit = if data.windows(5).any(|w| w == b"1e999") {
        1
    } else {
        0
    };
    let gp = TempProject::new();
    let rp = TempProject::new();
    store(&gp, data.clone());
    store(&rp, data);
    let g = run_go(go.binary(), gp.root(), &command, b"", &child_env(None)).unwrap();
    assert_eq!(g.exit, expected_exit, "Go control {command:?}");
    let r = run_rust(rp.root(), &command, b"", &child_env(None)).unwrap();
    let gn = Normaliser::new().replace(gp.root().to_string_lossy().as_bytes(), "<PROJECT>");
    let rn = Normaliser::new().replace(rp.root().to_string_lossy().as_bytes(), "<PROJECT>");
    let mut diff = run_differences(&g, &gn, &r, &rn);
    diff.extend(tree_differences(&gp.snapshot(), &gn, &rp.snapshot(), &rn));
    diff
}
fn init_copied_setup(p: &TempProject, case: &str) -> Option<TempProject> {
    match case {
        "empty" => (),
        "three-empty" => p.write_files([
            (".luuna/missions/issues.jsonl", vec![]),
            (".luuna/missions/templates.jsonl", vec![]),
            (".luuna/missions/plans.jsonl", vec![]),
        ]),
        "whitespace" => p.write_files([(".luuna/missions/issues.jsonl", b"\n  \n".to_vec())]),
        "gitignore" => p.write_files([(".luuna/missions/.gitignore", b"*.lock\n".to_vec())]),
        "valid" => p.write_files([
            (
                ".luuna/missions/config.yaml",
                b"project: p\nversion: \"1\"\n".to_vec(),
            ),
            (".luuna/missions/issues.jsonl", b"kept\n".to_vec()),
        ]),
        "attrs" => p.write_files([(".gitattributes", b"user attrs no newline".to_vec())]),
        _ => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::symlink;
                let x = TempProject::new();
                if case == "linked-config" {
                    x.write_files([("config.yaml", b"project: p\nversion: \"1\"\n".to_vec())]);
                    p.write_files([(".luuna/missions/issues.jsonl", b"kept\n".to_vec())]);
                    symlink(
                        x.root().join("config.yaml"),
                        p.root().join(".luuna/missions/config.yaml"),
                    )
                    .unwrap();
                } else {
                    x.write_files([("attrs", b"outside attrs\n".to_vec())]);
                    symlink(x.root().join("attrs"), p.root().join(".gitattributes")).unwrap();
                }
                return Some(x);
            }
            #[cfg(not(unix))]
            panic!("Unix-only symlink");
        }
    }
    None
}
// The frozen harness compares link targets literally. Bind only this case's
// known outside project; the original links are also asserted unchanged.
fn bound_links(
    mut tree: std::collections::BTreeMap<String, support::Entry>,
    outside: Option<&TempProject>,
) -> std::collections::BTreeMap<String, support::Entry> {
    if let Some(outside) = outside {
        for entry in tree.values_mut() {
            if let support::EntryKind::Symlink(target) = &mut entry.kind {
                if let Ok(rest) = target.strip_prefix(outside.root()) {
                    *target = PathBuf::from("<OUTSIDE>").join(rest);
                }
            }
        }
    }
    tree
}
fn link_targets(p: &TempProject) -> Vec<Option<PathBuf>> {
    [".luuna/missions/config.yaml", ".gitattributes"]
        .into_iter()
        .map(|name| fs::read_link(p.root().join(name)).ok())
        .collect()
}
#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn copied_cases_match_go() {
    let clone = reference::reference_from_env();
    guarded("copied_cases_match_go", &clone, |report| {
        let go = reference::go_tool(&clone);
        report.record_go(go);
        for (name, raw, flag, value) in [
            ("labels-null", b"null".as_slice(), "--add-label", "zz"),
            ("labels-duplicate", br#"["a","a"]"#, "--add-label", "zz"),
            ("remove-unchanged", br#"["a",5]"#, "--remove-label", "nope"),
            ("title-keeps-labels", br#"["a",5]"#, "--title", "Changed"),
        ] {
            report.case(
                name,
                serde_json::Map::new(),
                copied(
                    go,
                    content(&field("labels", raw), b"", b""),
                    args(&["update", "p-t", flag, value, "--json"]),
                ),
            );
        }
        for (name, raw, title) in [
            (
                "title-keeps-precision",
                br#"{"n":9007199254740993}"#.as_slice(),
                true,
            ),
            ("null", b"null", false),
            ("null-value", br#"{"a":null}"#, false),
            ("float", br#"{"n":1.50}"#, false),
            ("exponent", br#"{"n":1e21}"#, false),
            ("html", br#"{"a":"<&>"}"#, false),
            ("key-order", br#"{"b":1,"a":2}"#, false),
            ("update-target-1e999", br#"{"n":1e999}"#, false),
        ] {
            report.case(
                name,
                serde_json::Map::new(),
                copied(
                    go,
                    content(&field("extensions", raw), b"", b""),
                    args(&[
                        "update",
                        "p-t",
                        if title { "--title" } else { "--extensions" },
                        if title { "Changed" } else { r#"{"k":1}"# },
                        "--json",
                    ]),
                ),
            );
        }
        for (name, tail) in [
            ("one-no-id", b"{\"title\":\"one\"}\n".as_slice()),
            ("numeric-id", b"{\"id\":5,\"title\":\"numeric\"}\n"),
            (
                "spaced",
                b" { \"id\" : \"extra\", \"title\" : \"spaced\" } \n",
            ),
            ("whitespace", b"\n  \n"),
            ("unchanged-byte", b"{\"id\":\"extra\",\"title\":\"\xff\"}\n"),
        ] {
            for cmd in [
                args(&["update", "p-t", "--title=Changed", "--json"]),
                args(&["block", "p-t", "--by", "p-b", "--json"]),
            ] {
                report.case(
                    &format!("{name}-{}", cmd[0]),
                    serde_json::Map::new(),
                    copied(go, content(b"", b"", tail), cmd),
                );
            }
        }
        for (name, raw) in [
            ("null", b"null".as_slice()),
            ("already", br#"["p-b"]"#),
            ("duplicate", br#"["p-c","p-c"]"#),
        ] {
            report.case(
                &format!("block-{name}"),
                serde_json::Map::new(),
                copied(
                    go,
                    content(&field("blockedBy", raw), b"", b""),
                    args(&["block", "p-t", "--by", "p-b", "--json"]),
                ),
            );
        }
        let mut cases = vec![
            "empty",
            "three-empty",
            "whitespace",
            "gitignore",
            "valid",
            "attrs",
        ];
        #[cfg(unix)]
        cases.extend(["linked-config", "linked-attrs"]);
        for case in cases {
            let gp = TempProject::new();
            let rp = TempProject::new();
            let gx = init_copied_setup(&gp, case);
            let rx = init_copied_setup(&rp, case);
            let go_links = link_targets(&gp);
            let rust_links = link_targets(&rp);
            let command = args(&["init", "--json"]);
            let g = run_go(go.binary(), gp.root(), &command, b"", &child_env(None)).unwrap();
            assert_eq!(g.exit, 0, "Go init control {case}");
            let r = run_rust(rp.root(), &command, b"", &child_env(None)).unwrap();
            let mut gn =
                Normaliser::new().replace(gp.root().to_string_lossy().as_bytes(), "<PROJECT>");
            let mut rn =
                Normaliser::new().replace(rp.root().to_string_lossy().as_bytes(), "<PROJECT>");
            if let (Some(gx), Some(rx)) = (&gx, &rx) {
                gn = gn.replace(gx.root().to_string_lossy().as_bytes(), "<OUTSIDE>");
                rn = rn.replace(rx.root().to_string_lossy().as_bytes(), "<OUTSIDE>");
            }
            let mut diff = run_differences(&g, &gn, &r, &rn);
            assert_eq!(link_targets(&gp), go_links, "Go retained original links");
            assert_eq!(
                link_targets(&rp),
                rust_links,
                "Rust retained original links"
            );
            diff.extend(tree_differences(
                &bound_links(gp.snapshot(), gx.as_ref()),
                &gn,
                &bound_links(rp.snapshot(), rx.as_ref()),
                &rn,
            ));
            if let (Some(gx), Some(rx)) = (&gx, &rx) {
                diff.extend(tree_differences(&gx.snapshot(), &gn, &rx.snapshot(), &rn));
            }
            report.case(&format!("init-{case}"), serde_json::Map::new(), diff);
        }
    });
}
