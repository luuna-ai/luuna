//! Hook-safe drift packets and read-only integrity verification commands.

use crate::registry::CommandContext;
use roots_core::args::flag_string;
use roots_core::claims::read_claim;
use roots_core::config::load_config;
use roots_core::drift::{
    DRIFT_TIER, DriftCommit, DriftHit, DriftMatchInput, drift_ideas, gather_drift, idea_file,
    match_drift,
};
use roots_core::errors::{RootsError, RootsResult};
use roots_core::graph::read_graph;
use roots_core::integrations::read_missions_issues;
use roots_core::paths::{find_project_root, require_roots_paths, roots_paths};
use roots_core::prose::scan_node_dirs;
use roots_core::serde_json::{Map, Value, to_value};
use roots_core::verify::{
    GitProbe, Severity, VERIFY_CHECKS, VerifyIssue, load_verify_context, run_verify,
};

const DRIFT_INSTRUCTION: &str = "For each idea below, check whether the change still fits it. If it may not, ask the human one specific question: luuna-roots ask <id> \"<question>?\" --as agent:<model>. If the change clearly fits, do nothing. Never edit .luuna/roots/human/.";
const SHOWN_FILES: usize = 20;

enum DriftOutcome {
    Skipped { skipped: String, tier: Option<u8> },
    Packet(DriftPacket),
}

struct DriftIdea {
    id: String,
    slug: String,
    status: String,
    statement: String,
    path: Option<String>,
    reasons: Vec<roots_core::drift::DriftReason>,
}

struct DriftPacket {
    rev: String,
    files: Vec<String>,
    truncated: bool,
    commits: Vec<DriftCommit>,
    ideas: Vec<DriftIdea>,
    excluded: Vec<String>,
}

fn json<T: roots_core::serde::Serialize>(value: &T) -> RootsResult<Value> {
    to_value(value).map_err(|error| RootsError::error(error.to_string()))
}

fn drift_packet(
    root: &std::path::Path,
    environment: &roots_core::actor::Environment,
    rev: &str,
) -> RootsResult<DriftOutcome> {
    let paths = roots_paths(root);
    let config = load_config(&paths)?;
    if config.tier < DRIFT_TIER {
        return Ok(DriftOutcome::Skipped {
            skipped: "tier".to_owned(),
            tier: Some(config.tier),
        });
    }
    let gathered = gather_drift(root, environment, rev);
    let roots_core::drift::DriftGit::Available {
        files,
        truncated,
        commits,
        ..
    } = gathered
    else {
        let roots_core::drift::DriftGit::Unavailable { reason, .. } = gathered else {
            unreachable!("drift result is exhaustive")
        };
        return Ok(DriftOutcome::Skipped {
            skipped: format!("git: {reason}"),
            tier: None,
        });
    };
    let graph = read_graph(&paths)?;
    let dirs = scan_node_dirs(&paths)?;
    let ideas = drift_ideas(&paths, &graph, &dirs)?;
    let matched = match_drift(&DriftMatchInput {
        graph: &graph,
        config: &config,
        ideas: &ideas,
        files: &files,
        commits: &commits,
        issues: &read_missions_issues(root),
    });
    let ideas = matched
        .touched
        .into_iter()
        .map(|DriftHit { node, reasons }| {
            let claim = read_claim(&paths, &graph, &node)?;
            Ok(DriftIdea {
                id: node.id.clone(),
                slug: node.slug.clone(),
                status: node.status.as_str().to_owned(),
                statement: claim.statement,
                path: idea_file(&paths, &node, &dirs)?,
                reasons,
            })
        })
        .collect::<RootsResult<Vec<_>>>()?;
    Ok(DriftOutcome::Packet(DriftPacket {
        rev: rev.to_owned(),
        files,
        truncated,
        commits,
        ideas,
        excluded: matched.excluded,
    }))
}

fn compute_drift(context: &CommandContext<'_>, rev: &str) -> DriftOutcome {
    let Some(root) = find_project_root(context.io.cwd()) else {
        return DriftOutcome::Skipped {
            skipped: "not a roots project".to_owned(),
            tier: None,
        };
    };
    match drift_packet(&root, &context.io.environment(), rev) {
        Ok(outcome) => outcome,
        Err(error) => DriftOutcome::Skipped {
            skipped: format!("error: {}", error.message),
            tier: None,
        },
    }
}

fn reason_json(reason: &roots_core::drift::DriftReason) -> Value {
    json(reason).expect("drift reason serializes")
}

fn packet_json(packet: &DriftPacket) -> Map<String, Value> {
    let mut result = Map::new();
    result.insert("skipped".to_owned(), Value::Null);
    result.insert("rev".to_owned(), Value::String(packet.rev.clone()));
    result.insert(
        "files".to_owned(),
        Value::Array(packet.files.iter().cloned().map(Value::String).collect()),
    );
    result.insert("truncated".to_owned(), Value::Bool(packet.truncated));
    result.insert(
        "commits".to_owned(),
        Value::Array(
            packet
                .commits
                .iter()
                .map(|commit| {
                    let mut value = Map::new();
                    value.insert("sha".to_owned(), Value::String(commit.sha.clone()));
                    value.insert("subject".to_owned(), Value::String(commit.subject.clone()));
                    Value::Object(value)
                })
                .collect(),
        ),
    );
    result.insert(
        "ideas".to_owned(),
        Value::Array(
            packet
                .ideas
                .iter()
                .map(|idea| {
                    let mut value = Map::new();
                    value.insert("id".to_owned(), Value::String(idea.id.clone()));
                    value.insert("slug".to_owned(), Value::String(idea.slug.clone()));
                    value.insert("status".to_owned(), Value::String(idea.status.clone()));
                    value.insert(
                        "statement".to_owned(),
                        Value::String(idea.statement.clone()),
                    );
                    value.insert(
                        "path".to_owned(),
                        idea.path.clone().map_or(Value::Null, Value::String),
                    );
                    value.insert(
                        "reasons".to_owned(),
                        Value::Array(idea.reasons.iter().map(reason_json).collect()),
                    );
                    Value::Object(value)
                })
                .collect(),
        ),
    );
    result.insert(
        "excluded".to_owned(),
        Value::Array(packet.excluded.iter().cloned().map(Value::String).collect()),
    );
    result.insert(
        "instruction".to_owned(),
        Value::String(DRIFT_INSTRUCTION.to_owned()),
    );
    result
}

fn skipped_json(skipped: String, tier: Option<u8>) -> Map<String, Value> {
    let mut result = Map::new();
    result.insert("skipped".to_owned(), Value::String(skipped));
    if let Some(tier) = tier {
        result.insert("tier".to_owned(), Value::from(tier));
    }
    result.insert("ideas".to_owned(), Value::Array(Vec::new()));
    result
}

fn idea_section(idea: &DriftIdea) -> Vec<String> {
    let mut lines = vec![
        String::new(),
        format!("## {} {} ({})", idea.id, idea.slug, idea.status),
        String::new(),
        format!(
            "> {}",
            if idea.statement.is_empty() {
                "(empty)"
            } else {
                &idea.statement
            }
        ),
        String::new(),
    ];
    if let Some(path) = &idea.path {
        lines.push(format!("- file: {path} (human-owned: never edit it)"));
    }
    lines.extend(
        idea.reasons
            .iter()
            .map(|reason| format!("- {}: {}", reason.kind.as_str(), reason.detail)),
    );
    lines
}

fn render_drift_markdown(packet: &DriftPacket) -> String {
    let count = packet.ideas.len();
    let shown = packet.files.iter().take(SHOWN_FILES).collect::<Vec<_>>();
    let more = packet.files.len() - shown.len() + usize::from(packet.truncated);
    let mut lines = vec![
        format!(
            "# luuna-roots drift: {count} idea{} touched by changes since {}",
            if count == 1 { "" } else { "s" },
            packet.rev
        ),
        String::new(),
        DRIFT_INSTRUCTION.to_owned(),
        String::new(),
        format!(
            "Changed files ({}{}): {}{}",
            packet.files.len(),
            if packet.truncated { "+" } else { "" },
            shown
                .iter()
                .map(|file| file.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            if more > 0 { ", …" } else { "" }
        ),
    ];
    if !packet.commits.is_empty() {
        lines.push(format!(
            "Commits: {}",
            packet
                .commits
                .iter()
                .map(|commit| format!("{} {}", commit.sha, commit.subject))
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    for idea in &packet.ideas {
        lines.extend(idea_section(idea));
    }
    format!("{}\n", lines.join("\n"))
}

/// `luuna-roots drift [--diff <rev>]`: deliberately hook-safe and read-only.
pub fn drift_command(context: CommandContext<'_>) -> RootsResult<()> {
    let rev = flag_string(context.flags, "diff").unwrap_or("HEAD");
    match compute_drift(&context, rev) {
        DriftOutcome::Skipped { skipped, tier } => {
            context.out.result(context.io, skipped_json(skipped, tier));
        }
        DriftOutcome::Packet(packet) => {
            context.out.result(context.io, packet_json(&packet));
            if !context.out.json && !packet.ideas.is_empty() {
                context.io.stdout(&render_drift_markdown(&packet));
            }
        }
    }
    Ok(())
}

fn where_issue(issue: &VerifyIssue) -> String {
    match (&issue.file, issue.line) {
        (Some(file), Some(line)) => format!("{file}:{line}"),
        _ => String::new(),
    }
}

fn issue_line(context: &CommandContext<'_>, issue: &VerifyIssue) -> String {
    let mark = if issue.severity == Severity::Error {
        context.out.colors.red("✗")
    } else {
        context.out.colors.yellow("!")
    };
    let location = where_issue(issue);
    format!(
        "{mark} {}  {}{}",
        context.out.colors.bold(&issue.code),
        if location.is_empty() {
            String::new()
        } else {
            format!("{}  ", context.out.colors.dim(&location))
        },
        issue.message
    )
}

fn plural(count: usize, word: &str) -> String {
    format!("{count} {word}{}", if count == 1 { "" } else { "s" })
}

/// `luuna-roots verify`: the read-only persistent-data integrity audit.
pub fn verify_command(context: CommandContext<'_>) -> RootsResult<()> {
    let paths = require_roots_paths(context.io.cwd())?;
    let git = GitProbe::new(&paths.root, &context.io.environment());
    let report = run_verify(&load_verify_context(paths, git)?, VERIFY_CHECKS)?;
    let errors = report
        .issues
        .iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    let warnings = report
        .issues
        .iter()
        .filter(|issue| issue.severity == Severity::Warning)
        .collect::<Vec<_>>();
    let summary = format!(
        "{}, {}",
        plural(report.errors, "error"),
        plural(report.warnings, "warning")
    );
    if !context.out.json {
        let lines = errors
            .iter()
            .chain(warnings.iter())
            .map(|issue| issue_line(&context, issue))
            .collect::<Vec<_>>();
        context.out.lines(context.io, &lines);
    }
    if report.errors > 0 {
        let mut detail = Map::new();
        detail.insert("errors".to_owned(), Value::from(report.errors as u64));
        detail.insert("warnings".to_owned(), Value::from(report.warnings as u64));
        detail.insert("issues".to_owned(), json(&report.issues)?);
        return Err(RootsError::validation(format!("verify: {summary}")).with_detail(detail));
    }
    let mut result = Map::new();
    result.insert("ok".to_owned(), Value::Bool(true));
    result.insert("errors".to_owned(), Value::from(0));
    result.insert("warnings".to_owned(), Value::from(report.warnings as u64));
    result.insert("issues".to_owned(), json(&report.issues)?);
    context.out.result(context.io, result);
    if report.warnings > 0 {
        context.out.line(context.io, &format!("verify: {summary}"));
    } else {
        context.out.success(context.io, "verify: no problems found");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::MemoryIo;
    use crate::run_cli;
    use roots_core::config::{default_config, write_config};
    use roots_core::events::{EventFields, append_event, make_event};
    use roots_core::graph::graph_records;
    use roots_core::model::{
        EdgeRecord, EdgeRel, Graph, IdeaStatus, NodeKind, NodeRecord, NodeStatus,
    };
    use roots_core::paths::roots_paths;
    use roots_core::prose::content_hash;
    use roots_core::serde_json;
    use roots_core::store::write_jsonl_file;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    fn sandbox(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "roots-integrity-command-{name}-{}",
            roots_core::ids::random_hex(8)
        ));
        fs::create_dir_all(&root).expect("create sandbox");
        root
    }

    fn env() -> roots_core::actor::Environment {
        std::env::vars().collect()
    }

    fn git(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args([
                "-c",
                "user.name=Test Human",
                "-c",
                "user.email=test@example.invalid",
            ])
            .args(args)
            .current_dir(root)
            .env_clear()
            .envs(env())
            .output()
            .expect("start git");
        assert!(
            output.status.success(),
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn project(root: &Path, tier: u8) -> (roots_core::paths::RootsPaths, NodeRecord, PathBuf) {
        let paths = roots_paths(root);
        let mut config = default_config("test");
        config.tier = tier;
        write_config(&paths, &config).expect("write config");
        let node = NodeRecord::new(
            "r-a1b2".to_owned(),
            NodeKind::Idea,
            "offline-sync".to_owned(),
            NodeStatus::Idea(IdeaStatus::Planted),
            "human:test".to_owned(),
            "2026-01-01T00:00:00Z".to_owned(),
        );
        write_jsonl_file(
            &paths.graph,
            &graph_records(&Graph {
                nodes: vec![node.clone()],
                edges: Vec::new(),
            }),
        )
        .expect("write graph");
        let file = paths.human.join("a1b2-offline-sync").join("idea.md");
        fs::create_dir_all(file.parent().expect("idea parent")).expect("human tree");
        fs::write(&file, "Offline sync survives edits\n").expect("idea prose");
        (paths, node, file)
    }

    #[test]
    fn drift_uses_an_actual_git_fixture_and_stays_silent_without_hits() {
        let root = sandbox("drift-git");
        let (_paths, node, _file) = project(&root, 3);
        git(&root, &["init", "-q"]);
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-qm", "initial"]);
        let source = root.join("src/sync/offline.ts");
        fs::create_dir_all(source.parent().expect("source parent")).expect("source directory");
        fs::write(&source, "export const online = false;\n").expect("source file");
        let mut io = MemoryIo::new(&root);
        io.env = env();
        assert_eq!(
            run_cli(&["drift".to_owned(), "--json".to_owned()], &mut io),
            0
        );
        let body: Value = serde_json::from_str(&io.stdout).expect("drift JSON");
        assert_eq!(body["skipped"], Value::Null);
        assert_eq!(
            body["files"],
            Value::Array(vec![Value::String("src/sync/offline.ts".to_owned())])
        );
        assert_eq!(body["ideas"][0]["id"], node.id);
        assert_eq!(body["ideas"][0]["reasons"][0]["kind"], "slug");
        fs::remove_dir_all(root).expect("clean sandbox");
    }

    #[test]
    fn drift_is_hook_safe_in_text_and_explains_skips_in_json() {
        let outside = sandbox("drift-outside");
        let mut text = MemoryIo::new(&outside);
        text.env = env();
        assert_eq!(run_cli(&["drift".to_owned()], &mut text), 0);
        assert_eq!(text.stdout, "");
        assert_eq!(text.stderr, "");

        let mut outside_json = MemoryIo::new(&outside);
        outside_json.env = env();
        assert_eq!(
            run_cli(
                &["drift".to_owned(), "--json".to_owned()],
                &mut outside_json
            ),
            0
        );
        let outside_body: Value = serde_json::from_str(&outside_json.stdout).expect("outside JSON");
        assert_eq!(outside_body["skipped"], "not a roots project");
        assert_eq!(outside_body["ideas"], Value::Array(Vec::new()));

        let root = sandbox("drift-skips");
        let _ = project(&root, 2);
        let mut low = MemoryIo::new(&root);
        low.env = env();
        assert_eq!(run_cli(&["drift".to_owned()], &mut low), 0);
        assert_eq!(low.stdout, "");

        let mut low_json = MemoryIo::new(&root);
        low_json.env = env();
        assert_eq!(
            run_cli(&["drift".to_owned(), "--json".to_owned()], &mut low_json),
            0
        );
        let low_body: Value = serde_json::from_str(&low_json.stdout).expect("tier JSON");
        assert_eq!(low_body["skipped"], "tier");
        assert_eq!(low_body["tier"], 2);

        let _ = project(&root, 3);
        let mut no_git = MemoryIo::new(&root);
        no_git.env = env();
        assert_eq!(
            run_cli(&["drift".to_owned(), "--json".to_owned()], &mut no_git),
            0
        );
        let no_git_body: Value = serde_json::from_str(&no_git.stdout).expect("no git JSON");
        assert_eq!(no_git_body["skipped"], "git: not a git work tree");

        let mut bad_rev = MemoryIo::new(&root);
        bad_rev.env = env();
        assert_eq!(
            run_cli(
                &[
                    "drift".to_owned(),
                    "--json".to_owned(),
                    "--diff=--output=/tmp/x".to_owned(),
                ],
                &mut bad_rev,
            ),
            0
        );
        let bad_body: Value = serde_json::from_str(&bad_rev.stdout).expect("bad revision JSON");
        assert_eq!(
            bad_body["skipped"],
            "git: invalid revision \"--output=/tmp/x\""
        );

        fs::remove_dir_all(outside).expect("clean outside sandbox");
        fs::remove_dir_all(root).expect("clean project sandbox");
    }

    #[test]
    fn drift_does_not_treat_issue_labels_as_changed_file_mentions() {
        let root = sandbox("drift-labels");
        let (_paths, node, _file) = project(&root, 3);
        git(&root, &["init", "-q"]);
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-qm", "initial"]);
        let missions = root.join(".luuna/missions");
        fs::create_dir_all(&missions).expect("missions directory");
        fs::write(
            missions.join("issues.jsonl"),
            format!(
                "{{\"id\":\"m-1\",\"title\":\"Unrelated\",\"intent\":[\"{}\"],\"labels\":[\"src/misleading.ts\"]}}\n",
                node.id
            ),
        )
        .expect("write issue");
        let changed = root.join("src/misleading.ts");
        fs::create_dir_all(changed.parent().expect("changed parent")).expect("source directory");
        fs::write(changed, "export {};\n").expect("changed file");

        let mut io = MemoryIo::new(&root);
        io.env = env();
        assert_eq!(
            run_cli(&["drift".to_owned(), "--json".to_owned()], &mut io),
            0
        );
        let body: Value = serde_json::from_str(&io.stdout).expect("drift JSON");
        assert_eq!(body["ideas"], Value::Array(Vec::new()));
        fs::remove_dir_all(root).expect("clean sandbox");
    }

    #[test]
    fn verify_reports_malformed_store_and_human_tree_without_writing() {
        let root = sandbox("verify-boundary");
        let (paths, _node, _file) = project(&root, 2);
        fs::write(paths.human.join("stray.md"), "x").expect("stray file");
        fs::write(
            &paths.graph,
            "<<<<<< HEAD\n{\"type\":\"node\",\"id\":\"r-bad\"}\n",
        )
        .expect("damage graph");
        let before = fs::read_to_string(&paths.graph).expect("graph before");
        let mut io = MemoryIo::new(&root);
        io.env = env();
        assert_eq!(
            run_cli(&["verify".to_owned(), "--json".to_owned()], &mut io),
            6
        );
        let body: Value = serde_json::from_str(&io.stdout).expect("verify error JSON");
        let codes = body["issues"]
            .as_array()
            .expect("issues")
            .iter()
            .filter_map(|issue| issue["code"].as_str())
            .collect::<Vec<_>>();
        assert!(codes.contains(&"jsonl.parse"));
        assert!(codes.contains(&"record.invalid"));
        assert!(codes.contains(&"boundary.stray"));
        assert_eq!(
            fs::read_to_string(&paths.graph).expect("graph after"),
            before
        );
        fs::remove_dir_all(root).expect("clean sandbox");
    }

    #[test]
    fn verify_rejects_dirty_and_agent_hash_ledger_edits_in_a_git_project() {
        let root = sandbox("verify-ledger");
        let (paths, node, file) = project(&root, 2);
        let text = fs::read_to_string(&file).expect("idea text");
        let mut extra = Map::new();
        extra.insert("hash".to_owned(), Value::String(content_hash(&text)));
        append_event(
            &paths,
            &make_event(
                "plant",
                "human:test",
                EventFields {
                    node: Some(node.id.clone()),
                    extra,
                    ..Default::default()
                },
            ),
        )
        .expect("plant event");
        git(&root, &["init", "-q"]);
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-qm", "initial"]);
        fs::write(
            &file,
            "Offline sync survives edits\nchanged outside a session\n",
        )
        .expect("dirty idea");
        let mut io = MemoryIo::new(&root);
        io.env = env();
        assert_eq!(
            run_cli(&["verify".to_owned(), "--json".to_owned()], &mut io),
            6
        );
        let body: Value = serde_json::from_str(&io.stdout).expect("verify JSON");
        assert_eq!(body["issues"][0]["code"], "ledger.untrusted-edit");

        git(&root, &["commit", "-qam", "human edit"]);
        let mut human = MemoryIo::new(&root);
        human.env = env();
        assert_eq!(
            run_cli(&["verify".to_owned(), "--json".to_owned()], &mut human),
            0
        );
        let human_body: Value = serde_json::from_str(&human.stdout).expect("human verify JSON");
        assert_eq!(human_body["issues"], Value::Array(Vec::new()));

        fs::write(&file, "Offline sync survives edits\nagent edit\n").expect("agent idea");
        git(
            &root,
            &[
                "-c",
                "user.name=Claude",
                "-c",
                "user.email=noreply@anthropic.com",
                "commit",
                "-qam",
                "agent edit",
            ],
        );
        let mut agent = MemoryIo::new(&root);
        agent.env = env();
        assert_eq!(
            run_cli(&["verify".to_owned(), "--json".to_owned()], &mut agent),
            6
        );
        let agent_body: Value = serde_json::from_str(&agent.stdout).expect("agent verify JSON");
        assert_eq!(agent_body["issues"][0]["code"], "ledger.agent-commit");
        fs::remove_dir_all(root).expect("clean sandbox");
    }

    #[test]
    fn verify_checks_graph_invariants_and_human_asset_symlinks() {
        let root = sandbox("verify-graph-boundary");
        let (paths, node, idea) = project(&root, 2);
        write_jsonl_file(
            &paths.graph,
            &graph_records(&Graph {
                nodes: vec![node.clone()],
                edges: vec![EdgeRecord {
                    record_type: "edge".to_owned(),
                    id: "e-self".to_owned(),
                    from: node.id.clone(),
                    to: node.id.clone(),
                    rel: EdgeRel::Serves,
                    by: "human:test".to_owned(),
                    proposed_by: None,
                    proposal: None,
                    created_at: "2026-01-01T00:00:00Z".to_owned(),
                }],
            }),
        )
        .expect("write self edge");
        let assets = idea.parent().expect("idea directory").join("assets");
        fs::create_dir_all(&assets).expect("assets directory");
        let outside = root.join("outside.txt");
        fs::write(&outside, "outside").expect("outside target");
        let link = assets.join("outside-link");
        let linked = {
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(&outside, &link).is_ok()
            }
            #[cfg(windows)]
            {
                std::os::windows::fs::symlink_file(&outside, &link).is_ok()
            }
            #[cfg(not(any(unix, windows)))]
            {
                false
            }
        };

        let mut io = MemoryIo::new(&root);
        io.env = env();
        assert_eq!(
            run_cli(&["verify".to_owned(), "--json".to_owned()], &mut io),
            6
        );
        let body: Value = serde_json::from_str(&io.stdout).expect("verify JSON");
        let codes = body["issues"]
            .as_array()
            .expect("issues")
            .iter()
            .filter_map(|issue| issue["code"].as_str())
            .collect::<Vec<_>>();
        assert!(codes.contains(&"graph.invariant"));
        if linked {
            assert!(codes.contains(&"boundary.symlink"));
        }
        fs::remove_dir_all(root).expect("clean sandbox");
    }

    #[test]
    fn verify_warning_and_error_text_json_exit_contracts() {
        let root = sandbox("verify-output");
        let (paths, _node, _file) = project(&root, 2);

        let mut warning_text = MemoryIo::new(&root);
        warning_text.env = env();
        assert_eq!(run_cli(&["verify".to_owned()], &mut warning_text), 0);
        assert!(warning_text.stdout.contains("! ledger.missing"));
        assert!(warning_text.stdout.contains("verify: 0 errors, 1 warning"));
        assert_eq!(warning_text.stderr, "");

        let mut warning_json = MemoryIo::new(&root);
        warning_json.env = env();
        assert_eq!(
            run_cli(
                &["verify".to_owned(), "--json".to_owned()],
                &mut warning_json
            ),
            0
        );
        let warning_body: Value = serde_json::from_str(&warning_json.stdout).expect("warning JSON");
        assert_eq!(warning_body["success"], true);
        assert_eq!(warning_body["errors"], 0);
        assert_eq!(warning_body["warnings"], 1);

        fs::write(paths.human.join("stray.txt"), "x").expect("stray file");
        let mut error_text = MemoryIo::new(&root);
        error_text.env = env();
        assert_eq!(run_cli(&["verify".to_owned()], &mut error_text), 6);
        assert!(error_text.stdout.contains("✗ boundary.stray"));
        assert!(error_text.stdout.contains("! ledger.missing"));
        assert!(
            error_text
                .stderr
                .contains("error: verify: 1 error, 1 warning")
        );

        fs::remove_dir_all(root).expect("clean sandbox");
    }
}
