//! Preflight the effective config and all retained stored values before any write.
use super::{merge_defaults, path_error, Config};
use crate::{
    json::{self, Value},
    store, yaml, AtlasError, Result,
};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};
fn refuse(path: &Path, reason: &str) -> AtlasError {
    AtlasError::Message(format!(
        "Refusing to rewrite {}: {reason}. The file is unchanged.",
        path.display()
    ))
}
fn data(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
        _ => true,
    }
}
fn at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut current = value;
    for part in path {
        current = current.as_object()?.get(part)?;
    }
    Some(current)
}
fn intentional(path: &[Vec<u8>], replaced: &[Vec<String>]) -> bool {
    replaced
        .iter()
        .any(|p| p.len() == path.len() && p.iter().zip(path).all(|(a, b)| a.as_bytes() == b))
}
fn covered(path: &[Vec<u8>], replaced: &[Vec<String>]) -> bool {
    replaced
        .iter()
        .any(|p| p.len() <= path.len() && p.iter().zip(path).all(|(a, b)| a.as_bytes() == b))
}
fn descendant(path: &[Vec<u8>], replaced: &[Vec<String>]) -> bool {
    replaced
        .iter()
        .any(|p| p.len() > path.len() && p.iter().zip(path).all(|(a, b)| a.as_bytes() == b))
}
fn spelling(path: &[Vec<u8>]) -> String {
    if path.is_empty() {
        "<root>".into()
    } else {
        path.iter()
            .map(|p| String::from_utf8_lossy(p))
            .collect::<Vec<_>>()
            .join(".")
    }
}
fn scalar_same(a: &Value, b: &Value) -> bool {
    if let (Some(a), Some(b)) = (a.string_bytes(), b.string_bytes()) {
        return a == b;
    }
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Number(a), Value::Number(b)) if !a.is_finite() || !b.is_finite() => {
            a == b || (a.is_nan() && b.is_nan())
        }
        (Value::Number(a), Value::Number(b)) if *a == 0.0 && *b == 0.0 => {
            a.is_sign_negative() == b.is_sign_negative()
        }
        (Value::Number(a), Value::Integer(b)) if *a == 0.0 && *b == 0 => !a.is_sign_negative(),
        (Value::Number(_) | Value::Integer(_), Value::Number(_) | Value::Integer(_)) => {
            json::to_compact_string(a).ok() == json::to_compact_string(b).ok()
        }
        _ => false,
    }
}
/// `replaced` lists only complete explicitly replaced/deleted paths, never a
/// scalar ancestor that a descendant set would overwrite implicitly.
pub fn check_lossless_rewrite(
    atlas_dir: &Path,
    next: &Config,
    replaced: &[Vec<String>],
) -> Result<()> {
    let path = atlas_dir.join("config.yaml");
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(path_error("read", &path, e)),
    };
    let text =
        std::str::from_utf8(&bytes).map_err(|_| refuse(&path, "stored YAML is not UTF-8"))?;
    let raw = yaml::parse(text)?;
    if yaml::has_later_data(text)? {
        return Err(refuse(&path, "later YAML document data would be discarded"));
    }
    if !matches!(raw, Value::Object(_)) && data(&raw) {
        return Err(refuse(
            &path,
            "the non-mapping document contains stored data",
        ));
    }
    if let Some(Value::Array(items)) = at(&raw, &["domains"]) {
        if !covered(&[b"domains".to_vec()], replaced)
            && items.iter().any(|v| v.string_bytes().is_none())
        {
            return Err(refuse(
                &path,
                "non-string domains list entries would be discarded",
            ));
        }
    }
    for parts in [
        &["domains"][..],
        &["governance"][..],
        &["classification_defaults"][..],
        &["classification_defaults", "shelf_life"][..],
    ] {
        let key = parts
            .iter()
            .map(|s| s.as_bytes().to_vec())
            .collect::<Vec<_>>();
        if covered(&key, replaced) {
            continue;
        }
        if let Some(value) = at(&raw, parts) {
            if parts == ["domains"] && matches!(value, Value::Array(_)) {
                continue;
            }
            if !matches!(value, Value::Object(_)) && data(value) {
                return Err(refuse(
                    &path,
                    &format!(
                        "stored {} data would be replaced by defaults",
                        parts.join(".")
                    ),
                ));
            }
        }
    }
    let mut before = raw.as_object().cloned().unwrap_or_default();
    merge_defaults(&mut before);
    let encoded = yaml::to_yaml_string(&Value::Object(next.clone()))?;
    let prospective = yaml::parse(&encoded)
        .map_err(|_| refuse(&path, "the rewritten YAML would not be readable"))?;
    let mut after = prospective.as_object().cloned().unwrap_or_default();
    merge_defaults(&mut after);
    let before = Value::Object(before);
    let after = Value::Object(after);
    let mut pending = vec![(Vec::<Vec<u8>>::new(), &before, Some(&after))];
    while let Some((path_parts, old, new)) = pending.pop() {
        if intentional(&path_parts, replaced) {
            continue;
        }
        match old {
            Value::Object(object) => {
                let next = new.and_then(Value::as_object);
                if object.is_empty()
                    && !matches!(new, Some(Value::Object(_)))
                    && !descendant(&path_parts, replaced)
                {
                    return Err(refuse(
                        &path,
                        &format!("stored {} would be lost", spelling(&path_parts)),
                    ));
                }
                for (key, value) in object.iter() {
                    let bytes = object.key_bytes(key);
                    let mut child = path_parts.clone();
                    child.push(bytes.to_vec());
                    pending.push((child, value, next.and_then(|o| o.get_go_key(bytes))));
                }
            }
            Value::Array(items) => {
                let next = new.and_then(Value::as_array);
                if items.is_empty() && !matches!(new, Some(Value::Array(_))) {
                    return Err(refuse(
                        &path,
                        &format!("stored {} would be lost", spelling(&path_parts)),
                    ));
                }
                for (index, value) in items.iter().enumerate() {
                    let mut child = path_parts.clone();
                    child.push(index.to_string().into_bytes());
                    pending.push((child, value, next.and_then(|a| a.get(index))));
                }
            }
            _ => {
                if !new.is_some_and(|new| scalar_same(old, new)) {
                    return Err(refuse(
                        &path,
                        &format!("stored {} would change or disappear", spelling(&path_parts)),
                    ));
                }
            }
        }
    }
    Ok(())
}
/// Go config set/unset uses CreateTemp (0600), unlike WriteConfig's truncate.
pub fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    write_atomic_with_suffix(path, data, || {
        format!(
            "{}{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )
    })
}
fn write_atomic_with_suffix(
    path: &Path,
    data: &[u8],
    mut suffix: impl FnMut() -> String,
) -> Result<()> {
    struct Temp(std::path::PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let mut sequence = 0;
    loop {
        let temporary = path.with_file_name(format!("{name}.tmp.{}", suffix()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = match options.open(&temporary) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && sequence < 10000 => {
                sequence += 1;
                continue;
            }
            Err(error) => return Err(path_error("open", &temporary, error)),
        };
        let temp = Temp(temporary);
        file.write_all(data)
            .map_err(|e| path_error("write", &temp.0, e))?;
        drop(file);
        store::rename_replacing(&temp.0, path).map_err(|source| AtlasError::Rename {
            from: temp.0.clone(),
            to: path.to_path_buf(),
            source,
        })?;
        return Ok(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn atomic_temp_collision_keeps_the_preexisting_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("config.yaml");
        let occupied = dir.path().join("config.yaml.tmp.occupied");
        fs::write(&occupied, b"held user data").unwrap();
        let mut first = true;
        write_atomic_with_suffix(&target, b"version: '1'\n", || {
            if std::mem::take(&mut first) {
                "occupied".into()
            } else {
                "fresh".into()
            }
        })
        .unwrap();
        assert_eq!(fs::read(&occupied).unwrap(), b"held user data");
        assert_eq!(fs::read(&target).unwrap(), b"version: '1'\n");
        assert!(!dir.path().join("config.yaml.tmp.fresh").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&target).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}
