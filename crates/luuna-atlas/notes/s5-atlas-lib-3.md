# Atlas lifecycle, full YAML and config rewriting

Reference: Luuna Atlas c4db80c3f2e622afbdd84f09077fd73b2a92d199, Go 1.27.0.
The CLI parser and runtime wiring are later plans; these are typed library operations.

## Implemented surface

lifecycle::run accepts Context, command, positional arguments and Options with
explicit flags and input. It implements query/rank/ready/status/stale/validate,
outcome/move/archive/restore/delete-domain/prune/compact and config
schema/show-path/set/unset. Shared health, anchors, supersession components, stale
analysis, merge_records and format_status support the remaining prime work.

The embedded lifecycle/contract.json is copied byte-for-byte from the reference.
No imported text is executed. The reader retains the exact ordinary subset path
and adds an iterative event graph for anchors, aliases, merges, tags, binary bytes,
complex keys, directives and first-document handling. Raw binary keys retain
identity separately from their Go UTF-8 display. Array JSONL rows keep Go's raw
sidecar for query/API/archive serialization while record field lookup sees no fields.

## Preservation decisions

Every relevant rewrite preflights raw data and prospective retained values before
changing stored files. Multi-file transfers and multi-domain prune/compact preflight
all affected files before their first stored-data write.

* GDL1's indented first document followed by unrelated less-indented content remains
  a refusal. Additional document data, discarded config root/domain/default
  subtrees and retained negative-zero sign loss also refuse.
* Ordinary anchors, aliases, merge keys, tagged values and invalid-UTF-8 binary
  strings survive Go rewrites and are supported. There is no blanket alias refusal.
* Explicit config replacement/deletion covers the selected subtree. A descendant
  assignment cannot silently discard an existing scalar ancestor.
* The earlier duplicate-key, legacy-outcome, conflicting-alias, retained-number,
  filtered-outcome and false/zero-ID guards apply to lifecycle writers.
* Move refuses duplicate source IDs that would remove additional records after
  appending only one destination row. A hook-changing ID cannot remove an unrelated
  source record. Go's copy-like result when a changed ID matches no source remains.
* Restore proves the later hardcoded strict live read can succeed before removing
  its archive row. Go can otherwise lose that row even with allow-unknown-types.
* Automatic prune cannot overwrite a retained archive_reason. Explicit archive
  status changes and stripping archive metadata on restore are intentional.
* Archive array rows survive rewrites. Their retained negative-zero values receive
  the same iterative sign guard as object fields.
* Unreadable/blocked archive destinations refuse before live data changes. An
  exclusive zero-length access probe in the nearest existing parent checks OS
  write access and is removed immediately; no archive directory is created by the
  preflight. No existing path is truncated by this probe.
* Compaction detects a source changed during hooks/preparation and refuses to overwrite
  it. Actual Go loses such hook-written rows in both auto/apply modes. Raw hook
  side effects remain the responsibility of the hook author; commands do not undo them.
* Comments and YAML formatting changes alone follow Go. Refusals preserve original
  stored bytes, modes and links; successful operations retain Go's write order.

Config add follows a symlink through truncation; config set/unset replace the
symlink with an atomic 0600 file. This corrects the earlier inferred claim that
all config writers follow links. Temp-file collision controls retain the existing
file. Raw hooks keep their original semantics; managed background-child cleanup
belongs to the wire plan.

## Known boundaries

The graph reader accepts 10000 flow/block levels and rejects the next level, as
the reference does. Cyclic aliases are refused rather than reproducing Go's
nontermination or stack failure. This is not a claim of exhaustive YAML test-suite
conformance: malformed-input wording outside the exercised matrix may still differ
from yaml.v3 (the carried CF3 boundary). Existing JSON parsing depth stays unchanged.
Go's empty successful JSONPretty result beyond 10000 output levels remains copied.
Go Number's uint64 behavior remains copied, including config validation distinctions.

The raw Object sidecar and registry array Text fix close foundation representation
and recursion gaps. Only the specifically planned obsolete YAML subset assertions
and the [1,2] raw-array expected literal changed; their inputs and unrelated checks
remain. The array correction now expects the exact Go-preserved array.

## Evidence

The isolated lifecycle suite covers JSON/human output, quiet behavior, errors,
schema/alias validation, raw-name identity, timestamps/numeric boundaries, hooks,
archive/restore, multi-file refusal, compaction strategies/limits and Git-history
staleness. Four separate hostile groups stream fixtures and exercise YAML depth,
deep registry Text, raw array sidecars and a 20000-node supersession cycle, including
a 128 KiB native thread stack. Run hostile tests only after compilation/reference
build with a 4 GiB address-space cap, 120-second timeout and a disk-backed TMPDIR.

The existing Go, formatting and action suites remain unchanged. Reference-dependent
tests are ignored by default and must be run explicitly. WSL results do not establish
native macOS/Windows execution or hosted CI success.

## Dependency and redistribution

Only granit-parser =2.1.0 is added; default features are disabled and std/error_messages
enabled. Registry checksum:
05f05586d4d4df3d47123207826c40b252728b438a1a07bad3c01ee6bea401df.
It is MIT OR Apache-2.0, Rust 1.81, with already-locked arraydeque/smallvec dependencies
and no build script. Base lock packages and versions are retained. The release must
carry the parser's upstream license/attribution notices, including its parser-lineage
notices, alongside the existing project licenses.
