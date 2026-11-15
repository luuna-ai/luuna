//! Owner: plan s3-beacon-lib-3. Hostile and very large inputs for the
//! artifact commands and doctor (PIPELINE rule 10).
//!
//! Every test is ignored and runs only under the memory and time limit:
//! `LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> prlimit --as=4294967296 timeout 120 cargo test -p luuna-beacon --locked --test artifacts_hostile -- --ignored`,
//! after the binary is built without the limit (`--no-run`). Every input is
//! written to disk by loops over constants in this file: a fixed chunk
//! written a constant number of times. None is built as one large value in
//! memory or sized from the environment. Inputs other than the sparse file
//! stay under 100 MB in the cases' temporary roots; the sparse file goes
//! under Cargo's target folder and allocates no blocks.
//!
//! These tests are written against the library contract of the plan and
//! compile once the library exists.
#![cfg(unix)]

mod support;

use luuna_beacon::{ArtifactKind, Beacon};
use serde_json::json;
use std::fs;
use std::io::{BufWriter, Write as _};
use std::path::Path;
use support::artifacts::*;
use support::case::{Case, Seen, Step};

/// Go's file cap per artifact (`artifact.go:53`).
const FILE_LIMIT: usize = 256;
/// Many more files than the cap.
const MANY_FILES: usize = 2000;
/// A 64 KiB chunk of ASCII.
static TEXT_CHUNK: [u8; 65536] = [b'a'; 65536];
/// A 64 KiB chunk that is not UTF-8.
static BINARY_CHUNK: [u8; 65536] = [0xff; 65536];
/// 64 chunks of 64 KiB: exactly 4 MiB.
const FOUR_MIB_CHUNKS: usize = 64;
/// 48 chunks of 64 KiB: 3 MiB.
const THREE_MIB_CHUNKS: usize = 48;
/// Valid base64 quads in the hand-written record: 3 MiB of text.
const QUADS: usize = 786_432;
/// Nested folders above one file.
const DEPTH: usize = 200;
/// `[` characters in one artifacts line.
const OPEN_BRACKETS: usize = 100_000;
/// A chunk of `[`.
const BRACKET_CHUNK: [u8; 1000] = [b'['; 1000];
/// Nesting levels of a prompt's frontmatter, below the parser cap.
const FRONTMATTER_LEVELS: usize = 9999;
/// The sparse source file: 6 GiB, never read by the library.
const SPARSE_SIZE: u64 = 6 * 1024 * 1024 * 1024;

fn ready() -> Case {
    let mut case = Case::with_project(PROJECT);
    plain(&mut case, Step::Init);
    case
}

fn error(seen: &Seen) -> &str {
    assert_eq!(seen.code, 1, "the step was expected to fail");
    seen.error.as_deref().expect("an error document")
}

/// `SKILL.md` and `count - 1` one-line files under `f/`, in walk order
/// after `SKILL.md`.
fn many(count: usize) -> Vec<Entry> {
    let mut entries = vec![entry("SKILL.md", "# many\n")];
    for index in 1..count {
        entries.push(entry(&format!("f/{index:04}.txt"), "x\n"));
    }
    entries
}

#[test]
#[ignore = "hostile input: run only under prlimit --as=4294967296 timeout 120 with --ignored, see crates/luuna-beacon/notes/s3-beacon-lib-3.md"]
fn file_limit_256_is_allowed_and_257_is_refused_like_go() {
    let mut case = ready();
    art(&mut case, tree("src/at", many(FILE_LIMIT)));
    let seen = art(&mut case, import(SKILL, "src/at", "at"));
    assert_eq!(seen.code, 0, "{:?}", seen.error);
    assert_eq!(
        seen.raw[0]["artifact"]["files"].as_array().unwrap().len(),
        FILE_LIMIT
    );
    art(&mut case, export(SKILL, "at").out("out/at"));

    art(&mut case, tree("src/over", many(FILE_LIMIT + 1)));
    // GD63: the cap is checked before the 257th file is read, so an
    // unreadable 257th file still gives the limit error.
    let last = format!("src/over/f/{:04}.txt", FILE_LIMIT);
    art(&mut case, chmod(&last, 0o000));
    assert_unreadable(&case.lib_root().join(&last));
    let seen = art(&mut case, import(SKILL, "src/over", "over"));
    assert_eq!(error(&seen), "artifact file limit exceeded");
    art(&mut case, chmod(&last, 0o644));
}

#[test]
#[ignore = "hostile input: run only under prlimit --as=4294967296 timeout 120 with --ignored, see crates/luuna-beacon/notes/s3-beacon-lib-3.md"]
fn many_files_stop_at_the_limit_like_go() {
    let mut case = ready();
    art(&mut case, tree("src/many", many(MANY_FILES)));
    let seen = art(&mut case, import(SKILL, "src/many", "many"));
    assert_eq!(error(&seen), "artifact file limit exceeded");
    let seen = art(&mut case, import(AGENT, "src/many", "many"));
    assert_eq!(error(&seen), "artifact file limit exceeded");
    art(&mut case, list(SKILL));
}

#[test]
#[ignore = "hostile input: run only under prlimit --as=4294967296 timeout 120 with --ignored, see crates/luuna-beacon/notes/s3-beacon-lib-3.md"]
fn four_mib_is_allowed_and_one_byte_more_is_refused_like_go() {
    let mut case = ready();
    art(&mut case, tree("src/big", vec![entry("SKILL.md", "big\n")]));
    art(
        &mut case,
        stream(
            "src/big/big.txt",
            vec![Part::Repeat {
                chunk: &TEXT_CHUNK,
                times: FOUR_MIB_CHUNKS,
            }],
        ),
    );
    let seen = art(&mut case, import(SKILL, "src/big", "big"));
    assert_eq!(seen.code, 0, "{:?}", seen.error);
    art(&mut case, verify(SKILL, Some("big")));
    art(&mut case, export(SKILL, "big").out("out/big"));
    assert_eq!(
        fs::metadata(case.lib_root().join("out/big/big.txt"))
            .unwrap()
            .len(),
        (TEXT_CHUNK.len() * FOUR_MIB_CHUNKS) as u64
    );

    art(
        &mut case,
        tree("src/over", vec![entry("SKILL.md", "over\n")]),
    );
    art(
        &mut case,
        stream(
            "src/over/big.txt",
            vec![
                Part::Repeat {
                    chunk: &TEXT_CHUNK,
                    times: FOUR_MIB_CHUNKS,
                },
                Part::Bytes(b"a".to_vec()),
            ],
        ),
    );
    let seen = art(&mut case, import(SKILL, "src/over", "over"));
    assert_eq!(error(&seen), "artifact file exceeds 4 MiB: big.txt");
}

#[test]
#[ignore = "hostile input: run only under prlimit --as=4294967296 timeout 120 with --ignored, see crates/luuna-beacon/notes/s3-beacon-lib-3.md"]
fn large_base64_payload_round_trips_like_go() {
    let mut case = ready();
    art(&mut case, tree("src/bin", vec![entry("SKILL.md", "bin\n")]));
    art(
        &mut case,
        stream(
            "src/bin/blob.bin",
            vec![Part::Repeat {
                chunk: &BINARY_CHUNK,
                times: THREE_MIB_CHUNKS,
            }],
        ),
    );
    let seen = art(&mut case, import(SKILL, "src/bin", "bin"));
    assert_eq!(seen.code, 0, "{:?}", seen.error);
    assert_eq!(
        seen.raw[0]["artifact"]["files"][1]["encoding"],
        json!("base64")
    );
    art(&mut case, read(SKILL, "bin", "blob.bin"));
    art(&mut case, verify(SKILL, None));
    art(&mut case, export(SKILL, "bin").target("claude"));
    let exported = case.lib_root().join(".claude/skills/bin/blob.bin");
    assert_eq!(
        fs::metadata(&exported).unwrap().len(),
        (BINARY_CHUNK.len() * THREE_MIB_CHUNKS) as u64
    );
}

/// Streams one artifacts line whose base64 content holds `QUADS` valid
/// quads and then one with an illegal byte.
fn write_late_bad_record(root: &Path) {
    let file = fs::File::create(root.join(".luuna/beacon/artifacts.jsonl"))
        .expect("create artifacts.jsonl");
    let mut out = BufWriter::new(file);
    out.write_all(
        br#"{"id":"artifact-c1","kind":"skill","name":"late","version":1,"files":[{"path":"SKILL.md","encoding":"base64","content":""#,
    )
    .unwrap();
    for _ in 0..QUADS {
        out.write_all(b"QUFB").unwrap();
    }
    out.write_all(b"QU!B").unwrap();
    out.write_all(
        br#"","sha256":"00","executable":false}],"sha256":"00","createdAt":"2024-01-01T00:00:00Z"}"#,
    )
    .unwrap();
    out.write_all(b"\n").unwrap();
    out.flush().unwrap();
}

#[test]
#[ignore = "hostile input: run only under prlimit --as=4294967296 timeout 120 with --ignored, see crates/luuna-beacon/notes/s3-beacon-lib-3.md"]
fn large_base64_record_with_a_late_bad_byte_like_go() {
    let mut case = ready();
    on_both(&mut case, write_late_bad_record);
    let expected = format!(
        "invalid artifact record skill late@1: illegal base64 data at input byte {}",
        QUADS * 4 + 2
    );
    assert_eq!(error(&art(&mut case, list(SKILL))), expected);
    assert_eq!(error(&art(&mut case, verify(SKILL, None))), expected);
    let seen = art(&mut case, doctor(false));
    assert_eq!(seen.code, 1);
}

#[test]
#[ignore = "hostile input: run only under prlimit --as=4294967296 timeout 120 with --ignored, see crates/luuna-beacon/notes/s3-beacon-lib-3.md"]
fn deep_directory_nesting_like_go() {
    let mut case = ready();
    let mut deep = String::new();
    for _ in 0..DEPTH {
        deep.push_str("d/");
    }
    deep.push_str("leaf.md");
    art(
        &mut case,
        tree(
            "src/deep",
            vec![entry("SKILL.md", "deep\n"), entry(&deep, "leaf\n")],
        ),
    );
    let seen = art(&mut case, import(SKILL, "src/deep", "deep"));
    assert_eq!(seen.code, 0, "{:?}", seen.error);
    assert_eq!(seen.raw[0]["artifact"]["files"][1]["path"], json!(deep));
    art(&mut case, read(SKILL, "deep", &deep));
    art(&mut case, export(SKILL, "deep").out("out/deep"));
    assert!(case.lib_root().join("out/deep").join(&deep).is_file());
}

#[test]
#[ignore = "hostile input: run only under prlimit --as=4294967296 timeout 120 with --ignored, see crates/luuna-beacon/notes/s3-beacon-lib-3.md"]
fn deeply_nested_json_line_is_refused_like_go() {
    let mut case = ready();
    on_both(&mut case, |root| {
        stream_file(
            &root.join(".luuna/beacon/artifacts.jsonl"),
            &[
                Part::Repeat {
                    chunk: &BRACKET_CHUNK,
                    times: OPEN_BRACKETS / BRACKET_CHUNK.len(),
                },
                Part::Bytes(b"\n".to_vec()),
            ],
        );
    });
    let seen = art(&mut case, list(SKILL));
    assert!(
        error(&seen).starts_with("invalid JSONL line 1 in <root>/.luuna/beacon/artifacts.jsonl: "),
        "{:?}",
        seen.error
    );
    art(&mut case, verify(AGENT, None));
}

/// Streams one prompt line whose frontmatter nests `FRONTMATTER_LEVELS`
/// objects.
fn write_deep_frontmatter(root: &Path) {
    let file =
        fs::File::create(root.join(".luuna/beacon/prompts.jsonl")).expect("create prompts.jsonl");
    let mut out = BufWriter::new(file);
    out.write_all(
        br#"{"id":"artifact-c2","name":"deep","version":1,"sections":[{"name":"Task","body":"t"}],"status":"active","createdAt":"2024-01-01T00:00:00Z","updatedAt":"2024-01-01T00:00:00Z","frontmatter":"#,
    )
    .unwrap();
    for _ in 1..FRONTMATTER_LEVELS {
        out.write_all(br#"{"a":"#).unwrap();
    }
    out.write_all(b"{}").unwrap();
    for _ in 1..FRONTMATTER_LEVELS {
        out.write_all(b"}").unwrap();
    }
    out.write_all(b"}\n").unwrap();
    out.flush().unwrap();
}

#[test]
#[ignore = "hostile input: run only under prlimit --as=4294967296 timeout 120 with --ignored, see crates/luuna-beacon/notes/s3-beacon-lib-3.md"]
fn doctor_on_deeply_nested_prompt_frontmatter_like_go() {
    let mut pair = Pair::new(Git::Available);
    pair.step(Step::Init);
    pair.both(|root, _| write_deep_frontmatter(root));
    let seen = pair.art(doctor(false));
    assert_eq!(seen.raw[0]["command"], json!("doctor"));
    pair.art(doctor(true));
}

#[test]
#[ignore = "hostile input: run only under prlimit --as=4294967296 timeout 120 with --ignored, see crates/luuna-beacon/notes/s3-beacon-lib-3.md"]
fn huge_sparse_source_file_is_refused_without_reading_it() {
    // Library only: Go reads the whole file before its size check (GD81).
    let holder = tempfile::Builder::new()
        .prefix("beacon-sparse-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .expect("create a folder under Cargo's target folder");
    let root = holder.path().canonicalize().expect("canonical root");
    let beacon = Beacon::open(&root).expect("open the root");
    beacon.init().expect("init");
    let source = root.join("src/s");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("SKILL.md"), b"sparse\n").unwrap();
    let huge = fs::File::create(source.join("huge.bin")).unwrap();
    huge.set_len(SPARSE_SIZE).expect("make a sparse file");
    drop(huge);
    let store = root.join(".luuna/beacon/artifacts.jsonl");
    let before = fs::read(&store).unwrap();
    match beacon.artifact_import(ArtifactKind::Skill, Some(Path::new("src/s")), "sparse") {
        Ok(_) => panic!("a 6 GiB source file was stored"),
        Err(error) => assert_eq!(error.to_string(), "artifact file exceeds 4 MiB: huge.bin"),
    }
    assert_eq!(fs::read(&store).unwrap(), before);
    assert_eq!(
        names_under(&root.join(".luuna/beacon")),
        names_under(&root.join(".luuna/beacon"))
            .into_iter()
            .filter(|name| !name.ends_with(".lock") && !name.starts_with(".tmp"))
            .collect::<Vec<_>>(),
        "no lock or temporary file remains"
    );
}
