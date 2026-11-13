//! Full tree I/O, exact JSON and atomic replacement with A8 preflight.
use super::{emit, legacy, yaml, TreeValue as V};
use crate::{
    error::{Error, Result},
    json,
};
use std::{collections::BTreeMap, fs, io::Write, path::Path};
pub(crate) fn command(message: impl Into<String>) -> Error {
    Error::Command {
        message: message.into(),
    }
}
impl V {
    pub(crate) fn get(&self, key: &str) -> Option<&V> {
        if let V::Map(m) = self {
            m.iter().find(|(k, _)| k == key).map(|(_, v)| v)
        } else {
            None
        }
    }
    pub(crate) fn get_mut(&mut self, key: &str) -> Option<&mut V> {
        if let V::Map(m) = self {
            m.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v)
        } else {
            None
        }
    }
    pub(crate) fn set(&mut self, key: String, value: V) {
        if let V::Map(m) = self {
            if let Some((_, v)) = m.iter_mut().find(|(k, _)| k == &key) {
                *v = value;
            } else {
                m.push((key, value));
            }
        }
    }
    pub(crate) fn text(&self) -> Option<&str> {
        if let V::Str(s) = self {
            Some(s)
        } else {
            None
        }
    }
}
pub(crate) fn read_tree(dir: &Path) -> Result<V> {
    let path = dir.join("config.yaml");
    let data = fs::read(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            command(format!("Config not found at {}", path.display()))
        } else {
            Error::io("read", &path, e)
        }
    })?;
    let value = yaml::decode_document(&data)
        .map_err(|e| yaml::in_file(&path, e))?
        .unwrap_or(V::Map(Vec::new()));
    Ok(legacy::upgrade_legacy_config_keys(value))
}
pub(crate) fn display_tree(dir: &Path, value: &V) -> Result<String> {
    let path = dir.join("config.yaml");
    let bytes = fs::read(&path).map_err(|e| Error::io("read", &path, e))?;
    emit::stringify_tree(value, &yaml::key_orders(&bytes)?)
}
pub(crate) fn json_bytes(v: &V, html: bool) -> Result<Vec<u8>> {
    fn add(v: &V, html: bool, out: &mut Vec<u8>) -> Result<()> {
        match v {
            V::Null => out.extend_from_slice(b"null"),
            V::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
            V::Int(n) => out.extend_from_slice(n.to_string().as_bytes()),
            V::Uint(n) => out.extend_from_slice(n.to_string().as_bytes()),
            V::Float(n) => out.extend_from_slice(
                json::format_float(*n)
                    .map_err(|e| command(e.to_string()))?
                    .as_bytes(),
            ),
            V::Str(s) => json::quote(out, s, html),
            V::Seq(items) => {
                out.push(b'[');
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(b',');
                    }
                    add(v, html, out)?;
                }
                out.push(b']');
            }
            V::Map(entries) => {
                let sorted = entries
                    .iter()
                    .map(|(k, v)| (k, v))
                    .collect::<BTreeMap<_, _>>();
                out.push(b'{');
                for (i, (k, v)) in sorted.into_iter().enumerate() {
                    if i > 0 {
                        out.push(b',');
                    }
                    json::quote(out, k, html);
                    out.push(b':');
                    add(v, html, out)?;
                }
                out.push(b'}');
            }
            V::Other(t) => return Err(command(format!("json: unsupported type: {t}"))),
        }
        Ok(())
    }
    let mut out = Vec::new();
    add(v, html, &mut out)?;
    Ok(out)
}
fn legacy_collision(v: &V, path: &str) -> Option<String> {
    match v {
        V::Map(m) => {
            if v.get("mulch_source").is_some() && v.get("atlas_source").is_some() {
                return Some(if path.is_empty() {
                    "(root)".into()
                } else {
                    path.into()
                });
            }
            for (k, v) in m {
                if let Some(p) = legacy_collision(
                    v,
                    &if path.is_empty() {
                        k.clone()
                    } else {
                        format!("{path}.{k}")
                    },
                ) {
                    return Some(p);
                }
            }
        }
        V::Seq(seq) => {
            for (i, v) in seq.iter().enumerate() {
                if let Some(p) = legacy_collision(v, &format!("{path}.{i}")) {
                    return Some(p);
                }
            }
        }
        _ => {}
    }
    None
}
fn equal(a: &V, b: &V) -> bool {
    match (a, b) {
        (V::Int(a), V::Float(b)) | (V::Float(b), V::Int(a)) => {
            *b >= i64::MIN as f64
                && *b < 9_223_372_036_854_775_808.0
                && b.fract() == 0.0
                && *a == *b as i64
        }
        (V::Map(a), V::Map(b)) => {
            a.len() == b.len()
                && a.iter().all(|(k, v)| {
                    b.iter()
                        .find(|(key, _)| key == k)
                        .is_some_and(|(_, other)| equal(v, other))
                })
        }
        (V::Seq(a), V::Seq(b)) => a.len() == b.len() && a.iter().zip(b).all(|(a, b)| equal(a, b)),
        _ => a == b,
    }
}
fn bad_scalar(value: &V, path: &str) -> Option<(bool, String, String)> {
    match value {
        V::Map(entries) => {
            for (key, value) in entries {
                let dotted = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                let emitted = emit::format_key(key);
                let source = format!("outer:\n  {emitted}: null\n");
                let key_ok = yaml::decode_document(source.as_bytes())
                    .ok()
                    .flatten()
                    .and_then(|v| v.get("outer").and_then(|m| m.get(key)).cloned())
                    .is_some();
                if !key_ok {
                    return Some((true, dotted, emitted));
                }
                if let Some(x) = bad_scalar(value, &dotted) {
                    return Some(x);
                }
            }
        }
        V::Seq(values) => {
            for (i, v) in values.iter().enumerate() {
                if let Some(x) = bad_scalar(v, &format!("{path}.{i}")) {
                    return Some(x);
                }
            }
        }
        V::Null => {}
        v => {
            if let Ok(emitted) = emit::scalar(v) {
                let reread = yaml::decode_document(format!("value: {emitted}\n").as_bytes())
                    .ok()
                    .flatten();
                if !reread
                    .as_ref()
                    .and_then(|v| v.get("value"))
                    .is_some_and(|other| equal(v, other))
                {
                    return Some((false, path.into(), emitted));
                }
            }
        }
    }
    None
}
pub(crate) fn write_tree(dir: &Path, value: &V) -> Result<()> {
    let path = dir.join("config.yaml");
    let old = match fs::read(&path) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(Error::io("read", &path, e)),
    };
    let orders = yaml::key_orders(&old).unwrap_or_default();
    let content = emit::stringify_tree(value, &orders)?;
    if yaml::has_later_document(&old)? {
        return Err(command(format!("Refusing to rewrite {}: it holds more than one YAML document, and the Go tool would keep only the first. Nothing was written.",path.display())));
    }
    if let Some(raw) = yaml::decode_document(&old)? {
        if let Some(at) = legacy_collision(&raw, "") {
            return Err(command(format!("Refusing to rewrite {}: {at} holds both mulch_source and atlas_source, and the Go tool would drop mulch_source. Nothing was written.",path.display())));
        }
    }
    let reread = yaml::decode_document(content.as_bytes());
    if !reread
        .as_ref()
        .ok()
        .and_then(|v| v.as_ref())
        .is_some_and(|other| equal(value, other))
    {
        let (key, at, text) =
            bad_scalar(value, "").unwrap_or((false, "(root)".into(), content.trim_end().into()));
        return Err(command(format!("Refusing to write {}: the {} at {at} would be written as {text}, which reads back as a different value. Nothing was written.",path.display(),if key{"key"}else{"value"})));
    }
    let metadata = fs::metadata(&path).ok();
    let mut attempt = 0;
    let (tmp, mut file) = loop {
        let mut random = [0; 8];
        crate::model::random_bytes(&mut random);
        let suffix = crate::model::hex(&random);
        let tmp = dir.join(format!("config.yaml.tmp.{suffix}"));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&tmp) {
            Ok(file) => break (tmp, file),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && attempt < 10 => {
                attempt += 1;
            }
            Err(e) => return Err(Error::io("create", tmp, e)),
        }
    };
    let result = (|| {
        file.write_all(content.as_bytes())
            .map_err(|e| Error::io("write", &tmp, e))?;
        if let Some(m) = metadata {
            file.set_permissions(m.permissions())
                .map_err(|e| Error::io("chmod", &tmp, e))?;
        }
        drop(file);
        fs::rename(&tmp, &path).map_err(|e| Error::io("rename", &path, e))
    })();
    if result.is_err() {
        let _ = fs::remove_file(tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn config_tree_schema_and_order() {
        let t = crate::config_tree::tree::test_support::project("version: \"1\"\nproject: demo\n");
        let dir = t.path().join(".luuna/missions");
        let tree = read_tree(&dir).unwrap();
        assert_eq!(
            display_tree(&dir, &tree).unwrap(),
            "version: \"1\"\nproject: demo\n"
        );
        let schema = include_bytes!("../../assets/s4-missions-lib-3/schema.json");
        assert!(json::is_valid(schema));
        assert_eq!(
            json_bytes(&V::Int(9_007_199_254_740_993), false).unwrap(),
            b"9007199254740993"
        );
    }
    #[test]
    fn write_tree_is_atomic_and_keeps_the_mode() {
        for mode in [0o644, 0o600] {
            let t =
                crate::config_tree::tree::test_support::project("project: demo\nversion: \"1\"\n");
            let file = crate::config_tree::tree::test_support::config(t.path());
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&file, fs::Permissions::from_mode(mode)).unwrap();
            }
            #[cfg(not(unix))]
            let _ = mode;
            let permissions = fs::metadata(&file).unwrap().permissions();
            let mut tree = read_tree(file.parent().unwrap()).unwrap();
            tree.set("project".into(), V::Str("other".into()));
            let store = crate::store::Store::open(file.parent().unwrap()).unwrap();
            let lock = store.lock(crate::store::DataFile::Config).unwrap();
            write_tree(file.parent().unwrap(), &tree).unwrap();
            drop(lock);
            assert_eq!(fs::metadata(&file).unwrap().permissions(), permissions);
            assert_eq!(
                fs::read_to_string(&file).unwrap(),
                "project: other\nversion: \"1\"\n"
            );
            assert_eq!(fs::read_dir(file.parent().unwrap()).unwrap().count(), 1);
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use crate::command::{run, Invocation, Outcome};
    use std::{fs, path::Path};
    pub fn project(content: &str) -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        store(t.path(), content);
        t
    }
    pub fn store(root: &Path, content: &str) {
        let dir = root.join(".luuna/missions");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("config.yaml"), content).unwrap();
    }
    pub fn invoke(root: &Path, args: &[&str]) -> Outcome {
        run(&Invocation::new(root, args.iter().copied())).unwrap()
    }
    pub fn config(root: &Path) -> std::path::PathBuf {
        root.join(".luuna/missions/config.yaml")
    }
}
