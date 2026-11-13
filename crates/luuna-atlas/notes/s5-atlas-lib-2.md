# Notes for s5-atlas-lib-2: record, edit, delete, search, show, renderers and the building blocks

Copied from the plan s5-atlas-lib-slots (plan.md) so that this plan starts from them. Paths below are under `crates/luuna-atlas/`.

## Ownership

- Owns: `src/actions/**` (record, edit, delete, search, show, the API typed operations, and the human renderers in `src/actions/render.rs`); the building blocks `src/options.rs`, `src/git.rs`, `src/hooks.rs`, `src/schema.rs`, `src/records.rs`, `src/timefmt.rs`, `src/evidence.rs`, `src/style.rs`, `src/assets.rs`, `src/confirm.rs`; `tests/parity_runtime_actions.rs`, `tests/fixtures/s5-atlas-lib-2/` and this file.
- Foundation files it may still edit: none. `json_parse_error`, `write_file_atomic` and `rename_replacing` are `pub(crate)` after the slots plan.
- Needs on main: the foundation and the slots plan only.

## Ownership notes

- `json_parse_error`, `write_file_atomic` and `rename_replacing` in `src/store.rs` are `pub(crate)` after the slots plan; `store.rs` stays frozen.
- `libc` is declared (unix only); `regex` is not. Hand-write every Go regexp.
- `-2` reads config through `config::read_config` and so keeps the indented first key refusal (GDL1).
- The slots plan's list of statements in the -2 plan that it supersedes is in its plan.md, section 'Corrections s5-atlas-lib-2 needs when its base is set'.

## Assumptions that name this plan

- AS13: human output and colour are in scope; `style.rs` is ported faithfully (it ignores `NO_COLOR` as Go does) together with the renderers in `src/actions/render.rs`.
- AS17: hooks (`sh -c`, setsid, group SIGKILL, diagnostics) are ported faithfully in `hooks.rs`, using `libc`. Tests never reach the network.

## Go data loss (A8)

None of this plan's own cases is known. This plan inherits GDL1 through `config::read_config` and must not add a second YAML reader that bypasses it.

GDL1 (observed): a `config.yaml` whose first key is indented and a later key is less indented (the indented first key case, for example `  b: 1` then `a: 2`). Go's yaml.v3 reads the later key as a second document, so `config show` prints the config without `a` and `add` rewrites the file without `a: 2`. The port refuses with `unsupported YAML at line 2: content after the end of the document is not supported` (`yaml.rs:399-404`), exits 1 and leaves the file untouched.

Comments on rewrite (decided for the owner, AS19 confirmed; orchestration ledger, needs-owner-wave-2.md, decided item 3): loss of comments alone on a rewrite of `config.yaml` is copied from Go and is listed in the hand-off as a known loss. Loss of keys, values, documents, lines, records or body text is refused with the file untouched.

Every other Go behaviour is still copied (A4 differences), suspected defects included. Only the cases named here are deliberate differences from Go.

## Carried from the foundation reviews

- CF1: `json::Value` derives `Debug` and `PartialEq` (`json.rs:46`), both recursive per nesting level, and `Integer(100) != Number(100.0)`. No `{:?}` and no `==` on Values from user data in non-test code; compare numbers as Go does. In particular a test must not assume `Integer(100)` equals `Number(100.0)`.
- CF2: config and file writes follow symlinks as Go's `os.WriteFile` does (`config.rs:279`, `commands.rs:219`). Copy it, do not fix it, list it in the hand-off as a suspected Go defect.
- CF5: Go's JSONPretty prints an empty line with exit 0 past 10000 levels; the crate copies it (`output.rs:198-199`). Keep copying and list it in the hand-off as a suspected Go defect.
- CF6: Go's `Number()` returns 0 for a uint64, so a port of Number must not reuse `as_f64` unchanged. Applies to `records::number` and `number_default`.

## Building blocks (s5-atlas-lib-2a)

Plan s5-atlas-lib-2a built the ten blocks (`options`, `git`, `hooks`, `schema`, `records` with `records/unicode_tables.rs` and `records/rewrite_guard.rs`, `timefmt`, `evidence`, `style`, `assets`, `confirm`) and nothing under `src/actions/`. s5-atlas-lib-2b builds the commands on them. Paths are under `crates/luuna-atlas/`.

### API for later plans

| Rust | Go | Used by |
| --- | --- | --- |
| `options::{GlobalOptions{json,quiet,verbose,allow_unknown_types,allow_domain_mismatch,format}, ::prescan(&[String]), .set(&str,&[String])}` | Run pre-scan, setGlobal | 2b, -3, -4, -5 |
| `options::Env::{from_process(), fixed(vars,args,stdout_is_terminal), var(&str), look_path(&str)->Option<PathBuf>}`; `options::Context{root,cwd,env,globals}`, `Context::new(root)` | App, exec.LookPath | all |
| `git::{git(cwd,&Env,&[&str])->Result<String>, is_git_repo, get_current_commit, get_changed_files, get_context_files, get_active_files, git_common_dir, is_inside_worktree, resolve_store_root, find_repo_root, file_matches_any, file_lives_under_dir, normalize_dir_anchor(s), infer_dir_anchors, has_file_anchors, matches_file_anchors, filter_by_context}` | git.go, config.go | 2b, -3, -4, -5 |
| `hooks::run_hooks(&Context, Option<&Config>, event, &Value, stderr:&mut String)->Result<HookResult>` | RunHooks | 2b, -3, -4 |
| `schema::{validate_record, validate_record_value, validate_json_schema, record_variant_schema, issues_text, get_allowed_types, get_required_fields, find_missing_domain_fields, find_incompatible_required_fields, validate_domain_record}` | schema.go | 2b, -3, -4, -5 |
| `records::{find_duplicate, normalize_text, record_body_text, extract_record_text, tokenize, search_records, confirmation_score, success_count, failure_count, total_applications, success_rate, apply_confirmation_boost, sort_by_confirmation_score, find_near_duplicates, has_rule_signal, check_record_quality, record_summary, truncate, format_js, filter_by_*, obj, list, any_list, record_list, number, number_default, js_lower, go_lower, split_csv, unique, contains}` | store.go, search.go, format.go, model.go | 2b, -3, -4, -5 |
| `records::check_lossless_rewrite(path,&Registry,replaced:&[usize])->Result<()>` | (A8, no Go counterpart) | 2b, -3 |
| `timefmt::{now_iso, iso, parse_date, format_time_ago, file_mod_time}`; `evidence::{evidence_missions, resolve_missions_dir, build_record_evidence}` | model.go, format.go, legacy_layout.go | 2b, -3, -4, -5 |
| `style::{color_level, style, error_line, warning_line, stdout_is_terminal}`; `assets::{get, KEYS}`; `confirm::confirm(prompt,&mut dyn Read,&mut String)->bool` | format_color*.go, format_assets.go, writeConfirm | 2b, -3, -4, -5 |

Rule: every later rewrite of an expertise file (edit, delete, record upsert, bulk, API edit/append/replace, and -3's moves) calls `records::check_lossless_rewrite(path, &registry, &replaced)` inside the lock, before `store::write_records`. `replaced` holds the 0-based indexes of kept records that the command deletes or replaces on purpose, so the guard skips them. Reads (search, show) never call it.

### Guard behaviour as built (C11)

1. A read failure other than a missing file is an error (`AtlasError::Io`), not Ok: `read_records` reads an unreadable file as empty, so a rewrite would wipe it. A missing file is Ok.
2. A line that is not valid JSON is skipped by the guard; `read_records` refuses it first.
3. Keys and field names in the refusal messages are JSON-quoted; ordinary keys print exactly as the messages below.
4. An alias that names its own canonical field is refused, because `apply_aliases` would delete the field.

The duplicate-key scan is iterative (no recursion per nesting level) and accepts lines up to the parser's 10000 levels. The check does not touch the file (bytes and mtime unchanged, no .lock or .tmp file).

### Go data loss (A8)

A rewrite of an expertise file re-encodes every line Go read. Where that loses data, the port refuses before writing; the file stays byte for byte as it was. 2b's parity tests run Go on each case to confirm Go loses the value and check the Rust file is byte-identical afterwards.

- GDL3 (observed in code, `json.rs:25-26`): a line repeats a key at any depth; Go keeps the last value. Refused: `Refusing to rewrite <path>: line <N> repeats the key "<key>", and rewriting would keep only its last value. The file is unchanged.`
- GDL4 (observed in code, `store.rs:115-135`): a legacy `outcome` that is not an object or holds a key beyond status, duration, test_results, agent. Refused: `Refusing to rewrite <path>: line <N> has a legacy "outcome" that cannot be converted to "outcomes" without losing data. The file is unchanged.` A legacy outcome holding only those four keys converts losslessly and is copied.
- GDL5 (observed in code, `registry.rs:575-595`): a legacy alias present while the canonical field holds a different non-empty value. Refused: `Refusing to rewrite <path>: line <N> has "<legacy>", an old name of "<canonical>", whose value differs from "<canonical>" and would be dropped. The file is unchanged.` Equal values, or an alias filling an absent, null or empty canonical field, are copied.
- GDL1 is inherited through `config::read_config` (see above); hooks read config only through it.

Considered and copied (each goes in the hand-off):

- K1: comment and blank lines are dropped by every rewrite (decided item 3; a blank line carries no data, P23). Test: 2b `actions_a8_comments_and_blank_lines_dropped_as_go`.
- K2: numbers are rewritten in Go's shortest form (1.0 to 1, 1e2 to 100, a 20-digit integer to its nearest float). Inferred that Go reads back the same float; 2b `actions_a8_number_forms_read_back_same` settles it.
- K3: invalid UTF-8 and unpaired surrogates in stored strings become U+FFFD (`json.rs:26-27`). Most worth a second look. Test: 2b `actions_a8_invalid_utf8_copied`.
- K4: record upsert of a named type (`record.go:540-546`) and API recordExpertise replace (`api.go:54-86`) replace the stored record; fields the new one lacks are gone (P24). Listed as a suspected defect.
- K5: duplicate keys or 20-digit integers in bulk stdin or a hook's stdout are reduced as Go parses them, then written.
- K6: edit ignores flags for fields the stored type lacks.
- K7: a JSON array row reads as an empty record and `write_records` refuses it with `Unknown record type: ` before writing (`store.rs:44-50, 140-150`); Go also refuses.
- K8: flag values with invalid UTF-8 from argv are not reachable in this plan (typed options are Rust strings). -5's parser must refuse non-UTF-8 values or show Go reads them back the same.

### Differences from Go (A4)

- D1: the store root is a parameter; `resolve_store_root` is exported.
- D2: cwd and environment are passed in (`Context`, `Env`).
- D3: `color_level` reads the argv in Env, not os.Args.
- D4: `GODEBUG=execerrdot=0` does not re-enable running a program found through `.`.
- D5: no Windows hook kill (taskkill) and no console colour detection; on non-unix the hook child alone is killed on timeout.
- D6 (C3): `parse_date` with an RFC1123 zone abbreviation (for example EST): Go consults the process's local time zone; the port takes the wall-clock time as UTC, which is Go's result when the local zone does not know the name.
- D7 (C4): `go_lower` / `js_lower` Unicode version (P12): 28 characters (U+A7CE, U+A7D2, U+A7D4, U+16EA0 to U+16EB8) are lowercased by Go 1.27.0 (Unicode 17) but not by Rust 1.90 (Unicode 16). Kept.
- D8 (C5): `find_duplicate` where both records' `type` is an array or object: Go panics; the port returns not equal.
- D9 (C7): `validate_json_schema` on a cyclic schema errors after 1000 steps; Go overflows its stack.
- D10 (C8): `%q` through the foundation's `commands::go_quote` escapes only control characters; Go `strconv.Quote` also escapes non-printable characters (U+00A0, U+00AD, U+200B, unassigned code points). Affects `validate_domain_record`, dir-anchor and unknown-hook-event messages for such input.
- D11 (C10): `Env::look_path` tests the execute bit by mode bits, not `Eaccess(X_OK)`.
- D12 (C10): when hooks read config themselves (no config passed), the read goes through `config::read_config` and does not run Go ReadConfig's legacy `.mulch` migration; it writes nothing.
- Hook timeout arithmetic follows amd64 Go (float to integer conversion of the timeout).

### A6 small corrections

- C1 (t6): the criterion that a grep for the literal `"gh"` in `src` prints nothing contradicts the port: Go's evidence keys include it (`record.go:205,308`; `schema.go:47`). Followed the criterion's purpose, no start of a GitHub CLI process. The literal appears only as an evidence key (`evidence.rs`, `records.rs`, `schema.rs`); no `Command::new`, `look_path` or argument vector names that program.
- C2 (t2): `confirm` reads as Go's `bufio.Reader.ReadString('\n')` (`delete.go:9-14`): it reads past 4096 bytes until a newline or EOF, ignores the read error and uses the partial line. The criterion said one 4096-byte chunk and false on EOF or error. Two tests added in `confirm.rs`.
- C9 (t6): the git child gets Env's variables plus `PWD=<abs cwd>`, as Go's os/exec adds it when Dir is set and Env is inherited. Hooks set Env explicitly in Go, so no PWD there.

### Suspected defects kept (Go behaviour copied)

- Hook timeouts print with `%g` (1000000 prints as `1e+06ms`); fractional timeouts are truncated.
- Background children of a hook survive a normal hook exit; hook output is unbounded.
- Hooks inherit every variable of the caller's environment, tokens included, and run config.yaml text through `sh -c`.
- A failed hook start (no sh) is reported as `exited with code 1` with no message.
- The global pre-scan also matches flag values.
- `confirm` writes its prompt to stdout even with `--json`, and a second call drops buffered input.
- `FORCE_COLOR=-1` is not clamped (inferred).
- git output is read without `-z`, so non-ASCII paths come back quoted (inferred); git errors are swallowed.
- A crafted `.git` can redirect the store root (`resolve_store_root`).

### Carried findings as they apply

- CF1: no `==`, `{:?}` or derived comparison on user Values in non-test code; numbers compare as Go does. A test must not assume `Integer(100)` equals `Number(100.0)`.
- CF2: this plan writes no store file. For 2b and -3: appends follow a symlinked expertise file, atomic rewrites replace the link, symlinked directories are followed. Copied, listed.
- CF5: nothing here prints through `output.rs`. For 2b: Go's JSONPretty prints an empty line with exit 0 past 10000 levels, and the crate copies it; list it as a suspected Go defect.
- CF6: `records::number` and `number_default` give 0 for an Integer above int64 (test `number_of_uint64_is_zero`); hooks' `timeout_ms` and `search.boost_factor` go through them.

### Assumptions by id

P2 human output and colour in scope (`style.rs` ignores `NO_COLOR` as Go does); P3 hooks faithful, no GitHub CLI call in -2; P4 no parser here; P5 version, notices and contract not relevant; P6 store root is a parameter; P7 explicit `Env`; P8 no regex, every Go regexp hand-written; P9 confirm and assets owned by -2; P11 Unicode tables of Go 1.27.0 `unicode.L` and `unicode.N`; P12 `js_lower` is `str::to_lowercase`, `go_lower` lowercases per character and maps U+0130 to `i`; P16 hook diagnostics (hostname from `libc::gethostname`, pid, Unix milliseconds); P18 hostile tests are `hostile_*`, ignored by default, run only under `prlimit`; P19 no derived Debug or PartialEq on user Values; P20 pinned Go used only for the table generator; P21 the split into 2a and 2b; P22 guard placement (inside the lock, kept records only); P23 blank lines and invalid UTF-8 copied; P24 upsert replace is not A8; P25 git and sh found through Env's PATH.

### Points for later plans

- C6: `registry::go_text` (frozen) recurses per nested array; 2a code avoids it on user arrays. 2b and -3 must not call it on user-supplied nested arrays without a depth bound.
- C5 (D8): 2b decides how the `record` command reports two records whose `type` is an array or object (Go panics; `find_duplicate` returns not equal).
- C8 (D10): `%q` printability is not fixed in 2a; messages that quote user text through `go_quote` differ from Go for non-printable characters (2b, -5).
- `git::git` drops stdout on failure; a caller that needs the stdout of a failing command (the -4 case) needs a variant.
- Hooks need the config passed for a store not yet migrated (D12): the caller reads it and passes `Some(&Config)`, so the legacy `.mulch` migration is not skipped.
- `validate_record` on an array row: 2b keeps the raw value and calls `validate_record_value`.
