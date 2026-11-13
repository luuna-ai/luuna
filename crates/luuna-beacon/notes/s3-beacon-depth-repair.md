# Beacon deep-frontmatter ownership repair

The artifacts plan's unchanged hostile test aborted with SIGABRT. A probe on current main read and encoded valid 9,999-level object frontmatter, then aborted while dropping the returned prompt. This occurred in a crate-only build; whole-workspace serde_json feature unification can change its container representation and stack behavior.

`Prompt::frontmatter` now owns `JsonValue` values. The wrapper borrows/dereferences to serde_json::Value, but drops, clones and compares its owned tree iteratively. Decoder intermediate values and duplicate replacement use the same ownership. No dependency, nesting limit, stored byte format, Go behavior, or CLI output contract changed. The crate is unpublished; Rust callers constructing a map value from serde_json::Value now use `.into()`. This avoids a Drop implementation on Prompt itself, which would prevent ordinary struct construction and field moves.

The existing model, payload and resolve fixture insertions only gained explicit `.into()` conversions, in a separate commit. Their assertions are unchanged. A new streamed hostile regression covers read, encode, clone, equality, drop, and error cleanup after an already-read deep record, with unchanged store bytes. Run it only under `prlimit --as=4294967296 timeout 120`, after compiling it separately. Test data lives under Cargo's target directory.

This repair does not make arbitrary operations on a borrowed raw serde_json::Value iterative. Native wiring must preserve the runtime's existing response limits and use the library's iterative Go encoder for deeply nested CLI output. Go reference clones stay read-only.
