use luuna_beacon::{Beacon, ListFilter};
use std::io::{BufWriter, Write};

#[test]
#[ignore = "hostile input: run under prlimit --as=4294967296 timeout 120"]
fn deeply_nested_frontmatter_can_be_read_written_and_dropped_on_a_default_stack() {
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let beacon = Beacon::open(dir.path()).unwrap();
    beacon.init().unwrap();
    let path = dir.path().join(".luuna/beacon/prompts.jsonl");
    let mut file = BufWriter::new(std::fs::File::create(&path).unwrap());
    file.write_all(br#"{"id":"depth-1","name":"deep","version":1,"frontmatter":"#)
        .unwrap();
    for _ in 1..9999 {
        file.write_all(br#"{"a":"#).unwrap();
    }
    file.write_all(b"{}").unwrap();
    for _ in 1..9999 {
        file.write_all(b"}").unwrap();
    }
    file.write_all(b"}\n").unwrap();
    file.flush().unwrap();
    drop(file);
    let before = std::fs::read(&path).unwrap();
    eprintln!("depth probe: reading");
    let records = beacon.list(&ListFilter::default()).unwrap();
    eprintln!("depth probe: read complete");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].name, "deep");
    let copy = records.clone();
    assert_eq!(records, copy);
    drop(copy);
    let encoded = records[0].to_go_json();
    assert!(encoded.contains("\"frontmatter\":{\"a\":"));
    eprintln!("depth probe: dropping");
    drop(records);
    eprintln!("depth probe: drop complete");
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    file.write_all(b"{\"frontmatter\":{\"bad\":1e999}}\n")
        .unwrap();
    drop(file);
    let with_bad_line = std::fs::read(&path).unwrap();
    assert!(beacon.list(&ListFilter::default()).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), with_bad_line);
}
