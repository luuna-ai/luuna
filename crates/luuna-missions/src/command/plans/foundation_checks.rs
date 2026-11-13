//! Preserved foundation inputs delegate their evolving plan expectations here.
use crate::{
    command::{run, Invocation},
    error::Error,
};
use std::{fs, path::Path};
pub(in crate::command) fn read(root: &Path, args: &[&str]) {
    match run(&Invocation::new(root, args.iter().copied())) {
        Err(Error::Unsupported { plan, .. }) => assert_eq!(plan, "s4-missions-lib-4"),
        other => panic!("expected Unsupported, got {other:?}"),
    }
}
fn files(root: &Path) -> (Vec<u8>, Vec<u8>) {
    let dir = root.join(".luuna/missions");
    (
        fs::read(dir.join("issues.jsonl")).unwrap(),
        fs::read(dir.join("plans.jsonl")).unwrap(),
    )
}
pub(in crate::command) fn update(root: &Path, args: &[&str], kind: u8) {
    let before = files(root);
    let mut all = vec!["update"];
    all.extend(args);
    match run(&Invocation::new(root, all)) {
        Err(Error::Unsupported { plan, feature }) => {
            assert_eq!(plan, "s4-missions-lib-4");
            if kind == 1 {
                assert!(feature.contains("p-1"), "{feature}");
            }
        }
        other => panic!("expected Unsupported, got {other:?}"),
    };
    assert_eq!(files(root), before, "{args:?}");
    if kind != 2 {
        assert!(fs::read_dir(root.join(".luuna/missions"))
            .unwrap()
            .all(|e| {
                let name = e.unwrap().file_name().to_string_lossy().into_owned();
                !name.contains(".lock") && !name.contains(".tmp")
            }));
    }
}
