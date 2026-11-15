# Workflow provenance and compatibility

Reference: `<reference-clone>` at `2fbbf8e5b6a238ea47543e1dad112fe1716faec4`. The foundation's MIT attribution is retained in `crates/luuna-missions/UPSTREAM.md`. No Go implementation source or history was copied.

## Suspected defects copied

| ID | Behavior | Go source |
| --- | --- | --- |
| SD200 | close on an already closed ticket overwrites closedAt and updatedAt and prints Closed again. | `internal/cli/issue_mutate.go:46-52` |
| SD201 | close reports a plan-transition failure after issues.jsonl was already rewritten (no rollback). | `issue_mutate.go:62-67` |
| SD202 | close always takes the plans.jsonl lock, even without plans, so it needs write access for plans.jsonl.lock and can time out on a stale plans lock. | `issue_mutate.go:36` |
| SD203 | close accepts duplicate ids and reports each one. | `issue_mutate.go:41-61, :73-89` |
| SD204 | close updates dependents only through the closing ticket's blocks list; a ticket with a blockedBy entry not mirrored in blocks keeps it. | `issue_mutate.go:53-60` |
| SD205 | ready lists a ticket whose plan_id names a draft plan even when it is blocked. | `internal/cli/issue_read.go:553-561` |
| SD206 | ready judges a requires_plan ticket only by the plan that seeds it, never by its own plan_id. | `issue_read.go:553-557` |
| SD207 | ready reports --sort and --priority errors only after reading the store, so outside a project the not-in-project error wins. | `issue_read.go:524-584` |
| SD208 | blocked lists in file order, includes in_progress tickets (icon '>' plus ' [blocked]'), and accepts no --limit. | `internal/cli/issue_query.go:109-139; issue_read.go:358-377` |
| SD209 | stats --json sorts byPriority keys as strings ('10' before '2'), and the text form prints the 'By Type' heading even with no tickets. | `issue_query.go:224-285` |
| SD210 | stats and label list-all count a label twice when one ticket holds it twice. | `issue_query.go:216-221; issue_links.go:294-298` |
| SD211 | dep add and dep remove set updatedAt on both tickets even when nothing changes; dep remove accepts a ticket as its own blocker. | `internal/cli/issue_links.go:13-56` |
| SD212 | dep and label look for the project before checking the subcommand, so an unknown subcommand outside a project reports the project error. | `issue_links.go:67-72, :284` |
| SD213 | label add/remove set updatedAt even when nothing changes; label remove's JSON echoes the requested labels, not those removed. | `issue_links.go:394, :400-406` |
| SD214 | unblock --all silently overrides --from; --all with nothing to remove still rewrites the file and sets updatedAt; --from removes an open blocker. | `issue_links.go:197-249` |
| SD215 | completions takes `--json` as a shell name, and an unknown shell exits 1 with a '✗' line on stderr and no JSON envelope even with --json. | `internal/cli/completions.go:20-42` |
| SD216 | --version or -v anywhere among the arguments, even as an option's value, prints the version when the first argument is an option or a known command. | `internal/cli/app.go:170-177` |
| SD217 | When a record already holds plan_status or plan_children, ready and blocked --json re-encode the whole record with keys sorted, losing its key order (no content is lost). | `issue_read.go:28-58` |
| SD218 | The version JSON reports runtime "go" and Go's platform spelling; the Rust port prints the same for parity. | `app.go:142-155, :344` |
| SD219 | Root help is printed on stderr with exit 1 when no command is given, but on stdout with exit 0 for --help. | `app.go:166-185` |
| SD220 | When blocks, blockedBy or labels holds a value that is not a list (for example "urgent"), label remove, dep remove, unblock and close's dependent update do not apply their change to it and still set updatedAt and report success; unblock --from fails with '<T> is not blocked by <B>' although the value names B. No data is lost (the bytes are kept), so this is copied, not refused. | `json.go:99; issue_links.go:51-53, :218-246; issue_mutate.go:58-59; PROBE:25054, :25057-25062, PROBE:34` |
| SD221 | label add and dep add pass the existing list through uniqueStrings, so an existing exact duplicate entry is dropped when anything is added. | `issue_links.go:30-31, :388; issue_mutate.go:113-123 (DC17)` |
| SD222 | label remove, dep remove and close's dependent update drop a field written as [] even when nothing was removed (withoutString returns nil for an empty result). | `issue_mutate.go:101-112; json.go:164-176, :213-242 (DC17)` |

**SD9 correction:** The foundation's SD9 ('blocked --json ignores --limit', crates/luuna-missions/UPSTREAM.md) is inaccurate: blocked parses only --format and --json, so --limit is an unknown option (issue_query.go:110). This plan copies Go and records the correction in notes/s4-missions-lib-2/UPSTREAM.md; the frozen UPSTREAM.md is not edited.

## Deliberate differences from Go

### DL200: close

- Go: `internal/cli/issue_mutate.go:62 (store.WriteIssues in runClose, :22-91); dependents' blockedBy changed at :58; internal/store/store.go:116-156`.
- Loss: file-level (a), (b), (c); field (e), (f) in a dependent's blockedBy (V1 is not changed by close, SD220).
- Check: after every id is found and plans::check_status_change passed, under the plans.jsonl and issues.jsonl locks, before the write and before plans::apply_status_change.
- Tests: `command::workflow::close::tests::a_lossy_issues_file_is_refused_unchanged; command::workflow::close::tests::a_dependent_list_go_cannot_rewrite_is_refused_unchanged; controls command::workflow::close::tests::lists_close_leaves_unchanged_are_written_as_go_writes_them`.

### DL201: dep add

- Go: `internal/cli/issue_links.go:33 (store.WriteIssues in addDependency, :13-35); fields changed at :30-31`.
- Loss: file-level (a), (b), (c); field (d), (e), (f) in the issue's blockedBy and the blocker's blocks.
- Check: after both ids are found, under the issues.jsonl lock, before the write.
- Tests: `command::workflow::dep::tests::add_refuses_a_lossy_issues_file_unchanged; command::workflow::dep::tests::add_refuses_a_list_go_cannot_rewrite_unchanged; controls command::workflow::dep::tests::lists_dep_leaves_unchanged_are_kept_byte_for_byte`.

### DL202: dep remove

- Go: `internal/cli/issue_links.go:54 (store.WriteIssues in removeDependency, :37-56); fields changed at :51-52`.
- Loss: file-level (a), (b), (c); field (e), (f) in both lists (V1 unchanged, SD220).
- Check: after both ids are found, under the issues.jsonl lock, before the write.
- Tests: `command::workflow::dep::tests::remove_refuses_a_lossy_issues_file_unchanged; command::workflow::dep::tests::remove_refuses_a_list_go_cannot_rewrite_unchanged; controls command::workflow::dep::tests::lists_dep_leaves_unchanged_are_kept_byte_for_byte`.

### DL203: unblock

- Go: `internal/cli/issue_links.go:249 (store.WriteIssues in runUnblock, :192-272; it writes even when nothing is removed, SD214); fields changed at :243, :245`.
- Loss: file-level (a), (b), (c); field (e), (f) in the ticket's blockedBy and each removed blocker's blocks (V1 unchanged, SD220).
- Check: after the id is found and the --from link is checked, under the issues.jsonl lock, before the write.
- Tests: `command::workflow::unblock::tests::a_lossy_issues_file_is_refused_unchanged; command::workflow::unblock::tests::a_list_go_cannot_rewrite_is_refused_unchanged; controls command::workflow::unblock::tests::lists_unblock_leaves_unchanged_are_kept_byte_for_byte`.

### DL204: label add

- Go: `internal/cli/issue_links.go:395 (store.WriteIssues in runLabel, :378-396); field changed at :388`.
- Loss: file-level (a), (b), (c); field (d), (e), (f) in labels.
- Check: after the labels are validated and the id is found, under the issues.jsonl lock, before the write.
- Tests: `command::workflow::label::tests::add_refuses_a_lossy_issues_file_unchanged; command::workflow::label::tests::add_refuses_a_list_go_cannot_rewrite_unchanged; controls command::workflow::label::tests::label_lists_left_unchanged_are_kept_byte_for_byte`.

### DL205: label remove

- Go: `internal/cli/issue_links.go:395 (the same write, remove branch :390-392)`.
- Loss: file-level (a), (b), (c); field (e), (f) in labels (V1 and a remove of an absent label unchanged).
- Check: as for DL204.
- Tests: `command::workflow::label::tests::remove_refuses_a_lossy_issues_file_unchanged; command::workflow::label::tests::remove_refuses_a_list_go_cannot_rewrite_unchanged; controls command::workflow::label::tests::label_lists_left_unchanged_are_kept_byte_for_byte`.

All refusals exit 1 with the standard command error envelope or stderr message. They leave bytes and file modes unchanged and remove their locks. Read-only commands retain Go behavior. Comments are not involved: JSONL has no comments.

The invalid UTF-8 and lone surrogate decision is settled by the observed Go run recorded in `orchestration/ledger/s4-missions-lib-2/logs/go-lossy-probe-1.txt`: after a changed list is written, Go reads those elements as U+FFFD rather than their original values. Sub-case (f) therefore refuses. No variant is copied under decision 6. Unchanged raw fields, null, exact lists and commands whose resulting decoded lists are unchanged retain Go behavior.

### Exact refusal text

- **(a)**: `Refusing to rewrite issues.jsonl: line <n> is not a ticket record the Go tool can read, and rewriting the file would drop it. Nothing was written.`
- **(b)**: `Refusing to rewrite issues.jsonl: lines <m> and <n> both hold the ticket "<id>", and rewriting the file would keep only the last one. Nothing was written.`
- **(b_without_id)**: `Refusing to rewrite issues.jsonl: lines <m> and <n> both hold a record without an id, and rewriting the file would keep only the last one. Nothing was written.`
- **(c)**: `Refusing to rewrite issues.jsonl: the record on line <n> repeats the key "<key>", and rewriting the file would keep only its last value. Nothing was written.`
- **(d)**: `Refusing to rewrite issues.jsonl: line <n> (ticket "<id>") holds a "<field>" value that is not a list of strings, and changing the field would replace that value. Nothing was written.`
- **(e)**: `Refusing to rewrite issues.jsonl: line <n> (ticket "<id>") holds an element of "<field>" that is not a string, and changing the field would write it as an empty string. Nothing was written.`
- **(f)**: `Refusing to rewrite issues.jsonl: line <n> (ticket "<id>") holds an element of "<field>" with invalid UTF-8 or a lone surrogate escape, and changing the field would write it as U+FFFD. Nothing was written.`

## Assets copied from the reference

All 60 assets are copied byte for byte, with the MIT notice retained as above.

- `<reference-clone>/internal/cli/completion_bash.txt` → `assets/s4-missions-lib-2/completions/completion_bash.txt`
- `<reference-clone>/internal/cli/completion_fish.txt` → `assets/s4-missions-lib-2/completions/completion_fish.txt`
- `<reference-clone>/internal/cli/completion_zsh.txt` → `assets/s4-missions-lib-2/completions/completion_zsh.txt`
- `<reference-clone>/internal/cli/help/block.txt` → `assets/s4-missions-lib-2/help/block.txt`
- `<reference-clone>/internal/cli/help/blocked.txt` → `assets/s4-missions-lib-2/help/blocked.txt`
- `<reference-clone>/internal/cli/help/close.txt` → `assets/s4-missions-lib-2/help/close.txt`
- `<reference-clone>/internal/cli/help/completions.txt` → `assets/s4-missions-lib-2/help/completions.txt`
- `<reference-clone>/internal/cli/help/config.txt` → `assets/s4-missions-lib-2/help/config.txt`
- `<reference-clone>/internal/cli/help/config__schema.txt` → `assets/s4-missions-lib-2/help/config__schema.txt`
- `<reference-clone>/internal/cli/help/config__set.txt` → `assets/s4-missions-lib-2/help/config__set.txt`
- `<reference-clone>/internal/cli/help/config__show.txt` → `assets/s4-missions-lib-2/help/config__show.txt`
- `<reference-clone>/internal/cli/help/config__unset.txt` → `assets/s4-missions-lib-2/help/config__unset.txt`
- `<reference-clone>/internal/cli/help/create.txt` → `assets/s4-missions-lib-2/help/create.txt`
- `<reference-clone>/internal/cli/help/dep.txt` → `assets/s4-missions-lib-2/help/dep.txt`
- `<reference-clone>/internal/cli/help/dep__add.txt` → `assets/s4-missions-lib-2/help/dep__add.txt`
- `<reference-clone>/internal/cli/help/dep__list.txt` → `assets/s4-missions-lib-2/help/dep__list.txt`
- `<reference-clone>/internal/cli/help/dep__remove.txt` → `assets/s4-missions-lib-2/help/dep__remove.txt`
- `<reference-clone>/internal/cli/help/doctor.txt` → `assets/s4-missions-lib-2/help/doctor.txt`
- `<reference-clone>/internal/cli/help/guard.txt` → `assets/s4-missions-lib-2/help/guard.txt`
- `<reference-clone>/internal/cli/help/init.txt` → `assets/s4-missions-lib-2/help/init.txt`
- `<reference-clone>/internal/cli/help/label.txt` → `assets/s4-missions-lib-2/help/label.txt`
- `<reference-clone>/internal/cli/help/label__add.txt` → `assets/s4-missions-lib-2/help/label__add.txt`
- `<reference-clone>/internal/cli/help/label__list-all.txt` → `assets/s4-missions-lib-2/help/label__list-all.txt`
- `<reference-clone>/internal/cli/help/label__list.txt` → `assets/s4-missions-lib-2/help/label__list.txt`
- `<reference-clone>/internal/cli/help/label__remove.txt` → `assets/s4-missions-lib-2/help/label__remove.txt`
- `<reference-clone>/internal/cli/help/list.txt` → `assets/s4-missions-lib-2/help/list.txt`
- `<reference-clone>/internal/cli/help/migrate-from-beads.txt` → `assets/s4-missions-lib-2/help/migrate-from-beads.txt`
- `<reference-clone>/internal/cli/help/onboard.txt` → `assets/s4-missions-lib-2/help/onboard.txt`
- `<reference-clone>/internal/cli/help/plan.txt` → `assets/s4-missions-lib-2/help/plan.txt`
- `<reference-clone>/internal/cli/help/plan__adopt.txt` → `assets/s4-missions-lib-2/help/plan__adopt.txt`
- `<reference-clone>/internal/cli/help/plan__create.txt` → `assets/s4-missions-lib-2/help/plan__create.txt`
- `<reference-clone>/internal/cli/help/plan__edit.txt` → `assets/s4-missions-lib-2/help/plan__edit.txt`
- `<reference-clone>/internal/cli/help/plan__list.txt` → `assets/s4-missions-lib-2/help/plan__list.txt`
- `<reference-clone>/internal/cli/help/plan__outcome.txt` → `assets/s4-missions-lib-2/help/plan__outcome.txt`
- `<reference-clone>/internal/cli/help/plan__prompt.txt` → `assets/s4-missions-lib-2/help/plan__prompt.txt`
- `<reference-clone>/internal/cli/help/plan__release.txt` → `assets/s4-missions-lib-2/help/plan__release.txt`
- `<reference-clone>/internal/cli/help/plan__reorder.txt` → `assets/s4-missions-lib-2/help/plan__reorder.txt`
- `<reference-clone>/internal/cli/help/plan__review.txt` → `assets/s4-missions-lib-2/help/plan__review.txt`
- `<reference-clone>/internal/cli/help/plan__show.txt` → `assets/s4-missions-lib-2/help/plan__show.txt`
- `<reference-clone>/internal/cli/help/plan__submit.txt` → `assets/s4-missions-lib-2/help/plan__submit.txt`
- `<reference-clone>/internal/cli/help/plan__templates.txt` → `assets/s4-missions-lib-2/help/plan__templates.txt`
- `<reference-clone>/internal/cli/help/plan__validate.txt` → `assets/s4-missions-lib-2/help/plan__validate.txt`
- `<reference-clone>/internal/cli/help/prime.txt` → `assets/s4-missions-lib-2/help/prime.txt`
- `<reference-clone>/internal/cli/help/ready.txt` → `assets/s4-missions-lib-2/help/ready.txt`
- `<reference-clone>/internal/cli/help/search.txt` → `assets/s4-missions-lib-2/help/search.txt`
- `<reference-clone>/internal/cli/help/setup.txt` → `assets/s4-missions-lib-2/help/setup.txt`
- `<reference-clone>/internal/cli/help/show.txt` → `assets/s4-missions-lib-2/help/show.txt`
- `<reference-clone>/internal/cli/help/stats.txt` → `assets/s4-missions-lib-2/help/stats.txt`
- `<reference-clone>/internal/cli/help/sync.txt` → `assets/s4-missions-lib-2/help/sync.txt`
- `<reference-clone>/internal/cli/help/tpl.txt` → `assets/s4-missions-lib-2/help/tpl.txt`
- `<reference-clone>/internal/cli/help/tpl__create.txt` → `assets/s4-missions-lib-2/help/tpl__create.txt`
- `<reference-clone>/internal/cli/help/tpl__list.txt` → `assets/s4-missions-lib-2/help/tpl__list.txt`
- `<reference-clone>/internal/cli/help/tpl__pour.txt` → `assets/s4-missions-lib-2/help/tpl__pour.txt`
- `<reference-clone>/internal/cli/help/tpl__show.txt` → `assets/s4-missions-lib-2/help/tpl__show.txt`
- `<reference-clone>/internal/cli/help/tpl__status.txt` → `assets/s4-missions-lib-2/help/tpl__status.txt`
- `<reference-clone>/internal/cli/help/tpl__step.txt` → `assets/s4-missions-lib-2/help/tpl__step.txt`
- `<reference-clone>/internal/cli/help/tpl__step__add.txt` → `assets/s4-missions-lib-2/help/tpl__step__add.txt`
- `<reference-clone>/internal/cli/help/unblock.txt` → `assets/s4-missions-lib-2/help/unblock.txt`
- `<reference-clone>/internal/cli/help/update.txt` → `assets/s4-missions-lib-2/help/update.txt`
- `<reference-clone>/internal/cli/help/upgrade.txt` → `assets/s4-missions-lib-2/help/upgrade.txt`
