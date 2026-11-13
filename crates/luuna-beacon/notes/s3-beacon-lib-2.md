# Beacon authoring library

Plan `s3-beacon-lib-2`, revision 3, continued on the merged artifact and deep-value prerequisites. The runtime adapter is owned by the later wiring plan; this change adds library operations only. Existing foundation sources, dependencies, lockfiles and frozen tests are unchanged.

## Public behavior

The authoring module exports YAML values, Markdown/TypeScript rendering, emit/import, pin/unpin, section diff, relationship tree, statistics and configuration APIs. Every filesystem operation uses the handle's explicit project root. No operation changes process environment/current directory, launches a process or writes to a terminal.

YAML values use a flat arena. Parsing, alias expansion accounting, value conversion, equality, cloning, encoding and destruction avoid recursively owned YAML collections. Source order determines configuration validation, routing, children, mixin users, removed sections and emit results where Go uses map iteration. YAML map serialization uses yaml.v3 natural ordering and Unicode properties recorded from the pinned Go toolchain.

## Deliberate differences

- RD3: a nil-map config assignment returns `GoPanic` instead of terminating the process.
- RD4: ancestor cycles return `CircularInheritance`; Go grows a slice until exhaustion.
- RD5: unspecified Go map orders use deterministic file/current-record order.
- RD6: relative emit/import paths resolve against the explicit project root.
- RD7/RD9: YAML scanner syntax/anchor diagnostics retain the event parser's spelling where allowed by the plan. Semantic typing, duplicate keys, alias limits and depth errors are compared exactly.
- RD8: invalid UTF-8/non-map config inputs fail with the YAML prefix. Authoring uses its complete owned decoder rather than routing valid deep-flow input through the foundation's 8-bit flow scanner. The foundation remains unchanged.
- RD10: OS errors retain the Go operation/path prefix and Rust's platform error detail.
- RD11: an empty `mixins` vector reports null because the foundation cannot distinguish an absent vector from an explicit empty array.

## Refusals before data loss

Every refusal leaves source bytes and the complete file tree unchanged, including lock and temporary files.

| IDs | Boundary |
| --- | --- |
| RD12–RD14 | Config rewrites refuse additional documents, dropped null top-level keys and any whole-document value that would read back differently. Validation retains precedence. The 46 scalar probe families copy Go's changes except negative zero; read-back equality compares exact numeric values, bytes, timestamps, sequences and maps. |
| RD15–RD20 | Import refuses unrecognised BOM/CRLF/empty frontmatter, non-whitespace introduction before the first heading, non-string descriptions, dropped null keys, later documents and a differing frontmatter name. |
| RD21 | Import refuses frontmatter whose JSON store representation changes its value, including nonrepresentable integers and non-UTF-8 binary strings. Go encoding errors take precedence and retain record-relative pointers and offsets. |
| RD22 | Emit precomputes every target and refuses replacement of differing existing content inside canonical `.luuna/`, including links. Check/dry-run do not write; identical content and new files remain allowed. |
| RD23–RD24 | Pin/unpin retain raw record lines from a single read, select current/version records first, then refuse unbound keys and repeated values that would be lost. Null/scalar rules, merged objects, reused array elements and used legacy fields follow Go's reading. Every line equal to the selected decoded record is inspected. Repeated integer fields compare int64 values exactly, optional numeric nulls clear their fields, and nested checks follow file order. Version increment overflow deliberately wraps as Go does. |

## Preserved Go defects

- **GD7**: config set on an empty or null config.yaml panics (assignment to entry in nil map). Port: error (AS7).
- **GD8**: validateRawConfig reports a random one of several unknown properties or bad targets (map order). Port: first in file order.
- **GD9**: config unset of a missing key skips validation and the write and still succeeds (removed false).
- **GD10**: writeJSON ignores json.MarshalIndent errors: config show --json, show --path --json and the set echo print only an empty line, exit 0, for non-string nested keys and NaN/Inf (also seen in the probe, log:51). Port: YamlValue::to_go_json returns Go's error; the step yields no document.
- **GD11**: tree's ancestor loop has no cycle guard and exhausts memory. Port: error (AS8).
- **GD12**: tree children and mixinUsers follow the order of prompts(), which comes from current()'s map (cli.go:780-787).
- **GD13**: diff lists removed sections in map order (cli.go:1695-1697).
- **GD14**: emit --all files and --check stale lists follow prompts() order, which comes from current()'s map.
- **GD15**: emitDir picks a random target when several match a tag or several are default (cli.go:1947-1958).
- **GD16**: Legacy emitDirByTag migration: tag order inside a target and which directory gets a '-2' suffix depend on map order (cli.go:1804-1826).
- **GD17**: config set/unset rewrite (saveRawConfig re-marshals the whole file): comments are dropped (copied, known loss); text changes for values Go reads the same before and after (digits beyond float64's precision, tags, number forms, timestamp text, escapes, flow style, anchors, merges, key order) are copied, each probe row listed (go_data_loss_cases 'GD17 (copied part)', probe_settlement.config_rewrite). Later documents and null top-level keys: refused (RD12, RD13); values that would read back different: refused (RD14).
- **GD18**: loadConfig accepts default: "true" (string) but validateRawConfig requires a bool, so such a config loads and then blocks every set/unset.
- **GD19**: A `targets` key that is not a mapping falls through to the legacy migration in loadConfig but is rejected by validateRawConfig.
- **GD20**: A legacy-only config (emitDir, no project/version) cannot be edited by config set (unknown property / project must be a non-empty string).
- **GD21**: import frontmatter split is LF-only (BOM, CRLF and empty blocks not recognised). Port: refused where a block is present (RD15); a first-line thematic break with no closing line is copied.
- **GD22**: import treats `## ` inside code fences as a heading (copied) and drops text before the first `## ` line (refused when not whitespace, RD16).
- **GD23**: import drops a non-string description from both the description and the frontmatter. Port: refused (RD17).
- **GD24**: import then emit does not round-trip: render appends a newline to a body that already ends in one.
- **GD25**: renderMarkdown lets frontmatter keys `name` and `description` override the prompt's own.
- **GD26**: .ts export: the name is only upper-cased with '-' as '_' (may not be an identifier); a description containing a newline breaks the comment.
- **GD27**: emit writes outside the project root through '..' in a prompt's emitDir.
- **GD28**: emit writes outside the project root through '..' in a prompt's emitAs.
- **GD29**: emit writes outside the project root through a prompt name containing '..' or '/' when emitAs is empty.
- **GD30**: emit writes outside the project root through '..' in a config target's dir.
- **GD31**: emit writes outside the project root through '..' in legacy emitDir or emitDirByTag.
- **GD32**: emit writes through committed symlinks (a linked directory or a linked file on the emit path), and --check reads through them.
- **GD33**: config set/unset and import read and write the store through committed symlinks (foundation finding carried forward, s3-beacon-lib/review-security.json:35-38, beacon.rs:92).
- **GD34**: emit overwrites any differing file a target names, without lock, atomic write or confirmation. Assessed under A8: copied outside .luuna/ (the command's purpose); refused inside .luuna/ (RD22).
- **GD35**: emit --out with several prompts writes them all to one path (last wins).
- **GD36**: emit creates the directory even for a skipped prompt, and a failure on prompt k leaves prompts 1..k-1 written.
- **GD37**: render ignores a pin; only emit honours it. Historical pins fail emission when the requested version is absent from current records; rendering still ignores the pin. This was observed in the paired Go run.
- **GD38**: pin and unpin always append a new version, even when nothing changes; unpin ignores an @version.
- **GD39**: pin's version parse accepts signs and trailing text (Sscanf %d).
- **GD40**: diff ignores version parse errors (an unparsable second version keeps the first's value) and accepts trailing text.
- **GD41**: diff matches records by name across ids and statuses (last duplicate wins) and collapses duplicate section names.
- **GD42**: stats counts archived prompts and counts a tag repeated inside one prompt twice.
- **GD43**: yaml.v3's key comparator (sorter.go) is not a strict weak order for some key sets, so Go's order for them depends on Go's sort algorithm. Port: sorter.go's comparator with a stable sort; ordinary keys compared against Go.
- **GD44**: import deletes a frontmatter `name` (cli.go:1639), losing it when it differs from --name. Port: refused (RD20).
- **GD45**: import stores frontmatter values that Go's JSON reading changes (integers beyond float64's exact range, non-UTF-8 strings). Port: refused (RD21).
- **GD46**: config show and loadConfig ignore later documents and top-level null keys of config.yaml. Read only: copied.
- **GD47**: pin and unpin re-encode the selected current record from Go's model.Prompt (cli.go:804-832).

Additional observed emitter defects are copied: a leading BOM makes the double-quoted writer escape subsequent characters, and a leading newline contributes the block header break. Config/import read-back checks still refuse a lossy write where required.

Malformed UTF-8 in a recognised frontmatter block keeps the YAML error boundary. Malformed UTF-8 in Markdown body text is additionally refused before append: an observed Go run replaces those source bytes during JSON storage. The source and store are retained; a read/invalid-data error explains the refusal. This is the same A8 value-preservation rule, applied to body text as well as frontmatter.

Compatibility algorithms follow yaml.v3 v3.0.1; its LICENSE and NOTICE are retained under `src/authoring/licenses/`. The generated Unicode property/case tables come from the pinned Go toolchain; its BSD license is retained there as well.

## Event parser provenance

`src/authoring/yaml_events/` contains the production parser/scanner/character helpers from yaml-rust2 0.10.4, under the adjacent MIT and Apache licenses. Local adaptations are module paths, removal of optional debug output/loader-oriented test code, standard `VecDeque` lookahead, a u32 flow counter, no invented block-scalar EOF newline, Go-compatible Unicode line breaks/folding, closing-quote indentation and a clippy-equivalent predicate. The outer decoder applies yaml.v3's independent 10,000 block/flow limits. No dependency or foundation parser was changed. Additional pinned-Go probes cover underscore-prefixed strings, short timestamps, mixed and complex keys, exact large-integer key order, binary top-level keys, null-key ignored values, duplicate-key error precedence and byte-wise malformed UTF-8 replacement. This local event implementation is exercised by the complete authoring parity and hostile suites; it does not claim upstream's full test suite was run.

## Verification

- 76 focused library tests; 28 public API tests; 42 ignored Go authoring comparisons; 10 ignored hostile cases.
- The hostile cases cover config/value/import alias bombs, many documents/anchors, flow depth exactly 10,000 on a 2 MiB stack, block/flow refusal above Go's limits, 9,999-level JSON rendering, a streamed 24 MiB Markdown source and ancestor cycles. The Go cycle child has a positive normal-tree control, a separate 1 GiB limit and a 30-second kill bound.
- Additional foundation Go-reference, parity and format comparisons, workspace Rust checks, hygiene and frontend checks are required before merge.

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
node scripts/check-hygiene.mjs
npm ci
npm test
LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_authoring -- --ignored
cargo test -p luuna-beacon --locked --test authoring_hostile --no-run
LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> RUST_TEST_THREADS=1 prlimit --as=4294967296 timeout 120 cargo test -p luuna-beacon --locked --test authoring_hostile -- --ignored
```

Compile before applying the hostile address-space limit. Large input/output logs belong on disk under the worktree target directory. Go builds use the read-only reference harness. These results establish Linux library behavior; native macOS execution and runtime/client integration belong to later checks.

## Fixture corrections

Reviewed continuation commits preserve the WIP API tests, adapt only the JSON ownership boundary, and correct expectations proven wrong by the pinned Go run: the Go 1.27 general-map error; validation before an invalid `dir` rewrite; full config for empty CLI `--path`; historical-pin failure against current records; description-field presence rather than a substring also in the prompt name; equal frontmatter/import names for the separate newline-growth test; root fragments embedded by `filepath.Join`; and forced shared-output writes to avoid Go's nondeterministic skip/name association. Separate tests retain exact skip/force behavior. Assertions still compare values, complete trees, bytes, modes and refusal text.
