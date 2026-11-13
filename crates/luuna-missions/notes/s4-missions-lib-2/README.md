# Missions workflow port

The native library now implements close, ready, blocked, stats, dep, label, unblock, completions, help, version, unknown-command suggestions and Go-shaped debug logging. The 48 allocated fixtures are in scope. This plan adds 65 library tests (125 to 190 on Linux) and 16 comparison tests. The Linux-only hostname test is conditional on other hosts.

Close retains the plan seam refusal until the plan lifecycle port: tickets with plan context or a nonblank plans file are refused before writing. Debug logging remains refused on targets whose hostname cannot be read using Go's mechanism; supported targets use uname or sysctl. The A8 refusals DL200–DL205 preserve files where Go would lose lines, records, keys or changed list values.

## Design decisions

- **DC1**: Embed all 57 Go help files in assets/s4-missions-lib-2/help/ and answer every `<cmd> --help` in app.rs, including commands of -3 and -4. Go answers --help in the dispatcher before the command runs (app.go:211-222); the hook is in this plan's file; -4's fixtures 124 and 135 need it. One table keeps a single owner and needs no further seam; the texts are Go's output at the reference commit.
- **DC2**: Root help generated from an app-owned 26-row table checked against COMMANDS by a unit test; VERSION is the constant "0.6.0"; version JSON prints runtime "go" and Go's platform spelling from std::env::consts (windows->win32, macos->darwin, x86_64->x64, aarch64->arm64, x86->386). COMMANDS (frozen mod.rs:300-327) holds no descriptions; CARGO_PKG_VERSION is 0.1.0; fixtures 003-007 and 014 pin these values.
- **DC3**: Hostname as Go's os.Hostname gets it: uname with the /proc/sys/kernel/hostname fallback on Linux and Android (Go src/os/sys_linux.go), sysctlbyname("kern.hostname") on macOS, iOS, FreeBSD and DragonFly (sys_bsd.go), through libc in one function; every other target keeps the debug-logging refusal (Error::Unsupported naming s4-missions-lib-2). The slots check-function contract's 'on unix' is read as these targets. s4-missions-lib-slots later_plan_corrections item 6. Each listed target then gets Go's value by Go's own system call; for any other target this plan cannot show equality, and refusing keeps the rule that the port never silently differs. Only Linux runs on this host.
- **DC4**: The plan index decodes plan records itself (a minimal IndexedPlan in plan_index.rs, accepting or rejecting a line as Go's decoding into model.Plan does, plans_jsonl_settlement) rather than waiting for -4's plan model. ready and blocked need status, seed, updatedAt and children now; -4 may replace the record type later (what_minus_4_calls).
- **DC5**: close finds every id first, then calls plans::check_status_change, then changes, then the A8 check, then writes, then calls plans::apply_status_change inside both locks. Same observable result as Go's single loop (nothing is written when an id is missing), and no refusal can follow a write.
- **DC6**: JSON arrays and maps are built as compact JSON with json::quote in workflow/render.rs and passed to Envelope::raw. Envelope (output.rs, foundation) has no array or map helper and is not this plan's file; Envelope::raw re-indents compact JSON as Go's encoder does.
- **DC7**: --respect-schedule uses a hand-written parser of Go's RFC3339Nano and 2006-01-02 forms; chrono is used only for the clock and date arithmetic. chrono's RFC 3339 parser accepts and rejects different inputs from Go's time.Parse; the workflow comparison test pins the cases.
- **DC8**: Four Go-comparison tests beyond the fixtures, in this plan's wrapper slot file. The 48 fixtures are almost all --json; text output, errors, 54 of the 57 help snapshots, the debug record, plan context and the A8 refusals have no fixture.
- **DC10**: Suspected defects numbered SD200 upward and A8 differences DL200 upward in notes/s4-missions-lib-2/UPSTREAM.md; the SD9 correction recorded there. The slots plan gives this plan SD200-SD299 and DL200-DL299 and freezes the crate's UPSTREAM.md (SL7).
- **DC11**: The A8 refusal texts of go_data_loss_cases.refusal_text, printed as a Go command error (Failure::Message). File-level cases first, the first in file order with (a) before (c) before (b) on one line; then field cases, the lowest line first, blocks before blockedBy before labels on one line. Follows the slots refusal_shape (names the file, the line numbers, the ticket and field where there is one, and the reason, says nothing was written, command error format). The file is named by its store name, not its path, so no host path enters output or tests. One deterministic message keeps the tests exact. A field case is only meaningful once every line decodes and ids are unique, so file-level cases come first.
- **DC12**: The check runs where Go would write: after the read and every Go validation, under the lock, immediately before the write. A command that Go would fail without writing keeps Go's error. Where Go writes nothing, nothing is lost, so Go's behaviour is copied (A8 is the only exception to A4 and applies only to the loss). Checking the bytes under the lock means no other Go-protocol writer can change the file between check and write.
- **DC13**: dep add ports Go's addDependency in dep.rs with the A8 check instead of calling block.rs add_dependency. block.rs add_dependency (block.rs:42-73) writes at :71 without the check and block.rs is a foundation file this plan may not edit (s4-missions-lib-slots later_plan_corrections; brief point 4). The duplication is about 20 lines; s4-missions-lib-refuse may later make block call the shared helper.
- **DC14**: Repeated keys are a case only at the top level of a record. Go's merge writes unchanged known values and unknown values back from their original raw bytes (internal/model/json.go:156-171), so a repeated key inside a nested value survives; only the top-level parseObject map (json.go:46-49) keeps the last value. Refusing nested repeats would refuse where Go loses nothing.
- **DC15**: The field check compares the tickets as read with the tickets as they will be written (before[k] against after[k], the Ticket list fields) and takes each changed field's raw value by re-reading the line in lossy.rs with json::parse_object; two new functions are added beside the revision 2 ones, which keep their signatures. This is Go's own changed-field test (json.go:164-166) on the values the foundation already decodes as Go does (model.rs:171-207, :388-398); the raw bytes are private in model.rs (:105), and re-reading the line under the lock reaches them without editing a frozen file. Keeping the revision 2 functions keeps what_minus_4_calls valid.
- **DC16**: A wrong-typed existing value in a scalar field the command sets (close: status, closedAt, updatedAt, and closeReason with --reason; dep, label, unblock: updatedAt) is not a case: Go and the port replace it as they replace a well-typed value. These assignments overwrite the whole field whatever it held (issue_mutate.go:47-51, :59; issue_links.go:32, :53, :241, :246, :394); a well-typed old value is replaced the same way, so nothing the command was meant to keep is lost, and the written value is a string from the clock, a constant or a Rust String argument, which Go's encoder writes so it reads back as written. Decision 6 concerns a written value that reads back differently and decision 3 the content a rewrite was meant to keep; neither covers a value the command is defined to replace. No observed run is needed because no read-back question arises.
- **DC17**: Two changes Go makes to exactly decoded lists are copied, not refused: uniqueStrings dropping an existing duplicate on label add and dep add (SD221), and withoutString turning [] into an absent field on remove (SD222). The value Go writes reads back as written; the change is the command's own operation on a value it decoded exactly (issue_mutate.go:101-123), the same operation it applies to clean data (PROBE:25033, :25036-25041 show an emptied field dropped). The brief's rule refuses only where the raw value is not exactly representable; these are listed as suspected defects instead.
- **DC18**: Field cases are sub-cases (d), (e) and (f) of DL200 to DL205, one entry per command; no DL206 or higher. Each command's entry already names its write point, check point and unit tests; adding its field cases there keeps one place per command for the hand-off and the notes.

## Files this plan owns

- `assets/s4-missions-lib-2/completions/completion_bash.txt`
- `assets/s4-missions-lib-2/completions/completion_fish.txt`
- `assets/s4-missions-lib-2/completions/completion_zsh.txt`
- `assets/s4-missions-lib-2/help/block.txt`
- `assets/s4-missions-lib-2/help/blocked.txt`
- `assets/s4-missions-lib-2/help/close.txt`
- `assets/s4-missions-lib-2/help/completions.txt`
- `assets/s4-missions-lib-2/help/config.txt`
- `assets/s4-missions-lib-2/help/config__schema.txt`
- `assets/s4-missions-lib-2/help/config__set.txt`
- `assets/s4-missions-lib-2/help/config__show.txt`
- `assets/s4-missions-lib-2/help/config__unset.txt`
- `assets/s4-missions-lib-2/help/create.txt`
- `assets/s4-missions-lib-2/help/dep.txt`
- `assets/s4-missions-lib-2/help/dep__add.txt`
- `assets/s4-missions-lib-2/help/dep__list.txt`
- `assets/s4-missions-lib-2/help/dep__remove.txt`
- `assets/s4-missions-lib-2/help/doctor.txt`
- `assets/s4-missions-lib-2/help/guard.txt`
- `assets/s4-missions-lib-2/help/init.txt`
- `assets/s4-missions-lib-2/help/label.txt`
- `assets/s4-missions-lib-2/help/label__add.txt`
- `assets/s4-missions-lib-2/help/label__list-all.txt`
- `assets/s4-missions-lib-2/help/label__list.txt`
- `assets/s4-missions-lib-2/help/label__remove.txt`
- `assets/s4-missions-lib-2/help/list.txt`
- `assets/s4-missions-lib-2/help/migrate-from-beads.txt`
- `assets/s4-missions-lib-2/help/onboard.txt`
- `assets/s4-missions-lib-2/help/plan.txt`
- `assets/s4-missions-lib-2/help/plan__adopt.txt`
- `assets/s4-missions-lib-2/help/plan__create.txt`
- `assets/s4-missions-lib-2/help/plan__edit.txt`
- `assets/s4-missions-lib-2/help/plan__list.txt`
- `assets/s4-missions-lib-2/help/plan__outcome.txt`
- `assets/s4-missions-lib-2/help/plan__prompt.txt`
- `assets/s4-missions-lib-2/help/plan__release.txt`
- `assets/s4-missions-lib-2/help/plan__reorder.txt`
- `assets/s4-missions-lib-2/help/plan__review.txt`
- `assets/s4-missions-lib-2/help/plan__show.txt`
- `assets/s4-missions-lib-2/help/plan__submit.txt`
- `assets/s4-missions-lib-2/help/plan__templates.txt`
- `assets/s4-missions-lib-2/help/plan__validate.txt`
- `assets/s4-missions-lib-2/help/prime.txt`
- `assets/s4-missions-lib-2/help/ready.txt`
- `assets/s4-missions-lib-2/help/search.txt`
- `assets/s4-missions-lib-2/help/setup.txt`
- `assets/s4-missions-lib-2/help/show.txt`
- `assets/s4-missions-lib-2/help/stats.txt`
- `assets/s4-missions-lib-2/help/sync.txt`
- `assets/s4-missions-lib-2/help/tpl.txt`
- `assets/s4-missions-lib-2/help/tpl__create.txt`
- `assets/s4-missions-lib-2/help/tpl__list.txt`
- `assets/s4-missions-lib-2/help/tpl__pour.txt`
- `assets/s4-missions-lib-2/help/tpl__show.txt`
- `assets/s4-missions-lib-2/help/tpl__status.txt`
- `assets/s4-missions-lib-2/help/tpl__step.txt`
- `assets/s4-missions-lib-2/help/tpl__step__add.txt`
- `assets/s4-missions-lib-2/help/unblock.txt`
- `assets/s4-missions-lib-2/help/update.txt`
- `assets/s4-missions-lib-2/help/upgrade.txt`
- `src/command/app.rs`
- `src/command/app/help.rs`
- `src/command/app/logging.rs`
- `src/command/plan_index.rs`
- `src/command/workflow/blocked.rs`
- `src/command/workflow/close.rs`
- `src/command/workflow/completions.rs`
- `src/command/workflow/dep.rs`
- `src/command/workflow/label.rs`
- `src/command/workflow/lossy.rs`
- `src/command/workflow/mod.rs`
- `src/command/workflow/ready.rs`
- `src/command/workflow/render.rs`
- `src/command/workflow/schedule.rs`
- `src/command/workflow/stats.rs`
- `src/command/workflow/unblock.rs`
- `tests/later_plans/s4_missions_lib_2.rs`
- `tests/support/fixtures/s4_missions_lib_2.rs`

## Checks and seams

Frozen harness inputs and unknown-input panics stay intact. Ported check-function branches assert complete Go outcomes; deferred branches are unchanged. The shared loss detector scans iteratively under the caller's store lock. Close holds plans then issues and calls the transition seams around its write. Native macOS evidence remains for CI; Linux parity does not establish other-host execution.

Run the required workspace checks, frontend checks, and Missions parity and format comparison suites with LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO configured. Compile hostile/deep tests separately before running with the documented address-space and time limits. Reports stay under target.
