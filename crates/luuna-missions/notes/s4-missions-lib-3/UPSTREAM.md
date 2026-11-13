# Upstream notes of s4-missions-lib-3

## Suspected Go defects found by s4-missions-lib-3

| Id | Behaviour | Evidence | Ported in |
| --- | --- | --- | --- |
| SD300 | A section kind given as the string 'object' is refused although the message says 'expected text / list / steps / object'. | `internal/config/templates.go:219-232` | copied |
| SD301 | With several bad templates or sections, which error LoadPlanTemplates reports depends on Go's random map order. | `internal/config/templates.go:119, :129, :246` | sorted order (A304) |
| SD302 | The config emitter writes plain strings such as True, Null, 0x10, 1e3, .inf, '- a', which read back as other types or as invalid YAML (extends SD20). | `internal/config/tree.go:280-311; yaml.v3 resolve.go:42-60, :124-265` | refused as DL306 for the values Go reads back differently; copied for those it reads back the same (yes, on, no, off and the like) |
| SD303 | Key order for a sequence nested in a sequence is looked up under the outer sequence's path. | `internal/config/tree.go:253` | copied |
| SD304 | migrate-from-beads appends two beads records with the same id, does not count lines it cannot parse, and reports skipped 0 for records whose id exists. | `internal/cli/migrate.go:102-136; fixture 148` | copied |
| SD305 | onboard rewrites legacy '<!-- seeds' markers in the whole file when it updates the section, but not when the section is current. | `internal/cli/onboard.go:34-57` | copied |
| SD306 | guard denies any path ending in .luuna/missions/<file>.jsonl, in any project, without resolving symbolic links. | `internal/cli/standalone.go:51-67` | copied |
| SD307 | setup rewrites .claude/settings.json with keys sorted, <, > and & escaped, and numbers through float64. | `internal/cli/setup.go:150-211` | key order, escaping and the number part copied (numbers: Go's only reader reads the same value, W1 and W4, logs/go-first-1.txt:117, :120; A320); invalid UTF-8 and unpaired surrogates written as U+FFFD also copied (W5-W8, :121-124); a repeated key is DL307 (W2, W3, A322) |
| SD308 | sync trims git status's stderr in its error but not git add's or git commit's. | `internal/cli/sync.go:50, :102, :108` | copied |
| SD309 | tpl parses options before choosing the subcommand. | `internal/cli/template.go:13-38` | copied |
| SD310 | prime discards every error of loading its state and prints state null. | `internal/cli/prime.go:206` | copied except Error::Unsupported (A307) |

The inherited SD11 sorted-map doctor rewrite, SD12 whole-index sync commit, and SD16 fixed upgrade refusal are copied. SD20 read-back loss is refused under DL306.

## Deliberate differences from Go (charter A8, Go data loss)

All refusals precede writes and preserve original bytes and mode.

### DL300: tpl step add

Go: `internal/cli/template.go:101-118 (WriteTemplates); internal/store/store.go:116-156`. Loss: templates.jsonl lines readRecords skips, earlier records with a repeated id, earlier values of a repeated key (a repeated key is a dropped key, A322). The existing steps Go replaces are DL308; existing steps that are objects are kept byte for byte by Go (A0, A4-A8, logs/go-first-1.txt:46, :50-54) and by the port (t6)

Refusal: `Refusing to rewrite <path>: line <n> <is not a record the tool can read | repeats the id "<id>" of line <m> | repeats the key "<key>">, and the Go tool would drop it without a message. Nothing was written.`. Test: `command::project::tpl::tests::step_add_refuses_a_lossy_rewrite_and_the_file_is_unchanged`.

### DL301: tpl pour

Go: `internal/cli/template.go:227-259 (WriteIssues)`. Loss: as DL300, in issues.jsonl

Refusal: `as DL300`. Test: `command::project::tpl::tests::pour_refuses_a_lossy_rewrite_and_the_file_is_unchanged`.

### DL302: migrate-from-beads

Go: `internal/cli/migrate.go:122-137 (WriteIssues, also when nothing is added)`. Loss: as DL300, in issues.jsonl

Refusal: `as DL300`. Test: `command::project::migrate_beads::tests::migrate_refuses_a_lossy_rewrite_and_the_file_is_unchanged`.

### DL303: doctor --fix

Go: `internal/cli/doctor.go:128-147 (decode into any, no UseNumber, :143), :615-637 (doctorDedupObjects skips non-objects and non-string ids, :620-628), :639-650 (json.Marshal), :652-693`. Loss: observed (logs/go-first-1.txt:85-95): (i) valid-JSON lines that are not objects (V1), (ii) objects without a string id (V2), (iii) numbers whose float64 value differs (V3, V5; show prints ...993 before and ...992 after, go-first-1-C.txt:521, :683), (iv) earlier members of a repeated key (V4, A322), (v) invalid UTF-8 and unpaired surrogate escapes written as U+FFFD, which Go's show then prints differently (V7-V10, go-first-1-C.txt:1418/:1587, :1650/:1819, :1891/:2051, :2106/:2245; A320). V6 and V10's dropped earlier record is the reported dedupe, copied.

Refusal: `Refusing to fix <path>: line <n> <is not a JSON object | has no string id | holds the number <text>, which the Go tool would write as <go text> | repeats the key "<key>" | holds <invalid UTF-8 | the unpaired escape <escape>> at <key path>, which the Go tool would write as U+FFFD>, and the Go tool would drop or change it without a message. Nothing was written.`. Test: `command::project::doctor_fix::tests::fix_refuses_a_line_that_is_not_an_object_and_changes_nothing, fix_refuses_a_record_without_an_id_and_changes_nothing, fix_refuses_a_number_float64_would_change_and_changes_nothing, fix_refuses_a_repeated_key_and_changes_nothing, fix_refuses_all_four_together_naming_the_first, fix_refuses_invalid_utf8_or_an_unpaired_surrogate_and_changes_nothing`.

### DL304: config set, config unset

Go: `internal/cli/config.go:257-269, :300-313; internal/config/tree.go:35-52; gopkg.in/yaml.v3@v3.0.1/yaml.go:156-173 (one document parsed)`. Loss: every YAML document after the first

Refusal: `Refusing to rewrite <path>: it holds more than one YAML document, and the Go tool would keep only the first. Nothing was written.`. Test: `command::project::config::tests::a_second_document_is_refused_and_the_file_is_unchanged`.

### DL305: config set, config unset

Go: `internal/config/legacy.go:104-123 (mulch_source dropped beside atlas_source, :114-120); tree.go:51`. Loss: the mulch_source value in a mapping that also holds atlas_source

Refusal: `Refusing to rewrite <path>: <dotted path or (root)> holds both mulch_source and atlas_source, and the Go tool would drop mulch_source. Nothing was written.`. Test: `command::project::config::tests::mulch_source_beside_atlas_source_is_refused_and_the_file_is_unchanged`.

### DL306: config set, config unset

Go: `internal/config/tree.go:280-311 (formatKey, formatScalar); gopkg.in/yaml.v3@v3.0.1/resolve.go:42-60, :124-265`. Loss: a string or key written plain that reads back as another type or value (value table in t4)

Refusal: `Refusing to write <path>: the <value|key> at <dotted path> would be written as <emitted text>, which reads back as a different value. Nothing was written.`. Test: `command::project::config::tests::a_value_that_reads_back_differently_is_refused_and_the_file_is_unchanged, a_key_that_reads_back_differently_is_refused_and_the_file_is_unchanged`.

### DL307: setup claude (install and --remove)

Go: `internal/cli/setup.go:150-170 (json.Unmarshal into any, :161-162), :197-211 (MarshalIndent, WriteFile when changed)`. Loss: earlier members of a repeated key (W2, W3; logs/go-first-1.txt:118-119; A322). Numbers through float64 (W1, W4) and U+FFFD (W5-W8) are copied: Go's only reader of the file reads the same value (A320; :117, :120-124).

Refusal: `Refusing to rewrite <path>: it repeats the key "<key>"; fix it by hand, nothing was written`. Test: `command::project::setup::tests::setup_refuses_a_repeated_key_and_the_file_is_unchanged`.

### DL308: tpl step add

Go: `internal/cli/template.go:101-118 (append, WriteTemplates); internal/model/json.go:81-102 (decodeKnownObject ignores a field's decode error), :164-176 (the changed steps re-encoded), :28-30 and :371-375 (a non-object step fails parseObject)`. Loss: the existing steps of the template it appends to when they are not a list (A1 the string "keep me", A2 an object: replaced by the new step alone) or hold an element that is not an object (A3: 5 written as {"title":""}); observed, logs/go-first-1.txt:43, :47-49

Refusal: `Refusing to rewrite <path>: line <n> holds template "<id>" whose steps <is not a list | item <k> is not an object>, and the Go tool would replace it without a message. Nothing was written.`. Test: `command::project::tpl::tests::step_add_refuses_steps_that_are_not_a_list_and_the_file_is_unchanged, step_add_refuses_a_step_that_is_not_an_object_and_the_file_is_unchanged`.

## Known losses copied from Go

- config set and unset drop comments (ruling item 3, needs-owner-wave-2.md:35; slots plan.json:305-309): copied, listed in notes and the hand-off.
- config set and unset write anchors, aliases and merge keys expanded; the decoded values are kept (A314; slots plan.json:259-261).
- migrate-from-beads skips .beads lines it cannot parse and does not carry unmapped fields, but never writes .beads (slots plan.json:296): copied, named in the hand-off.
- doctor --fix's reported dedupe drops the earlier records of a repeated id in templates.jsonl, with the fix line 'Deduplicated templates.jsonl' (V6, V10; logs/go-first-1.txt:68-70, :91, :95): the command's own change, copied (t7 test fix_deduplicates_templates_as_go_does).
- setup claude writes a number whose float64 value differs as the float64's text (W1 install, W4 --remove: 9007199254740993 written as 9007199254740992): Go's only reader of .claude/settings.json reads 9.007199254740992e+15 before and after (logs/go-first-1.txt:117, :120; A320): copied (t9 test setup_writes_large_numbers_through_float64_as_go_does).
- setup claude writes invalid UTF-8 and unpaired surrogate escapes, in values and keys, as U+FFFD (W5-W8): Go's only reader reads "a\ufffd" or the key "\ufffd" before and after (logs/go-first-1.txt:121-124; A320): copied (t9 test setup_writes_invalid_utf8_and_unpaired_surrogates_as_u_fffd_as_go_does).
- setup claude's addition and removal of its two hooks and doctor --fix's removal of a dangling blockedBy are the commands' own changes (go-first-1.txt:68, :106), copied.

## Assets copied from the reference

All five files are unchanged from reference commit `2fbbf8e5b6a238ea47543e1dad112fe1716faec4`; the reference remains read only.

- `prime_compact.json`: `<reference-clone>/internal/cli/prime_compact.json`, SHA-256 `4dbe3e763efabf786702353b3bea1fba9fb22893629da5e69ec6c4312a2a1a64`.
- `prime_compact_verify.json`: `<reference-clone>/internal/cli/prime_compact_verify.json`, SHA-256 `8aafa402b60cfa76adf33a5c61a3b06657f462105edfe62f60a5df65676cad5e`.
- `prime_full.json`: `<reference-clone>/internal/cli/prime_full.json`, SHA-256 `8ca61cb76107a1ad76967300880d9b9224739bbacbba5be5359522e81b3a18d0`.
- `prime_full_verify.json`: `<reference-clone>/internal/cli/prime_full_verify.json`, SHA-256 `5d3db1fe882d0b07964d4237044107e2d42d676fea2db0b8a8babec5122b9b00`.
- `schema.json`: `<reference-clone>/internal/config/schema.json`, SHA-256 `6d6d7c2e1b7f8f585f8486835d24226b069f2227a773492118239da6ab57dc95`.

## Scalar read-back clarification

A literal `<<` used as a scalar value is preserved by the pinned Go tool: `config set project << --json` writes `project: <<`, and `config show --json` returns the string `<<`. It is copied because there is no value loss. This differs from `<<` used as a mapping merge key, where a rewrite must still preserve the key and value or refuse. The emitter read-back tests cover these roles separately.

## Additional observed loss: DL309

`tpl step add` re-encodes the keys of each existing step in the changed template (`internal/model/json.go:125-185`, `:371-392`; reference `2fbbf8e`). A key containing invalid UTF-8 or a lone surrogate escape changes to U+FFFD, whereas the CLI previously returned the raw key inside the unchanged steps. The port refuses before writing: `Refusing to rewrite <path>: line <n> holds template "<id>" whose steps item <k> has a key with invalid UTF-8 or an unpaired surrogate escape, and the Go tool would write it as U+FFFD. Nothing was written.` Test: `command::project::tpl::tests::step_add_refuses_a_key_go_would_reencode_unchanged`, including byte/mode preservation and an unrelated-template control. This extends the original loss list for newly demonstrated A8 loss.

Duplicate keys inside an existing step are DL300 only when that template's steps change. Go retains an unrelated template's raw steps, including repeated step keys; the port keeps that control. Nested unknown values that Go keeps raw remain unchanged.

## Additional observed loss: DL310

`doctor --fix` can overwrite a present non-array, non-null reciprocal `blocks` or `blockedBy` value while reporting only an added bidirectional link (`internal/cli/doctor.go:734-773`, reference `2fbbf8e`). Pinned Go probes confirmed both directions. The port refuses before the entire fix run: `Refusing to fix <path>: line <n> holds a "<field>" value on issue "<id>" that is not an array, and fixing a bidirectional link would replace that value. Nothing was written.` Test: `command::project::doctor_fix::tests::fix_refuses_a_non_array_reciprocal_field_and_changes_nothing`, with strict byte/mode checks and array/null controls.

Doctor preserves the YAML reader's explicit Unsupported refusals instead of reporting a fabricated failed Go config check for an unported valid YAML value. Its fix plan is checked before any side effect, then executes Go's ordered operations under the store locks. If source bytes change between planning and lock admission, it asks the caller to retry rather than overwrite the concurrent data.
