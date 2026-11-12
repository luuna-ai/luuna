//! Embedded format assets and the session-end reminder. Owner: s5-atlas-lib-2.
//!
//! A port of the Go tool's `format_assets.go`: the 32 generated prose blocks the prime,
//! session-close, setup and onboard output is built from.

/// The 32 asset keys, in `format_assets.go` order.
pub const KEYS: [&str; 32] = [
    "prime_compact",
    "prime_empty_compact",
    "prime_markdown",
    "prime_empty_markdown",
    "prime_plain",
    "prime_empty_plain",
    "prime_xml",
    "prime_empty_xml",
    "close_conditional_compact",
    "close_conditional_markdown",
    "close_conditional_plain",
    "close_conditional_xml",
    "close_conditional_embedded",
    "close_minimal_compact",
    "close_minimal_markdown",
    "close_minimal_plain",
    "close_minimal_xml",
    "close_minimal_embedded",
    "close_directive_compact",
    "close_directive_markdown",
    "close_directive_plain",
    "close_directive_xml",
    "close_directive_embedded",
    "close_none_compact",
    "close_none_markdown",
    "close_none_plain",
    "close_none_xml",
    "close_none_embedded",
    "cursor",
    "codex",
    "git_hook",
    "onboard",
];

/// The asset `key` byte for byte as `formatAssets[key]`, or `""` for any other key (a Go
/// map's zero value).
pub fn get(key: &str) -> &'static str {
    match key {
        "prime_compact" => "# Project Expertise (via Atlas)\n\nIndex: one line per record, failures first. `luuna-atlas show <id>` prints the full record.\n\n__ATLAS_SECTIONS__\n\n## Quick Reference\n\n- `luuna-atlas show <id>` — full record body\n- `luuna-atlas search \"query\"` — find relevant records before implementing\n- `luuna-atlas prime --files src/foo.ts` — prime **before** editing a file, not just at session start\n- `luuna-atlas prime --full` — full record bodies instead of the index\n- `luuna-atlas record <domain> --type <type> --description \"...\"` — evidence auto-populates from git; link with `--relates-to <mx-id>`",
        "prime_empty_compact" => "# Project Expertise (via Atlas)\n\nNo expertise recorded yet. Use `luuna-atlas add <domain>` to create a domain, then `luuna-atlas record` to add records.\n\n## Quick Reference\n\n- `luuna-atlas show <id>` — full record body\n- `luuna-atlas search \"query\"` — find relevant records before implementing\n- `luuna-atlas prime --files src/foo.ts` — prime **before** editing a file, not just at session start\n- `luuna-atlas prime --full` — full record bodies instead of the index\n- `luuna-atlas record <domain> --type <type> --description \"...\"` — evidence auto-populates from git; link with `--relates-to <mx-id>`",
        "prime_markdown" => "# Project Expertise (via Atlas)\n\n> **Context Recovery**: Run `luuna-atlas prime` after compaction, clear, or new session\n\n## Rules\n\n- **Record learnings**: When you discover a pattern, fix a bug, or make a design decision — record it with `luuna-atlas record`\n- **Check expertise first**: Before implementing, check if relevant expertise exists with `luuna-atlas search` or `luuna-atlas prime --context`\n- **Targeted priming**: Use `luuna-atlas prime --files src/foo.ts` to load only records relevant to specific files\n- **Do NOT** store expertise in code comments, markdown files, or memory tools — use `luuna-atlas record`\n- Run `luuna-atlas doctor` if you are unsure whether records are healthy\n\n__ATLAS_SECTIONS__\n\n\n## Recording New Learnings\n\nWhen you discover a pattern, convention, failure, or make an architectural decision:\n\n```bash\nluuna-atlas record <domain> --type convention \"description\"\nluuna-atlas record <domain> --type failure --description \"...\" --resolution \"...\"\nluuna-atlas record <domain> --type decision --title \"...\" --rationale \"...\"\nluuna-atlas record <domain> --type pattern --name \"...\" --description \"...\" --files \"...\"\nluuna-atlas record <domain> --type reference --name \"...\" --description \"...\" --files \"...\"\nluuna-atlas record <domain> --type guide --name \"...\" --description \"...\"\n```\n\n**Required fields by type:**\n\n| Type | Required flags |\n|------|----------------|\n| `convention` | `\"<content>\"` (positional) |\n| `pattern` | `--name \"...\" --description \"...\"` |\n| `failure` | `--description \"...\" --resolution \"...\"` |\n| `decision` | `--title \"...\" --rationale \"...\"` |\n| `reference` | `--name \"...\" --description \"...\"` |\n| `guide` | `--name \"...\" --description \"...\"` |\n\n**Link evidence** to records. The current commit and changed files auto-populate from git; link trackers or related records explicitly:\n\n```bash\nluuna-atlas record <domain> --type pattern --name \"...\" --description \"...\" --evidence-missions SEED-123\nluuna-atlas record <domain> --type decision --title \"...\" --rationale \"...\" --evidence-gh 42 --evidence-linear ENG-9\nluuna-atlas record <domain> --type convention \"...\" --relates-to mx-abc  # link to related records\n```\n\n**Batch record** multiple records at once:\n\n```bash\nluuna-atlas record <domain> --batch records.json  # from file\necho '[{\"type\":\"convention\",\"content\":\"...\"}]' | luuna-atlas record <domain> --stdin  # from stdin\n```\n\n## Searching Expertise\n\nUse `luuna-atlas search` to find relevant records across all domains. Results are ranked by relevance (BM25):\n\n```bash\nluuna-atlas search \"file locking\"              # multi-word queries ranked by relevance\nluuna-atlas search \"atomic\" --domain cli        # limit to a specific domain\nluuna-atlas search \"ESM\" --type convention      # filter by record type\nluuna-atlas search \"concurrency\" --tag safety   # filter by tag\n```\n\nSearch before implementing — existing expertise may already cover your use case.\n\n## Domain Maintenance\n\nWhen a domain grows large, compact it to keep expertise focused:\n\n```bash\nluuna-atlas compact --auto --dry-run     # preview what would be merged\nluuna-atlas compact --auto               # merge same-type record groups\n```\n\nUse `luuna-atlas diff` to review what expertise changed:\n\n```bash\nluuna-atlas diff HEAD~3                  # see record changes over last 3 commits\n```\n\n## Session End\n\n**IMPORTANT**: Before ending your session, record what you learned and sync:\n\n```\n[ ] luuna-atlas learn          # see what files changed — decide what to record\n[ ] luuna-atlas record ...     # record learnings (see above)\n[ ] luuna-atlas sync           # validate, stage, and commit .luuna/atlas/ changes\n```\n\nDo NOT skip this. Unrecorded learnings are lost for the next session.",
        "prime_empty_markdown" => "# Project Expertise (via Atlas)\n\n> **Context Recovery**: Run `luuna-atlas prime` after compaction, clear, or new session\n\n## Rules\n\n- **Record learnings**: When you discover a pattern, fix a bug, or make a design decision — record it with `luuna-atlas record`\n- **Check expertise first**: Before implementing, check if relevant expertise exists with `luuna-atlas search` or `luuna-atlas prime --context`\n- **Targeted priming**: Use `luuna-atlas prime --files src/foo.ts` to load only records relevant to specific files\n- **Do NOT** store expertise in code comments, markdown files, or memory tools — use `luuna-atlas record`\n- Run `luuna-atlas doctor` if you are unsure whether records are healthy\n\nNo expertise recorded yet. Use `luuna-atlas add <domain>` to create a domain, then `luuna-atlas record` to add records.\n\n\n## Recording New Learnings\n\nWhen you discover a pattern, convention, failure, or make an architectural decision:\n\n```bash\nluuna-atlas record <domain> --type convention \"description\"\nluuna-atlas record <domain> --type failure --description \"...\" --resolution \"...\"\nluuna-atlas record <domain> --type decision --title \"...\" --rationale \"...\"\nluuna-atlas record <domain> --type pattern --name \"...\" --description \"...\" --files \"...\"\nluuna-atlas record <domain> --type reference --name \"...\" --description \"...\" --files \"...\"\nluuna-atlas record <domain> --type guide --name \"...\" --description \"...\"\n```\n\n**Required fields by type:**\n\n| Type | Required flags |\n|------|----------------|\n| `convention` | `\"<content>\"` (positional) |\n| `pattern` | `--name \"...\" --description \"...\"` |\n| `failure` | `--description \"...\" --resolution \"...\"` |\n| `decision` | `--title \"...\" --rationale \"...\"` |\n| `reference` | `--name \"...\" --description \"...\"` |\n| `guide` | `--name \"...\" --description \"...\"` |\n\n**Link evidence** to records. The current commit and changed files auto-populate from git; link trackers or related records explicitly:\n\n```bash\nluuna-atlas record <domain> --type pattern --name \"...\" --description \"...\" --evidence-missions SEED-123\nluuna-atlas record <domain> --type decision --title \"...\" --rationale \"...\" --evidence-gh 42 --evidence-linear ENG-9\nluuna-atlas record <domain> --type convention \"...\" --relates-to mx-abc  # link to related records\n```\n\n**Batch record** multiple records at once:\n\n```bash\nluuna-atlas record <domain> --batch records.json  # from file\necho '[{\"type\":\"convention\",\"content\":\"...\"}]' | luuna-atlas record <domain> --stdin  # from stdin\n```\n\n## Searching Expertise\n\nUse `luuna-atlas search` to find relevant records across all domains. Results are ranked by relevance (BM25):\n\n```bash\nluuna-atlas search \"file locking\"              # multi-word queries ranked by relevance\nluuna-atlas search \"atomic\" --domain cli        # limit to a specific domain\nluuna-atlas search \"ESM\" --type convention      # filter by record type\nluuna-atlas search \"concurrency\" --tag safety   # filter by tag\n```\n\nSearch before implementing — existing expertise may already cover your use case.\n\n## Domain Maintenance\n\nWhen a domain grows large, compact it to keep expertise focused:\n\n```bash\nluuna-atlas compact --auto --dry-run     # preview what would be merged\nluuna-atlas compact --auto               # merge same-type record groups\n```\n\nUse `luuna-atlas diff` to review what expertise changed:\n\n```bash\nluuna-atlas diff HEAD~3                  # see record changes over last 3 commits\n```\n\n## Session End\n\n**IMPORTANT**: Before ending your session, record what you learned and sync:\n\n```\n[ ] luuna-atlas learn          # see what files changed — decide what to record\n[ ] luuna-atlas record ...     # record learnings (see above)\n[ ] luuna-atlas sync           # validate, stage, and commit .luuna/atlas/ changes\n```\n\nDo NOT skip this. Unrecorded learnings are lost for the next session.",
        "prime_plain" => "__ATLAS_SECTIONS__",
        "prime_empty_plain" => "No expertise recorded yet. Use `luuna-atlas add <domain>` and `luuna-atlas record` to get started.",
        "prime_xml" => "<expertise>\n__ATLAS_SECTIONS__\n</expertise>",
        "prime_empty_xml" => "<expertise>\n  <empty>No expertise recorded yet. Use luuna-atlas add and luuna-atlas record to get started.</empty>\n</expertise>",
        "close_conditional_compact" => "# SESSION CLOSE\n\n**Record only what a future agent would get wrong without it.** Prefer a `failure` record (symptom -> root cause -> preventive check). Do not restate code, CLAUDE.md, or existing records; if an existing record helped, confirm it instead:\n\n```bash\nluuna-atlas learn                                   # see what files changed\nluuna-atlas record <domain> --type <type> ...       # record the insight\nluuna-atlas outcome <domain> <id> --status success  # confirm a record that helped\nluuna-atlas sync                                    # validate, stage, commit\n```\n\nSkip if nothing qualifies. Filler records are noise every future session pays to read.",
        "close_conditional_markdown" => "# SESSION CLOSE\n\n**Record only what a future agent would get wrong without it.** Prefer a `failure` record (symptom -> root cause -> preventive check). Do not restate code, CLAUDE.md, or existing records; if an existing record helped, confirm it instead:\n\n```bash\nluuna-atlas learn                                   # see what files changed\nluuna-atlas record <domain> --type <type> ...       # record the insight\nluuna-atlas outcome <domain> <id> --status success  # confirm a record that helped\nluuna-atlas sync                                    # validate, stage, commit\n```\n\nSkip if nothing qualifies. Filler records are noise every future session pays to read.",
        "close_conditional_plain" => "=== SESSION CLOSE ===\n\nRecord only what a future agent would get wrong without it. Prefer a failure record:\nsymptom -> root cause -> preventive check. Do not restate code, CLAUDE.md, or existing records.\n\n  luuna-atlas learn                                        (see what files changed)\n  luuna-atlas record <domain> --type <type> ...            (record the insight)\n  luuna-atlas outcome <domain> <id> --status success       (confirm a record that helped)\n  luuna-atlas sync                                         (validate, stage, commit)\n\nSkip if nothing qualifies. Filler records are noise every future session pays to read.",
        "close_conditional_xml" => "<session_close>\n  <instruction>Record only what a future agent would get wrong without it. Prefer a failure record: symptom, root cause, preventive check. Do not restate code, CLAUDE.md, or existing records.</instruction>\n  <commands>\n    <command>luuna-atlas learn — see what files changed</command>\n    <command>luuna-atlas record &lt;domain&gt; --type &lt;type&gt; --description &quot;...&quot;</command>\n    <command>luuna-atlas outcome &lt;domain&gt; &lt;id&gt; --status success — confirm a record that helped</command>\n    <command>luuna-atlas sync — validate, stage, commit</command>\n  </commands>\n  <note>Skip if nothing qualifies. Filler records are noise every future session pays to read.</note>\n</session_close>",
        "close_conditional_embedded" => "### Before You Finish\n\nRecord only what a future agent would get wrong without it. Prefer a `failure` record\n(symptom -> root cause -> preventive check). Do not restate code, CLAUDE.md, or existing\nrecords; if an existing record helped, confirm it instead:\n\n```bash\nluuna-atlas learn                                                                    # see what files changed\nluuna-atlas record <domain> --type <convention|pattern|failure|decision|reference|guide> --description \"...\"\nluuna-atlas outcome <domain> <id> --status success                                   # confirm a record that helped\nluuna-atlas sync                                                                     # validate, stage, commit\n```\n\nSkip if nothing qualifies. Filler records are noise every future session pays to read.",
        "close_minimal_compact" => "**Before closing**: record any insight worth preserving — `luuna-atlas learn` → `luuna-atlas record <domain> --type <type> …` → `luuna-atlas sync`. Skip if no insight surfaced.",
        "close_minimal_markdown" => "**Before closing**: record any insight worth preserving — `luuna-atlas learn` → `luuna-atlas record <domain> --type <type> …` → `luuna-atlas sync`. Skip if no insight surfaced.",
        "close_minimal_plain" => "Before closing, record any insight worth preserving: luuna-atlas learn → luuna-atlas record <domain> --type <type> … → luuna-atlas sync. Skip if no insight surfaced.",
        "close_minimal_xml" => "<session_close>\n  <instruction>Before closing, record any insight worth preserving: `luuna-atlas learn` → `luuna-atlas record &lt;domain&gt; --type &lt;type&gt; …` → `luuna-atlas sync`. Skip if no insight surfaced.</instruction>\n</session_close>",
        "close_minimal_embedded" => "**Before you finish**: record any insight worth preserving with `luuna-atlas learn` → `luuna-atlas record <domain> --type <type> …` → `luuna-atlas sync`. Skip if no insight surfaced.",
        "close_directive_compact" => "# SESSION CLOSE\n\n**Record insights before exiting:**\n\n1. `luuna-atlas learn` — see changed files and suggested domains\n2. `luuna-atlas record <domain> --type <type> …` — record each insight\n3. `luuna-atlas sync` — validate, stage, commit\n\n**Types**:\n- `convention` — a norm the project follows (naming, formatting, …)\n- `pattern` — how-to: a repeatable approach to a class of problem\n- `failure` — anti-pattern + fix: what broke and how you resolved it\n- `decision` — we picked X over Y, and why\n- `reference` — pointer to an external spec / doc / codebase landmark\n- `guide` — a walk-through of a multi-step workflow\n\n**Dedup**: `--name` upserts merge; re-recording is safe — outcomes accumulate, fields update in place.\n\n**Do not record**: restatements of CLAUDE.md or prior records, generic truisms (\"use TypeScript\"), single-PR-scoped facts (\"this PR adds X\"), ritual confirmations with no new information.\n\nSkip if no insight surfaced. Unrecorded learnings are lost; ritual filler records are also noise.",
        "close_directive_markdown" => "# SESSION CLOSE\n\n**Record insights before exiting:**\n\n1. `luuna-atlas learn` — see changed files and suggested domains\n2. `luuna-atlas record <domain> --type <type> …` — record each insight\n3. `luuna-atlas sync` — validate, stage, commit\n\n**Types**:\n- `convention` — a norm the project follows (naming, formatting, …)\n- `pattern` — how-to: a repeatable approach to a class of problem\n- `failure` — anti-pattern + fix: what broke and how you resolved it\n- `decision` — we picked X over Y, and why\n- `reference` — pointer to an external spec / doc / codebase landmark\n- `guide` — a walk-through of a multi-step workflow\n\n**Dedup**: `--name` upserts merge; re-recording is safe — outcomes accumulate, fields update in place.\n\n**Do not record**: restatements of CLAUDE.md or prior records, generic truisms (\"use TypeScript\"), single-PR-scoped facts (\"this PR adds X\"), ritual confirmations with no new information.\n\nSkip if no insight surfaced. Unrecorded learnings are lost; ritual filler records are also noise.",
        "close_directive_plain" => "=== SESSION CLOSE ===\n\nRecord insights before exiting:\n\n  1. luuna-atlas learn                              (see changed files + suggested domains)\n  2. luuna-atlas record <domain> --type <type> ...  (record each insight)\n  3. luuna-atlas sync                               (validate, stage, commit)\n\nTypes:\n  - convention : a norm the project follows (naming, formatting, ...)\n  - pattern    : how-to — a repeatable approach to a class of problem\n  - failure    : anti-pattern + fix — what broke and how you resolved it\n  - decision   : we picked X over Y, and why\n  - reference  : pointer to an external spec / doc / codebase landmark\n  - guide      : a walk-through of a multi-step workflow\n\nDedup: --name upserts merge; re-recording is safe — outcomes accumulate, fields update in place.\n\nDo NOT record:\n  - Restatements of CLAUDE.md or prior records\n  - Generic truisms (\"use TypeScript\", \"write tests\")\n  - Single-PR-scoped facts (\"this PR adds X\")\n  - Ritual confirmations with no new information\n\nSkip if no insight surfaced. Unrecorded learnings are lost; ritual filler records are also noise.",
        "close_directive_xml" => "<session_close>\n  <heading>SESSION CLOSE — record insights before exiting</heading>\n  <steps>\n    <step n=\"1\">Run `luuna-atlas learn` to see changed files and suggested domains.</step>\n    <step n=\"2\">Run `luuna-atlas record &lt;domain&gt; --type &lt;type&gt; …` for each insight worth preserving.</step>\n    <step n=\"3\">Run `luuna-atlas sync` to validate, stage, and commit the .luuna/atlas/ changes.</step>\n  </steps>\n  <types>\n    <type name=\"convention\">a norm the project follows (e.g. naming, formatting)</type>\n    <type name=\"pattern\">how-to: a repeatable approach to a class of problem</type>\n    <type name=\"failure\">anti-pattern + fix: what broke and how you resolved it</type>\n    <type name=\"decision\">we picked X over Y, and why</type>\n    <type name=\"reference\">pointer to an external spec, doc, or codebase landmark</type>\n    <type name=\"guide\">a walk-through of a multi-step workflow</type>\n  </types>\n  <dedup>`--name` upserts merge; re-recording is safe — outcomes accumulate, fields update in place.</dedup>\n  <do_not_record>\n    <item>Restatements of CLAUDE.md or prior records</item>\n    <item>Generic truisms (\"use TypeScript\", \"write tests\")</item>\n    <item>Single-PR-scoped facts (\"this PR adds X\")</item>\n    <item>Ritual confirmations with no new information</item>\n  </do_not_record>\n  <note>Skip if no insight surfaced. Unrecorded learnings are lost; ritual filler records are also noise.</note>\n</session_close>",
        "close_directive_embedded" => "### Before You Finish\n\nRecord insights worth preserving before closing this session:\n\n1. `luuna-atlas learn` — see changed files and suggested domains\n2. `luuna-atlas record <domain> --type <type> …` — record each insight\n3. `luuna-atlas sync` — validate, stage, commit\n\n**Types**:\n- `convention` — a norm the project follows (naming, formatting, …)\n- `pattern` — how-to: a repeatable approach to a class of problem\n- `failure` — anti-pattern + fix: what broke and how you resolved it\n- `decision` — we picked X over Y, and why\n- `reference` — pointer to an external spec / doc / codebase landmark\n- `guide` — a walk-through of a multi-step workflow\n\n**Dedup**: `--name` upserts merge; re-recording is safe — outcomes accumulate, fields update in place.\n\n**Do not record**: restatements of CLAUDE.md or prior records, generic truisms (\"use TypeScript\"), single-PR-scoped facts (\"this PR adds X\"), ritual confirmations with no new information.\n\nSkip if no insight surfaced. Unrecorded learnings are lost; ritual filler records are also noise.",
        "close_none_compact" => "",
        "close_none_markdown" => "",
        "close_none_plain" => "",
        "close_none_xml" => "",
        "close_none_embedded" => "",
        "cursor" => "---\ndescription: Atlas expertise integration\nglobs: *\nalwaysApply: true\n---\n\n# Atlas Expertise\n\nAt the start of every session, run the following command to load project expertise:\n\n```\nluuna-atlas prime\n```\n\nThis injects project-specific conventions, patterns, decisions, and other learnings into your context.\nUse `luuna-atlas prime --files src/foo.ts` to load only records relevant to specific files.\n\nEvidence auto-populates from git (current commit + changed files). Link trackers explicitly with `--evidence-missions <id>` / `--evidence-gh <id>` / `--evidence-linear <id>` / `--evidence-bead <id>`, or `--relates-to <mx-id>`.\n\n### Before You Finish\n\nRecord only what a future agent would get wrong without it. Prefer a `failure` record\n(symptom -> root cause -> preventive check). Do not restate code, CLAUDE.md, or existing\nrecords; if an existing record helped, confirm it instead:\n\n```bash\nluuna-atlas learn                                                                    # see what files changed\nluuna-atlas record <domain> --type <convention|pattern|failure|decision|reference|guide> --description \"...\"\nluuna-atlas outcome <domain> <id> --status success                                   # confirm a record that helped\nluuna-atlas sync                                                                     # validate, stage, commit\n```\n\nSkip if nothing qualifies. Filler records are noise every future session pays to read.\n",
        "codex" => "<!-- luuna-atlas:start -->\n## Atlas Expertise\n\nAt the start of every session, run `luuna-atlas prime` to load project expertise.\n\nThis injects project-specific conventions, patterns, decisions, and other learnings into your context.\nUse `luuna-atlas prime --files src/foo.ts` to load only records relevant to specific files.\n\nEvidence auto-populates from git (current commit + changed files). Link trackers explicitly with `--evidence-missions <id>` / `--evidence-gh <id>` / `--evidence-linear <id>` / `--evidence-bead <id>`, or `--relates-to <mx-id>`.\n\n### Before You Finish\n\nRecord only what a future agent would get wrong without it. Prefer a `failure` record\n(symptom -> root cause -> preventive check). Do not restate code, CLAUDE.md, or existing\nrecords; if an existing record helped, confirm it instead:\n\n```bash\nluuna-atlas learn                                                                    # see what files changed\nluuna-atlas record <domain> --type <convention|pattern|failure|decision|reference|guide> --description \"...\"\nluuna-atlas outcome <domain> <id> --status success                                   # confirm a record that helped\nluuna-atlas sync                                                                     # validate, stage, commit\n```\n\nSkip if nothing qualifies. Filler records are noise every future session pays to read.\n<!-- luuna-atlas:end -->",
        "git_hook" => "# luuna-atlas:start\n# Run luuna-atlas validate before committing\nif command -v luuna-atlas >/dev/null 2>&1; then\n  luuna-atlas validate\n  if [ $? -ne 0 ]; then\n    echo \"luuna-atlas validate failed. Commit aborted.\"\n    exit 1\n  fi\nfi\n# luuna-atlas:end",
        "onboard" => "## Project Expertise (Atlas)\n<!-- atlas-onboard:v0.11.0 -->\n\nThis project uses [Atlas](https://github.com/luuna-ai/luuna-atlas) v0.11.0 for structured expertise management.\n\n**At the start of every session**, run:\n```bash\nluuna-atlas prime\n```\n\nPrints a budget-capped index of project expertise: one line per record (id, type, summary,\nanchors), failures first. Run `luuna-atlas show <id>` for a full record and `luuna-atlas prime --files src/foo.ts`\nbefore editing a file to load only records relevant to that path.\n\n**Record only what a future agent would get wrong without it.** Prefer `failure` records\n(symptom -> root cause -> preventive check); anchor conventions and patterns with `--files` or\n`--dir-anchor`. `luuna-atlas record` blocks exact duplicates and flags near-duplicates of existing\nrecords (suggesting `--supersedes <id>`; `--force` records anyway):\n```bash\nluuna-atlas record <domain> --type <convention|pattern|failure|decision|reference|guide> --description \"...\"\n```\n\nEvidence auto-populates from git (current commit + changed files). Link explicitly with\n`--evidence-missions <id>` / `--evidence-gh <id>` / `--evidence-linear <id>` / `--evidence-bead <id>`,\n`--evidence-commit <sha>`, or `--relates-to <mx-id>`. Upserts of named records merge outcomes\ninstead of replacing them; validation failures print a copy-paste retry hint with missing fields\npre-filled.\n\nRun `luuna-atlas status` for domain health, `luuna-atlas doctor` to check record integrity (add `--fix` to strip\nbroken file anchors), `luuna-atlas --help` for the full command list. Write commands use file locking and\natomic writes, so multiple agents can record concurrently. Expertise survives `git worktree`\ncleanup — `.luuna/atlas/` resolves to the main repo.\n\n`luuna-atlas prune` soft-archives stale records to `.luuna/atlas/archive/` instead of deleting them; pass\n`--hard` for true deletion. Restore an archived record with `luuna-atlas restore <id>`. Do not read\n`.luuna/atlas/archive/` directly — those records are stale by definition. If you need historical\ncontext, run `luuna-atlas search --archived <query>`.\n\n__ATLAS_SESSION_CLOSE__\n",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_key_is_empty() {
        assert_eq!(get(""), "");
        assert_eq!(get("no_such_asset"), "");
        assert_eq!(get("PRIME_COMPACT"), "");
        assert_eq!(get("close_conditional_json"), "");
    }

    #[test]
    fn thirty_two_keys_in_go_order() {
        assert_eq!(KEYS.len(), 32);
        assert_eq!(KEYS[0], "prime_compact");
        assert_eq!(KEYS[8], "close_conditional_compact");
        assert_eq!(KEYS[31], "onboard");
        for (i, key) in KEYS.iter().enumerate() {
            assert!(!KEYS[..i].contains(key), "duplicate key {key}");
            if key.starts_with("close_none_") {
                assert_eq!(get(key), "", "{key}");
            } else {
                assert!(!get(key).is_empty(), "{key} is empty");
            }
        }
        assert!(get("prime_compact").starts_with("# Project Expertise (via Atlas)\n"));
        assert_eq!(get("prime_plain"), "__ATLAS_SECTIONS__");
    }

    /// Reads the Go string literal at the start of `s` (after its opening quote) and
    /// returns its bytes and the rest of `s` after the closing quote.
    fn unquote_go(s: &str) -> (Vec<u8>, &str) {
        let bytes = s.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        let hex = |digits: &[u8]| -> u32 {
            u32::from_str_radix(std::str::from_utf8(digits).expect("hex"), 16).expect("hex")
        };
        loop {
            match bytes[i] {
                b'"' => return (out, &s[i + 1..]),
                b'\\' => {
                    let c = bytes[i + 1];
                    i += 2;
                    let simple = match c {
                        b'a' => Some(7),
                        b'b' => Some(8),
                        b'f' => Some(12),
                        b'n' => Some(b'\n'),
                        b'r' => Some(b'\r'),
                        b't' => Some(b'\t'),
                        b'v' => Some(11),
                        b'\\' => Some(b'\\'),
                        b'"' => Some(b'"'),
                        b'\'' => Some(b'\''),
                        _ => None,
                    };
                    if let Some(b) = simple {
                        out.push(b);
                        continue;
                    }
                    match c {
                        b'x' => {
                            out.push(hex(&bytes[i..i + 2]) as u8);
                            i += 2;
                        }
                        b'u' | b'U' => {
                            let n = if c == b'u' { 4 } else { 8 };
                            let ch = char::from_u32(hex(&bytes[i..i + n])).expect("char");
                            let mut buf = [0u8; 4];
                            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                            i += n;
                        }
                        b'0'..=b'7' => {
                            let digits = std::str::from_utf8(&bytes[i - 1..i + 2]).expect("oct");
                            out.push(u8::from_str_radix(digits, 8).expect("octal"));
                            i += 2;
                        }
                        other => panic!("unknown escape \\{}", other as char),
                    }
                }
                b => {
                    out.push(b);
                    i += 1;
                }
            }
        }
    }

    #[test]
    #[ignore = "needs LUUNA_ATLAS_REFERENCE"]
    fn matches_go_source() {
        let reference = std::env::var_os("LUUNA_ATLAS_REFERENCE")
            .expect("LUUNA_ATLAS_REFERENCE names the Go reference clone");
        let path = std::path::Path::new(&reference)
            .join("internal")
            .join("atlas")
            .join("format_assets.go");
        let source = std::fs::read_to_string(&path).expect("read format_assets.go");
        let mut seen = Vec::new();
        for line in source.lines() {
            let Some(rest) = line.trim_start().strip_prefix('"') else {
                continue;
            };
            let (key, rest) = unquote_go(rest);
            let key = String::from_utf8(key).expect("key is UTF-8");
            let rest = rest
                .trim_start()
                .strip_prefix(':')
                .expect("colon")
                .trim_start();
            let value = rest.strip_prefix('"').expect("a double-quoted literal");
            let (value, tail) = unquote_go(value);
            assert_eq!(tail.trim(), ",", "one literal per line for {key}");
            assert_eq!(get(&key).as_bytes(), &value[..], "asset {key}");
            seen.push(key);
        }
        assert_eq!(seen, KEYS.to_vec());
    }
}
