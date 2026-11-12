# Upstream

This crate is a Rust port of the Go `luuna-missions` tool from the repository luuna-ai/luuna-missions.

- **Reference commit:** `2fbbf8e5b6a238ea47543e1dad112fe1716faec4`, read from a local clone. Every comparison in `tests/` is tied to this commit.
- **What was copied:** no Go source, Go file or Go history. The Rust code is written from the behaviour of the Go source and its regression fixtures.
- **Fixtures:** the 194 regression fixtures in `internal/regression/testdata/` are read in place from the clone and are not copied into this repository.
- **Lineage:** luuna-missions derives from Seeds (MIT).

## License notice

luuna-missions is distributed under the MIT License. Its notice, lines 1 to 21 of `LICENSE` at the reference commit, is reproduced verbatim:

```text
MIT License

Copyright (c) 2025 Jaymin West

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Suspected Go defects copied on purpose

The port copies the Go tool exactly, including the behaviour below, which looks like a defect. None of these is fixed here; each is listed for a later fix in Go and Rust together. Paths are relative to the reference clone.

| Id | Behaviour | Evidence | Ported in |
| --- | --- | --- | --- |
| SD1 | Records without an `id` key all decode with id `""` and collapse into one slot on read. | `internal/store/store.go:116-143` | s4-missions-lib (t2) |
| SD2 | Malformed JSONL lines are skipped silently on read. Append keeps them, but any full rewrite drops them without warning. | `internal/store/store.go:116-177` | s4-missions-lib (t2) |
| SD3 | Atomic replace does not fsync, and a target that did not exist gets CreateTemp's mode 0600 rather than 0644. | `internal/store/store.go:145-205` | s4-missions-lib (t2) |
| SD4 | Record objects in `--json` output carry `<`, `>` and `&` escapes (MarshalJSON uses `json.Marshal`), while envelope strings print literally. Observed while writing the harness: `show --json` prints a title `Literal <i> &` as `"Literal <i> &"`. | `internal/model/json.go:202-242`, `internal/cli/app.go:112-119` | s4-missions-lib (t3 to t5) |
| SD5 | JSON error mode is chosen by the literal `--json` token, so a failure under `--format json` alone prints the human `Error:` form on stderr. | `internal/cli/app.go:141`, `:260-272` | s4-missions-lib (t3) |
| SD6 | An unknown option prints `error: unknown option` on stderr even with `--json`; it is never a JSON envelope. | `internal/cli/options.go:16-22` | s4-missions-lib (t3) |
| SD7 | A space-separated value that begins with `--` is reported as a missing argument, so such a value can be passed only in `--k=v` form. | `internal/cli/options.go:64-124` | s4-missions-lib (t3) |
| SD8 | `LUUNA_MISSIONS_DEBUG=1` or a debug log level writes a debug record to stdout before the command's output, which breaks JSON stdout. | `internal/cli/logging.go:11-51` | s4-missions-lib-2 (refused with `Error::Unsupported` until then) |
| SD9 | `blocked --json` ignores `--limit`. | `internal/cli/issue_query.go:109-163` | s4-missions-lib-2 |
| SD10 | `tpl` with only flags indexes `p[0]` without a length check and may panic (exit 2 with a Go stack trace). Inferred, not observed. | `internal/cli/template.go:44` | s4-missions-lib-3 |
| SD11 | `doctor --fix` rewrites through `map[string]any`, which loses the records' key order (keys come out sorted). Its integrity and duplicate fixes cover issues.jsonl and templates.jsonl but not plans.jsonl. | `internal/cli/doctor.go:534-566`, `:615-674` | s4-missions-lib-3 |
| SD12 | `sync` commits the whole git index, not only `.luuna/missions/`. | `internal/cli/sync.go:100-106`, `internal/cli/sync_test.go:12-47` | s4-missions-lib-3 |
| SD13 | An unknown plan subcommand prints `Error: error: unknown command ...`, with the prefix doubled. | `internal/plan/plan.go:24-70` | s4-missions-lib-4 |
| SD14 | `plan validate` and `plan submit` write their diff JSON to stderr with empty stdout, even with `--json`. | `internal/plan/read.go:531-547` | s4-missions-lib-4 |
| SD15 | Id collision checks see only the ids in the local file, so creates on two branches merged with `merge=union` can produce the same id. | `internal/model/model.go:127-147` | s4-missions-lib (t2) |
| SD16 | `upgrade` always fails (JSON `success:false`, exit 1). | `internal/cli/standalone.go:12-21` | s4-missions-lib-3 |
| SD17 | A full rewrite (any update, block or other rewriting command) re-escapes literal `<`, `>`, `&`, U+2028 and U+2029 inside records that did not change, in known and unknown fields alike, so such a record is not byte-identical afterwards. `merge` re-emits the original raw values, but the store writes the record with `json.Marshal`, which compacts a Marshaler's output with HTML escaping. A record already written in Go's escaped form (including other escapes such as `ü` and numbers such as `1.50`) is kept byte for byte. Found by t1 (observed with the Go binary at the reference commit). | `internal/model/json.go:125-185`, `:202-208`; `internal/store/store.go:145-177` | s4-missions-lib (t2) |
| SD18 | The lock retry jitter is two random bytes taken as nanoseconds, so it is at most 65.535 µs, not up to the 100 ms `lockRetry` it is reduced modulo. Contending writers therefore retry almost in step every 100 ms. Found by t2 (read from the Go source; `time.Duration` counts nanoseconds). | `internal/store/store.go:64`, `:106-112` | s4-missions-lib (t2) |
