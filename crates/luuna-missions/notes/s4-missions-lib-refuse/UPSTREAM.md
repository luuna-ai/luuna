# Foundation data-loss refusals

Charter A8 requires refusal before silent data loss; A10 permits the two listed test changes while preserving the Go loss evidence. The shared store, model, JSON code and comparison harness stay unchanged. Reference: luuna-ai/luuna-missions at 2fbbf8e5b6a238ea47543e1dad112fe1716faec4, Go 1.27.

## Deliberate differences (charter A8, Go data loss), DL100 to DL199

| Id | Command | Loss and native handling | Text | Unit check | Go evidence |
| --- | --- | --- | --- | --- | --- |
| DL100 | update | file level (a): a line Go cannot read: `{not json`, `[1]`, `null`, or another record whose extensions hold 1e999; refuse before writes. | a | command::update::tests::a_lossy_issues_file_is_refused_unchanged | `<reference-clone>/internal/cli/issue_mutate.go`; refuse::update_refusals_where_go_loses_data cases a-not-json, a-array, a-null, a-1e999-other; also format_roundtrip::unchanged_records_rewrite_byte_identical 'full rewrite by update' (A10 hunk a) |
| DL101 | update | file level (b): two records with one id (the first one's bytes leave the file; Go's read-back of the id is the later record both times); refuse before writes. | b | command::update::tests::a_lossy_issues_file_is_refused_unchanged | `<reference-clone>/internal/cli/issue_mutate.go`; refuse::update_refusals_where_go_loses_data case b-same-id |
| DL102 | update | file level (b): two records without an id (all but the last are lost); refuse before writes. | b_without_id | command::update::tests::a_lossy_issues_file_is_refused_unchanged | `<reference-clone>/internal/cli/issue_mutate.go`; refuse::update_refusals_where_go_loses_data case b-no-id |
| DL103 | update | file level (c): a top-level key repeated in any record, another ticket's or the command's own (assumption RA4); refuse before writes. | c | command::update::tests::a_lossy_issues_file_is_refused_unchanged | `<reference-clone>/internal/cli/issue_mutate.go`; refuse::update_refusals_where_go_loses_data cases c-other, c-own |
| DL104 | update --add-label | field (d): labels a string, number or object; refuse before writes. | d with field labels | command::update::tests::labels_go_cannot_rewrite_are_refused_unchanged | `<reference-clone>/internal/cli/issue_mutate.go`; refuse::update_refusals_where_go_loses_data cases d-labels-string, d-labels-number, d-labels-object |
| DL105 | update --add-label, --remove-label | field (e): a number, object or null element in labels, when the list changes; refuse before writes. | e with field labels | command::update::tests::labels_go_cannot_rewrite_are_refused_unchanged | `<reference-clone>/internal/cli/issue_mutate.go`; refuse::update_refusals_where_go_loses_data cases e-labels-number-add, e-labels-number-remove, e-labels-object-add, e-labels-null-add |
| DL106 | update --add-label, --remove-label | field (f): invalid UTF-8 or a lone surrogate escape in a labels element, when the list changes; refuse before writes. | f with field labels | command::update::tests::labels_go_cannot_rewrite_are_refused_unchanged | `<reference-clone>/internal/cli/issue_mutate.go`; refuse::update_refusals_where_go_loses_data cases f-labels-byte-add, f-labels-byte-remove, f-labels-surrogate-add |
| DL107 | update --extensions | field (g): extensions a string, array or number; refuse before writes. | g | command::update::tests::extensions_go_cannot_rewrite_are_refused_unchanged | `<reference-clone>/internal/cli/issue_mutate.go`; refuse::update_refusals_where_go_loses_data cases g-string, g-array, g-number |
| DL108 | update --extensions | field (h): invalid UTF-8 or a lone surrogate escape in a key or string of extensions, at any depth; refuse before writes. | h | command::update::tests::extensions_go_cannot_rewrite_are_refused_unchanged | `<reference-clone>/internal/cli/issue_mutate.go`; refuse::update_refusals_where_go_loses_data cases h-value-byte, h-key-byte, h-value-surrogate, h-key-surrogate |
| DL109 | update --extensions | field (i): a number whose value Go's float64 re-encoding changes, at any depth (9007199254740993 -> 9007199254740992); refuse before writes. | i | command::update::tests::extensions_go_cannot_rewrite_are_refused_unchanged | `<reference-clone>/internal/cli/issue_mutate.go`; refuse::update_refusals_where_go_loses_data cases i-top, i-nested |
| DL110 | update --extensions | field (j): a key repeated inside extensions, at any depth (Go keeps the last value); refuse before writes. | j | command::update::tests::extensions_go_cannot_rewrite_are_refused_unchanged | `<reference-clone>/internal/cli/issue_mutate.go`; refuse::update_refusals_where_go_loses_data case j-top (nested repeated keys are covered by the unit tests; the rule is the same scan) |
| DL111 | block | file level (a), as DL100; refuse before writes. | a | command::block::tests::a_lossy_issues_file_is_refused_unchanged | `<reference-clone>/internal/cli/issue_links.go`; refuse::block_refusals_where_go_loses_data cases a-not-json, a-array, a-null, a-1e999-other; also format_roundtrip::unchanged_records_rewrite_byte_identical 'full rewrite by block' (A10 hunk a) |
| DL112 | block | file level (b), as DL101; refuse before writes. | b | command::block::tests::a_lossy_issues_file_is_refused_unchanged | `<reference-clone>/internal/cli/issue_links.go`; refuse::block_refusals_where_go_loses_data case b-same-id |
| DL113 | block | file level (b) without id, as DL102; refuse before writes. | b_without_id | command::block::tests::a_lossy_issues_file_is_refused_unchanged | `<reference-clone>/internal/cli/issue_links.go`; refuse::block_refusals_where_go_loses_data case b-no-id |
| DL114 | block | file level (c), as DL103 (another record, or the issue's own); refuse before writes. | c | command::block::tests::a_lossy_issues_file_is_refused_unchanged | `<reference-clone>/internal/cli/issue_links.go`; refuse::block_refusals_where_go_loses_data cases c-other, c-own |
| DL115 | block | field (d): the issue's blockedBy or the blocker's blocks is a number, object or string; refuse before writes. | d with field blockedBy or blocks | command::block::tests::lists_go_cannot_rewrite_are_refused_unchanged | `<reference-clone>/internal/cli/issue_links.go`; refuse::block_refusals_where_go_loses_data cases d-by-number, d-by-object, d-by-string, d-blocks-number, d-blocks-object, d-blocks-string |
| DL116 | block | field (e): a number or null element in blockedBy or blocks; refuse before writes. | e with field blockedBy or blocks | command::block::tests::lists_go_cannot_rewrite_are_refused_unchanged | `<reference-clone>/internal/cli/issue_links.go`; refuse::block_refusals_where_go_loses_data cases e-by-number, e-by-null, e-blocks-number, e-blocks-null (each runs Go and asserts its loss) |
| DL117 | block | field (f): invalid UTF-8 or a lone surrogate escape in a blockedBy or blocks element; refuse before writes. | f with field blockedBy or blocks | command::block::tests::lists_go_cannot_rewrite_are_refused_unchanged | `<reference-clone>/internal/cli/issue_links.go`; refuse::block_refusals_where_go_loses_data cases f-by-byte, f-by-surrogate, f-blocks-byte, f-blocks-surrogate |
| DL118 | init | no config.yaml and issues.jsonl holds data: Go empties it (SD22); refuse before writes. | k with file issues.jsonl | command::init::tests::init_without_config_yaml_refuses_to_empty_the_data_files (A10 hunk b) | `<reference-clone>/internal/config/config.go`; refuse::init_refusals_where_go_loses_data case issues |
| DL119 | init | no config.yaml and templates.jsonl holds data; refuse before writes. | k with file templates.jsonl | command::init::tests::init_refuses_each_file_go_would_empty_or_replace_unchanged | `<reference-clone>/internal/config/config.go`; refuse::init_refusals_where_go_loses_data case templates |
| DL120 | init | no config.yaml and plans.jsonl holds data; refuse before writes. | k with file plans.jsonl | command::init::tests::init_refuses_each_file_go_would_empty_or_replace_unchanged | `<reference-clone>/internal/config/config.go`; refuse::init_refusals_where_go_loses_data case plans |
| DL121 | init | no config.yaml and .luuna/missions/.gitignore holds an entry other than *.lock: Go replaces it with `*.lock\n`; refuse before writes. | l | command::init::tests::init_refuses_each_file_go_would_empty_or_replace_unchanged | `<reference-clone>/internal/config/config.go`; refuse::init_refusals_where_go_loses_data case gitignore |
| DL122 | init | the same losses through a symbolic link: issues.jsonl linked to an outside file with data, .gitignore linked to an outside file with other text, .luuna/missions linked to an outside folder holding issues.jsonl with data (SD27); refuse before writes. | k or l, naming the path inside the project | command::init::tests::init_refuses_through_symbolic_links_unchanged (unix) | `<reference-clone>/internal/config/config.go`; refuse::init_refusals_where_go_loses_data cases linked-issues, linked-gitignore, linked-folder (the outside file or folder lives in a second TempProject under std::env::temp_dir()) |
| DL123 | init | Nonempty regular data/.gitignore file readable metadata but read permission denied: Go still truncates/replaces it successfully; refuse before writes. | m | command::init::tests::init_refuses_each_file_go_would_empty_or_replace_unchanged | `<reference-clone>/internal/config/config.go`; refuse::init_refusals_where_go_loses_data write-only variants; original mode asserted before test restores readability to compare whole tree |

## Exact refusal templates

- **a:** Refusing to rewrite issues.jsonl: `line <n> is not a ticket record the Go tool can read, and rewriting the file would drop it. Nothing was written.`
- **b:** Refusing to rewrite issues.jsonl: `lines <m> and <n> both hold the ticket "<id>", and rewriting the file would keep only the last one. Nothing was written.`
- **b_without_id:** Refusing to rewrite issues.jsonl: `lines <m> and <n> both hold a record without an id, and rewriting the file would keep only the last one. Nothing was written.`
- **c:** Refusing to rewrite issues.jsonl: `the record on line <n> repeats the key "<key>", and rewriting the file would keep only its last value. Nothing was written.`
- **d:** Refusing to rewrite issues.jsonl: `line <n> (ticket "<id>") holds a "<field>" value that is not a list of strings, and changing the field would replace that value. Nothing was written.`
- **e:** Refusing to rewrite issues.jsonl: `line <n> (ticket "<id>") holds an element of "<field>" that is not a string, and changing the field would write it as an empty string. Nothing was written.`
- **f:** Refusing to rewrite issues.jsonl: `line <n> (ticket "<id>") holds an element of "<field>" with invalid UTF-8 or a lone surrogate escape, and changing the field would write it as U+FFFD. Nothing was written.`
- **g:** Refusing to rewrite issues.jsonl: `line <n> (ticket "<id>") holds an "extensions" value that is not an object, and changing the field would replace that value. Nothing was written.`
- **h:** Refusing to rewrite issues.jsonl: `line <n> (ticket "<id>") holds a key or string in "extensions" with invalid UTF-8 or a lone surrogate escape, and changing the field would write it as U+FFFD. Nothing was written.`
- **i:** Refusing to rewrite issues.jsonl: `line <n> (ticket "<id>") holds the number <token> in "extensions", and changing the field would write it as <go text>. Nothing was written.`
- **j:** Refusing to rewrite issues.jsonl: `line <n> (ticket "<id>") holds an object in "extensions" that repeats the key "<key>", and changing the field would keep only its last value. Nothing was written.`
- **k:** Refusing to initialize: `line <n> of .luuna/missions/<file> holds data and config.yaml is missing, and initializing would empty the file. Nothing was written.`
- **l:** Refusing to initialize: `line <n> of .luuna/missions/.gitignore holds an entry other than *.lock, and initializing would replace the file with *.lock alone. Nothing was written.`
- **m:** Refusing to initialize: `.luuna/missions/<file> is nonempty but cannot be read while config.yaml is missing, so initializing could overwrite unchecked user data. Nothing was written.`

## Copied, Go reads the same value back

| Case | Reason |
| --- | --- |
| update --add-label on labels null: Go writes ["zz"], read-back same | null holds no data (decided item 3 readings); exactly representable |
| update --add-label on ["a","a"]: the duplicate collapses to ["a","zz"] | As s4-missions-lib-2 copies it (plan.json:403, changed_lists_go_decodes_exactly_pass holds ["a","a"]), passed by its auditor and reported to the owner (needs-owner-wave-2.md:22). Consistency across Missions commands (assumption RA3). |
| update --remove-label that leaves the decoded list unchanged (an absent label; labels not a list; null) and --remove-label a on ["a","a"] | Go keeps the raw bytes or the read-back is the intended change; not changed means not checked |
| update with a flag that does not touch labels or extensions (--title) on labels ["a",5] or extensions with 9007199254740993 | Go keeps the raw bytes of an unchanged member |
| update --extensions on extensions null, {"a":null}, {"n":1.50}, {"n":1e21}, {"a":"<&>"}, {"b":1,"a":2} | Decision 6: Go's read-back gives the same value (1.5, 1e+21, re-escaped <&>, sorted keys); precedent 'For your attention' needs-owner-wave-2.md:21 |
| update --extensions on a target whose extensions hold 1e999: Go exits 1 `Issue not found: <id>` and writes nothing | The port's Ticket::from_json returns None for that line (model.rs:122-137), so read_tickets skips it and update.rs:102-104 answers the same `Issue not found` before the refusal check runs; nothing is written. Covered by command::update::tests::extensions_go_cannot_read_answer_issue_not_found and refuse::copied_cases_match_go case update-target-1e999. |
| --set-labels, --clear-extensions, --intent and the scalar flags | They replace the field by design; not a case (as s4-missions-lib-2 DC16) |
| block when blockedBy or blocks is null, or already holds the id, or holds a duplicate that collapses ([C,C] -> [C,B]) | null holds no data; an unchanged list is kept byte for byte; the duplicate collapse as for labels (assumption RA3) |
| update and block on a file with one record without an id, a numeric id, a record written with spaces, blank or whitespace-only lines, or invalid UTF-8 in an unchanged record | Go's read-back is the same before and after (zero-value fields added, blank lines dropped, the unchanged record kept byte for byte); SD100 lists the added fields |
| init with whitespace-only data files, three empty data files, or .gitignore exactly `*.lock\n` | Whitespace is not data (decided item 3 readings); nothing is lost |
| init with a valid config.yaml, or config.yaml a link to a valid file | Go writes only .gitattributes; the check does not run (`already` is true) |
| init appending the merge=union lines to .gitattributes through a link, or after text without a final newline | The content is kept and extended; nothing is lost (SD27 for the link stays copied) |
| init with config.yaml a dangling link and no data: Go exits 1 `open <p>/.luuna/missions/config.yaml: no such file or directory` and writes nothing | No data, so the check passes and config::init runs as at the base, failing at the same write. What the port prints there is unchanged by this plan (see risks R5). With data in the folder as well, the check runs first and refuses (k) or (l): both exit 1 and write nothing; the message differs (assumption RA9). |
| init in an empty folder | Control |
| show and list on the same lossy files | Read-only commands are unchanged (out of scope) |

Checks return exit one with the ordinary JSON error envelope for the literal --json flag, or Error: followed by the text on stderr. Request validation precedes the refusal. Existing locks remain; the issue guard is the last statement before persistence. Unchanged raw fields are retained, and explicit set/clear replacements remain Go behavior.

The helper reuses lib-2's pure whole-file scanner for consistent messages and order. File-level problems precede field problems. Fields are checked by source line, then blocks, blockedBy, labels, extensions. Extensions are scanned iteratively in document order. Exact UTF-8/surrogate checks and normalized decimal comparison protect retained data; Go-encoded before/after maps decide whether the field actually changed.

DL123 corrects the original unreadable-file assumption: Go can overwrite a nonempty mode-0200 file even though reading it fails. Refuse nonempty regular files whose contents cannot be checked. Production never changes permissions. Tests assert the original mode before restoring test-only readability for complete byte/tree comparison. Directories retain the earlier error path, and empty unreadable files contain no data.

Regular init files are scanned in 8 KiB chunks with at most four bytes of UTF-8 state and a six-byte ignore token. Tests stream a 16 MiB single-line fixture to target-backed disk. Special files are left to the existing writer: Go and native init both retain a link to /dev/zero and succeed, without preflight reading the device stream.

## Suspected Go defects, SD100 to SD199

| Id | Behavior | Handling and source |
| --- | --- | --- |
| SD100 | A full rewrite adds zero-valued known members to unchanged records that lacked them; read-back stays equal. | Copied; reference internal/model/json.go and the successful comparison controls. |
| SD101 | Clear on a wrongly typed raw extensions value retains it because the decoded map was already nil and raw merge sees no change. | Copied; reference internal/cli/issue_mutate.go and internal/model/json.go; update control test. |

Additional controls confirm that clear plus patch is rejected as mutually exclusive, zero with a large exponent stays zero, and an unchanged extensions patch retains raw precision. Copied symlink comparisons bind only the known outside-project root and separately assert that both original link targets stay unchanged. Shared comparison support remains frozen.

## Effect on foundation rows

SD1 and SD2 stay true for reading, read-only commands and create's append. Update and block refuse malformed lines, repeated ids including multiple id-less records, and repeated root keys (DL100-DL103, DL111-DL114). SD22 no longer occurs when data files hold data (DL118-DL120, DL123); init still empties whitespace-only files. SD27 is refused where links would lose data (DL122, DL123), while attribute-file link appends remain copied. SD25 is unchanged for the supplied patch itself; retained existing extensions are protected by DL107-DL110. Original foundation rows stay unchanged.

The inherited library I/O contract for a dangling config link with no data remains Error::Io; Go prints its own command error wording. Its no-write result is verified. This plan does not edit frozen config or error code.

## Test changes under A10

The format-roundtrip rewrite cases previously called unchanged_lines for Rust and pair.differences expecting Go's lossy result. Two insertions retain every original line, still prove that Go exits zero and drops malformed line four, and replace only those two cases with refused_unchanged: exact error, exit one, and the original whole tree including modes. The create case stays unchanged.

The old init assertions were `text(&outcome.stdout).starts_with("✓ Initialized")` and an empty issues-file result. The listed replacement requires exit one, empty stdout, exact refusal stderr, unchanged original bytes and mode, only the original issues file, and no root attributes file. Only the named test's original lines 209-219 may be removed. All other init/update/block changes are insertions. The frozen store test demonstrating lower-level malformed-line loss remains unchanged.

Run `cargo test --locked -p luuna-missions --test refuse -- --ignored` with LUUNA_MISSIONS_REFERENCE, LUUNA_MISSIONS_GO and LUUNA_MISSIONS_GOPROXY set to the reference clone, pinned Go and proxy list. All writes use disposable projects. Compile hostile tests first and run with the host memory/time limits. Native macOS evidence belongs to the later CI plan; it is not inferred from Linux checks.

A10 assertion excerpts (the source retains the complete listed checks):

Old rewrite expectations:
```rust
differences.extend(unchanged_lines("rust", &rust_issues, kept));
differences.extend(pair.differences(creates));
```

New rewrite expectations:
```rust
differences.extend(refused_unchanged(&pair, &original, &command));
report.case(case, serde_json::Map::new(), differences);
continue;
```

Old init expectations:
```rust
assert!(text(&outcome.stdout).starts_with("✓ Initialized"));
assert_eq!(fs::read(dir.join("issues.jsonl")).unwrap(), b"");
```

New init expectations include:
```rust
assert_eq!(outcome.exit_code, 1);
assert!(outcome.stdout.is_empty());
assert_eq!(fs::read(dir.join("issues.jsonl")).unwrap(), b"{\"id\":\"x\"}\n");
assert_eq!(mode(&dir.join("issues.jsonl")), before);
assert!(!temp.path().join(".gitattributes").exists());
```
