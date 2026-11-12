//! Read-only change-to-idea matching for `luuna-roots drift`.
//!
//! This mirrors the legacy tier-three Stop-hook packet: Git failures are data
//! (`DriftGit::Unavailable`), never command errors, and none of these helpers
//! write to a Roots store.

use crate::actor::Environment;
use crate::config::RootsConfig;
use crate::errors::RootsResult;
use crate::integrations::{MissionsIssue, cited_idea_ids, issue_links};
use crate::model::{EdgeRel, Graph, IdeaStatus, NodeKind, NodeRecord, NodeStatus};
use crate::notes::list_notes;
use crate::paths::RootsPaths;
use crate::prose::{NodeDirs, read_node_prose};
use crate::tier::effective_tier;
use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub const DRIFT_TIER: u8 = 3;
pub const MAX_FILES: usize = 500;
pub const MAX_COMMITS: usize = 50;
const MAX_NOTE_BYTES: u64 = 64 * 1024;
const GIT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_GIT_BYTES: usize = 16 * 1024 * 1024;
const STOP_WORDS: &[&str] = &[
    "the", "and", "for", "not", "with", "from", "into", "its", "our",
];
const TOOL_DIRS: &[&str] = &[".luuna", ".roots", ".seeds", ".mulch", ".canopy"];

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DriftCommit {
    pub sha: String,
    pub subject: String,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DriftGit {
    Available {
        rev: String,
        files: Vec<String>,
        truncated: bool,
        commits: Vec<DriftCommit>,
    },
    Unavailable {
        rev: String,
        reason: String,
    },
}

impl DriftGit {
    #[must_use]
    pub fn rev(&self) -> &str {
        match self {
            Self::Available { rev, .. } | Self::Unavailable { rev, .. } => rev,
        }
    }
}

/// Run Git with the caller's exact environment, bounded output, and a hard
/// timeout.  The TypeScript command supplies an `env` object to `spawnSync`,
/// which replaces rather than augments its process environment; `env_clear`
/// preserves that important test and hook behavior here.
fn git(root: &Path, env: &Environment, args: &[&str]) -> Option<String> {
    let mut child = Command::new("git")
        .args(args)
        .current_dir(root)
        .env_clear()
        .envs(env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    let reader = thread::spawn(move || -> std::io::Result<(Vec<u8>, bool)> {
        let mut handle = stdout;
        let mut buffer = [0_u8; 8192];
        let mut output = Vec::new();
        let mut too_large = false;
        loop {
            let read = handle.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            if output.len().saturating_add(read) <= MAX_GIT_BYTES {
                output.extend_from_slice(&buffer[..read]);
            } else {
                too_large = true;
            }
        }
        Ok((output, too_large))
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait().ok()? {
            Some(status) => break status,
            None if started.elapsed() >= GIT_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return None;
            }
            None => thread::sleep(Duration::from_millis(10)),
        }
    };
    let (output, too_large) = reader.join().ok()?.ok()?;
    if !status.success() || too_large {
        return None;
    }
    Some(String::from_utf8_lossy(&output).into_owned())
}

fn nul_list(output: &str) -> Vec<String> {
    output
        .split('\0')
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn parse_commits(output: &str) -> Vec<DriftCommit> {
    output
        .split('\x1e')
        .map(|chunk| chunk.trim_start_matches('\n'))
        .filter(|chunk| chunk.contains('\0'))
        .map(|chunk| {
            let (sha, message) = chunk.split_once('\0').unwrap_or_default();
            DriftCommit {
                sha: sha.to_owned(),
                subject: message.lines().next().unwrap_or_default().to_owned(),
                message: message.trim().to_owned(),
            }
        })
        .collect()
}

fn is_tool_data(path: &str) -> bool {
    TOOL_DIRS
        .iter()
        .any(|directory| path == *directory || path.starts_with(&format!("{directory}/")))
}

/// Changed worktree/index/untracked files and commit messages since `rev`.
/// All failures are represented as an unavailable result so the CLI can stay
/// safe to invoke from a Stop hook.
#[must_use]
pub fn gather_drift(root: &Path, env: &Environment, rev: &str) -> DriftGit {
    if rev.starts_with('-') || rev.trim().is_empty() {
        return DriftGit::Unavailable {
            rev: rev.to_owned(),
            reason: format!("invalid revision \"{rev}\""),
        };
    }
    if git(root, env, &["rev-parse", "--is-inside-work-tree"])
        .is_none_or(|result| result.trim() != "true")
    {
        return DriftGit::Unavailable {
            rev: rev.to_owned(),
            reason: "not a git work tree".to_owned(),
        };
    }
    let commit = format!("{rev}^{{commit}}");
    if git(root, env, &["rev-parse", "--verify", "--quiet", &commit]).is_none() {
        return DriftGit::Unavailable {
            rev: rev.to_owned(),
            reason: format!("unknown revision \"{rev}\""),
        };
    }
    let diff = git(
        root,
        env,
        &["diff", "--name-only", "--relative", "-z", rev, "--"],
    );
    let untracked = git(
        root,
        env,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    );
    let (Some(diff), Some(untracked)) = (diff, untracked) else {
        return DriftGit::Unavailable {
            rev: rev.to_owned(),
            reason: "git diff failed".to_owned(),
        };
    };
    let mut files = nul_list(&diff)
        .into_iter()
        .chain(nul_list(&untracked))
        .filter(|file| !is_tool_data(file))
        .collect::<Vec<_>>();
    files.sort();
    files.dedup();
    let truncated = files.len() > MAX_FILES;
    files.truncate(MAX_FILES);
    let count = format!("-n{MAX_COMMITS}");
    let log = git(
        root,
        env,
        &[
            "log",
            &count,
            "--format=%h%x00%B%x1e",
            &format!("{rev}..HEAD"),
            "--",
        ],
    );
    DriftGit::Available {
        rev: rev.to_owned(),
        files,
        truncated,
        commits: log.map_or_else(Vec::new, |output| parse_commits(&output)),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DriftReasonKind {
    Slug,
    Prose,
    Seeds,
    Commit,
    Serves,
}

impl DriftReasonKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Slug => "slug",
            Self::Prose => "prose",
            Self::Seeds => "seeds",
            Self::Commit => "commit",
            Self::Serves => "serves",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DriftReason {
    pub kind: DriftReasonKind,
    pub detail: String,
}

#[derive(Clone, Debug)]
pub struct DriftIdeaInput {
    pub node: NodeRecord,
    /// `idea.md` plus text agent notes.
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct DriftMatchInput<'a> {
    pub graph: &'a Graph,
    pub config: &'a RootsConfig,
    pub ideas: &'a [DriftIdeaInput],
    pub files: &'a [String],
    pub commits: &'a [DriftCommit],
    pub issues: &'a [MissionsIssue],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DriftHit {
    pub node: NodeRecord,
    pub reasons: Vec<DriftReason>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DriftMatch {
    pub touched: Vec<DriftHit>,
    pub excluded: Vec<String>,
}

fn is_live(node: &NodeRecord) -> bool {
    node.kind == NodeKind::Idea && node.status != NodeStatus::Idea(IdeaStatus::Composted)
}

#[must_use]
pub fn slug_words(slug: &str) -> Vec<&str> {
    slug.split('-')
        .filter(|word| word.len() >= 3 && !STOP_WORDS.contains(word))
        .collect()
}

fn path_tokens(path: &str) -> HashSet<String> {
    path.split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

/// Whether text names a full changed path or a distinctive file basename.
#[must_use]
pub fn mentions_path(text: &str, path: &str) -> bool {
    if text.contains(path) {
        return true;
    }
    let base = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    base.len() >= 5
        && Path::new(base)
            .extension()
            .is_some_and(|extension| !extension.is_empty())
        && text.contains(base)
}

fn slug_reasons(node: &NodeRecord, files: &[String]) -> Vec<DriftReason> {
    let words = slug_words(&node.slug);
    if words.is_empty() {
        return Vec::new();
    }
    files
        .iter()
        .filter(|file| {
            let tokens = path_tokens(file);
            words.iter().all(|word| tokens.contains(*word))
        })
        .map(|file| DriftReason {
            kind: DriftReasonKind::Slug,
            detail: format!("{file} matches the slug"),
        })
        .collect()
}

fn prose_reasons(text: &str, files: &[String]) -> Vec<DriftReason> {
    files
        .iter()
        .filter(|file| mentions_path(text, file))
        .map(|file| DriftReason {
            kind: DriftReasonKind::Prose,
            detail: format!("the idea or its notes mention {file}"),
        })
        .collect()
}

fn seeds_reasons(
    node: &NodeRecord,
    issues: &[MissionsIssue],
    files: &[String],
) -> Vec<DriftReason> {
    let mut output = Vec::new();
    for issue in issues {
        if !issue_links(issue).0.iter().any(|id| id == &node.id) {
            continue;
        }
        let description = issue
            .value
            .get("description")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        // Drift deliberately limits this narrower test to title/description.
        // Labels can establish an issue-to-idea link, but legacy drift did not
        // treat a filename in a label as evidence that the change touched it.
        let text = format!("{}\n{description}", issue.title);
        for file in files {
            if mentions_path(&text, file) {
                output.push(DriftReason {
                    kind: DriftReasonKind::Seeds,
                    detail: format!("{} mentions {file}", issue.id),
                });
            }
        }
    }
    output
}

fn commit_reasons(node: &NodeRecord, commits: &[DriftCommit]) -> Vec<DriftReason> {
    commits
        .iter()
        .filter(|commit| {
            cited_idea_ids(&commit.message)
                .iter()
                .any(|id| id == &node.id)
        })
        .map(|commit| DriftReason {
            kind: DriftReasonKind::Commit,
            detail: format!("{} {}", commit.sha, commit.subject),
        })
        .collect()
}

fn direct_hits(input: &DriftMatchInput<'_>) -> Vec<DriftHit> {
    input
        .ideas
        .iter()
        .filter_map(|idea| {
            let mut reasons = commit_reasons(&idea.node, input.commits);
            reasons.extend(slug_reasons(&idea.node, input.files));
            reasons.extend(prose_reasons(&idea.text, input.files));
            reasons.extend(seeds_reasons(&idea.node, input.issues, input.files));
            (!reasons.is_empty()).then(|| DriftHit {
                node: idea.node.clone(),
                reasons,
            })
        })
        .collect()
}

fn serves_ancestors(graph: &Graph, node: &NodeRecord) -> Vec<NodeRecord> {
    let mut output = Vec::new();
    let mut seen = HashSet::from([node.id.clone()]);
    let mut frontier = vec![node.clone()];
    while !frontier.is_empty() {
        let mut next = Vec::new();
        for item in frontier {
            for edge in graph
                .edges
                .iter()
                .filter(|edge| edge.rel == EdgeRel::Serves && edge.from == item.id)
            {
                let Some(target) = graph.nodes.iter().find(|node| node.id == edge.to) else {
                    continue;
                };
                if !is_live(target) || !seen.insert(target.id.clone()) {
                    continue;
                }
                output.push(target.clone());
                next.push(target.clone());
            }
        }
        frontier = next;
    }
    output
}

fn add_ancestors(graph: &Graph, hits: Vec<DriftHit>) -> Vec<DriftHit> {
    let direct = hits
        .iter()
        .map(|hit| hit.node.id.clone())
        .collect::<HashSet<_>>();
    let mut extra_ids = HashSet::new();
    let mut extra = Vec::new();
    for hit in &hits {
        for ancestor in serves_ancestors(graph, &hit.node) {
            if direct.contains(&ancestor.id) {
                continue;
            }
            if let Some(index) = extra
                .iter()
                .position(|item: &DriftHit| item.node.id == ancestor.id)
            {
                extra[index].reasons.push(DriftReason {
                    kind: DriftReasonKind::Serves,
                    detail: format!("{} {} serves it", hit.node.id, hit.node.slug),
                });
            } else if extra_ids.insert(ancestor.id.clone()) {
                extra.push(DriftHit {
                    node: ancestor,
                    reasons: vec![DriftReason {
                        kind: DriftReasonKind::Serves,
                        detail: format!("{} {} serves it", hit.node.id, hit.node.slug),
                    }],
                });
            }
        }
    }
    let mut output = hits;
    output.extend(extra);
    output
}

/// Match changed paths/messages to live ideas. Direct hits retain graph order;
/// their transitive `serves` targets follow in first-discovery order.
#[must_use]
pub fn match_drift(input: &DriftMatchInput<'_>) -> DriftMatch {
    let all = add_ancestors(input.graph, direct_hits(input));
    let mut touched = Vec::new();
    let mut excluded = Vec::new();
    for hit in all {
        if effective_tier(input.config, Some(&hit.node)).tier >= DRIFT_TIER {
            touched.push(hit);
        } else {
            excluded.push(hit.node.id.clone());
        }
    }
    DriftMatch { touched, excluded }
}

fn note_text(paths: &RootsPaths, relative: &str) -> String {
    let file = paths.root.join(relative);
    let extension = file.extension().and_then(|extension| extension.to_str());
    if !matches!(extension, Some("md" | "txt")) || !file.exists() {
        return String::new();
    }
    let Ok(metadata) = fs::metadata(&file) else {
        return String::new();
    };
    if metadata.len() > MAX_NOTE_BYTES {
        return String::new();
    }
    fs::read_to_string(file).unwrap_or_default()
}

/// Live ideas and the prose/agent-note text available for matching.
pub fn drift_ideas(
    paths: &RootsPaths,
    graph: &Graph,
    dirs: &NodeDirs,
) -> RootsResult<Vec<DriftIdeaInput>> {
    graph
        .nodes
        .iter()
        .filter(|node| is_live(node))
        .map(|node| {
            let prose = read_node_prose(paths, node, Some(dirs))?;
            let notes = list_notes(paths, node)
                .into_iter()
                .map(|note| note_text(paths, &note.path))
                .collect::<Vec<_>>();
            Ok(DriftIdeaInput {
                node: node.clone(),
                text: std::iter::once(prose.text)
                    .chain(notes)
                    .collect::<Vec<_>>()
                    .join("\n"),
            })
        })
        .collect()
}

/// The prose path relative to the project root, for a drift packet.
pub fn idea_file(
    paths: &RootsPaths,
    node: &NodeRecord,
    dirs: &NodeDirs,
) -> RootsResult<Option<String>> {
    let path = read_node_prose(paths, node, Some(dirs))?.path;
    Ok(path.map(|file| {
        file.strip_prefix(&paths.root)
            .unwrap_or(&file)
            .display()
            .to_string()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::default_config;
    use crate::model::{EdgeRecord, IdeaStatus, NodeKind, NodeStatus};
    use std::collections::BTreeMap;

    fn idea(id: &str, slug: &str) -> NodeRecord {
        NodeRecord::new(
            id.to_owned(),
            NodeKind::Idea,
            slug.to_owned(),
            NodeStatus::Idea(IdeaStatus::Planted),
            "human:test".to_owned(),
            "now".to_owned(),
        )
    }

    #[test]
    fn matches_slug_commit_and_serves_then_excludes_low_overrides() {
        let goal = idea("r-goal", "trust-data");
        let sync = idea("r-sync", "offline-sync");
        let mut quiet = idea("r-quiet", "quiet-zone");
        quiet.tier = Some(1);
        let graph = Graph {
            nodes: vec![goal.clone(), sync.clone(), quiet.clone()],
            edges: vec![EdgeRecord {
                record_type: "edge".to_owned(),
                id: "e-1".to_owned(),
                from: sync.id.clone(),
                to: goal.id.clone(),
                rel: EdgeRel::Serves,
                by: "human:test".to_owned(),
                proposed_by: None,
                proposal: None,
                created_at: "now".to_owned(),
            }],
        };
        let mut config = default_config("test");
        config.tier = 3;
        let ideas = vec![
            DriftIdeaInput {
                node: sync.clone(),
                text: String::new(),
            },
            DriftIdeaInput {
                node: quiet.clone(),
                text: String::new(),
            },
        ];
        let files = vec![
            "src/sync/offline.ts".to_owned(),
            "src/quiet/zone.ts".to_owned(),
        ];
        let result = match_drift(&DriftMatchInput {
            graph: &graph,
            config: &config,
            ideas: &ideas,
            files: &files,
            commits: &[],
            issues: &[],
        });
        assert_eq!(
            result
                .touched
                .iter()
                .map(|hit| hit.node.id.as_str())
                .collect::<Vec<_>>(),
            ["r-sync", "r-goal"]
        );
        assert_eq!(result.excluded, ["r-quiet"]);
        assert_eq!(result.touched[1].reasons[0].kind, DriftReasonKind::Serves);
        assert_eq!(slug_words("the-offline-sync"), ["offline", "sync"]);
        assert!(mentions_path("see src/output.ts", "src/output.ts"));
        assert!(!mentions_path("output", "src/output.ts"));
    }

    #[test]
    fn rejects_option_like_revisions_without_invoking_git() {
        let root = std::env::temp_dir();
        let result = gather_drift(&root, &BTreeMap::new(), "--output=/tmp/x");
        assert_eq!(
            result,
            DriftGit::Unavailable {
                rev: "--output=/tmp/x".to_owned(),
                reason: "invalid revision \"--output=/tmp/x\"".to_owned(),
            }
        );
    }
}
