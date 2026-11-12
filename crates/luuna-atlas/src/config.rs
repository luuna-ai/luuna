//! config.yaml: reading with the Go tool's defaults, writing, and adding domains.
//!
//! Every function takes the store's `.luuna/atlas` directory as a parameter. Task t1
//! declares the functions; task t4 fills them in.

use std::path::Path;

use crate::error::{AtlasError, Result};
use crate::json::Object;

/// A parsed config.yaml, keys in file order.
pub type Config = Object;

/// The line `init` adds to the project's `.gitattributes`.
pub const GIT_ATTRIBUTES_LINE: &str = ".luuna/atlas/expertise/*.jsonl merge=union";

/// The config.yaml that `init` writes, comments included: the Go tool's
/// `InitialConfigYAML`. Any command that rewrites the config drops the comments.
pub const INITIAL_CONFIG_YAML: &str = r##"# Atlas configuration. See https://github.com/luuna-ai/luuna-atlas for docs.
#
# Required fields below. Optional knobs are commented out at the bottom — uncomment
# to enable. Note: commands that mutate this file (e.g. `luuna-atlas add <domain>`,
# `luuna-atlas record` auto-creating a domain) rewrite it via the YAML serializer and
# strip these comments.

version: '1'
domains: {}
governance:
  max_entries: 100
  warn_entries: 150
  hard_limit: 200
classification_defaults:
  shelf_life:
    tactical: 14
    observational: 30

# ─── Optional knobs (uncomment to enable) ────────────────────────────────────
#
# prime:
#   # Pin `luuna-atlas prime`'s unscoped output shape. When unset (recommended), prime
#   # emits a budget-capped record index (one line per record, failures first).
#   #   - full     → the record index (same as unset)
#   #   - manifest → a per-domain count table instead of records
#   # --full / --manifest / scoping flags always override this on a per-call basis.
#   default_mode: full        # one of: full, manifest
#   # Trust-tier ranking weights for full-mode output. Sort score per record =
#   # (★ count * star) + classification weight. Override only the knobs you
#   # want to retune; unset fields keep their default. Higher scores surface
#   # first; within-tier ties preserve insertion order.
#   tier_weights:
#     star: 100               # multiplier on the ★ confirmation count
#     foundational: 50        # base score for foundational records
#     tactical: 20             # base score for tactical records
#     observational: 10        # base score for observational records
#
# search:
#   # Multiplier applied to BM25 scores so records with more confirmed outcomes
#   # rank higher. 0 disables (pure BM25). Default 0.1: a record with N successes
#   # gets a (1 + 0.1*N) boost. Override per-call with `luuna-atlas search --no-boost`.
#   boost_factor: 0.1
#
# disabled_types:
#   # Names of registered types (built-in or custom) that emit a deprecation
#   # warning on write. Reads still succeed; the type stays in CLI choices so
#   # peers in shared domains don't hard-fail. Use to retire a type gracefully.
#   - failure
#
# custom_types:
#   # Project-specific record types. Required + optional fields, dedup_key,
#   # and a summary template. See README for the full schema.
#   hypothesis:
#     required: [statement, prediction]
#     optional: [evidence_files]
#     dedup_key: statement
#     summary: "{statement}"   # tokens use single braces; {{statement}} also accepted
#     # aliases: rename a field while still reading legacy on-disk records.
#     # Map canonical (current) name → list of legacy names. At read time,
#     # legacy fields are rewritten to canonical and dropped from the record.
#     aliases:
#       statement: [claim]
#   # Inherit from a built-in type with extends:. Required/optional arrays
#   # merge with the parent's (union); dedup_key, summary, compact, section_title,
#   # extracts_files, and files_field override only when set, otherwise inherit.
#   # Custom-from-custom is not supported in v1.
#   adr:
#     extends: decision
#     required: [decision_status, deciders]   # added on top of decision's [title, rationale]
#     summary: "{decision_status}: {title}"   # tokens must be declared fields (parent's included)
#
# quality:
#   # Write-time gate on luuna-atlas record: near-duplicates, conventions/patterns with
#   # no file/dir anchor or evidence, conventions that restate code.
#   # warn (default) prints and writes; error blocks unless --force; off skips.
#   # Exact duplicates are always blocked.
#   level: warn
#
# hooks:
#   # Lifecycle hook scripts. Each event maps to an ordered list of shell
#   # commands. Atlas invokes each with the relevant payload as JSON on stdin.
#   # Exit 0 = continue. Non-zero from a `pre-*` hook blocks the action; from
#   # a `post-*` hook emits a warning. `pre-record`, `pre-prime`, and
#   # `pre-compact` may mutate the payload by printing modified JSON on
#   # stdout; `pre-prune` is block-or-allow only — its stdout is ignored.
#   #
#   # pre-record:    [./.luuna/atlas/hooks/scan-secrets.sh, ./.luuna/atlas/hooks/require-owner.sh]
#   # post-record:   [./.luuna/atlas/hooks/post-to-slack.sh]
#   # pre-prime:     [./.luuna/atlas/hooks/filter-by-team.sh]
#   # pre-prune:     [./.luuna/atlas/hooks/digest-then-confirm.sh]
#   # pre-compact:   [./.luuna/atlas/hooks/llm-summarize.sh]
#
# hook_settings:
#   # Per-hook execution timeout in milliseconds. Default 5000.
#   timeout_ms: 5000
"##;

/// Refuses a domain name that does not match `^[a-zA-Z0-9][a-zA-Z0-9_-]*$`, with the Go
/// tool's message.
pub fn validate_domain_name(_domain: &str) -> Result<()> {
    Err(AtlasError::not_implemented(
        "config::validate_domain_name (task t4)",
    ))
}

/// Reads `<atlas_dir>/config.yaml` and merges the Go tool's defaults into it.
pub fn read_config(_atlas_dir: &Path) -> Result<Config> {
    Err(AtlasError::not_implemented("config::read_config (task t4)"))
}

/// Rewrites `<atlas_dir>/config.yaml` from `config`.
pub fn write_config(_atlas_dir: &Path, _config: &Config) -> Result<()> {
    Err(AtlasError::not_implemented(
        "config::write_config (task t4)",
    ))
}

/// Adds `domain` to config.yaml if absent and creates its empty expertise file if absent.
pub fn add_domain(_atlas_dir: &Path, _domain: &str) -> Result<()> {
    Err(AtlasError::not_implemented("config::add_domain (task t4)"))
}
