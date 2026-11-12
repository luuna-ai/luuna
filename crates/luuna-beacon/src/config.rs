//! `config.yaml`: the bytes `init` writes and the lenient read of
//! `loadConfig` (`cli.go:155-160`, `1744-1763`), with YAML parsed by
//! yaml-rust2 (assumption AS8).

use crate::error::BeaconError;
use crate::paths;
use std::fs;
use std::path::Path;
use yaml_rust2::{Yaml, YamlLoader};

/// The project name used when config.yaml does not give one.
pub(crate) const DEFAULT_PROJECT: &str = "luuna-beacon";

/// The config version used when config.yaml does not give one.
pub(crate) const DEFAULT_VERSION: &str = "1";

/// What Go's `yaml.Marshal` writes for the default config `init` creates:
/// `model.Config{Project: "luuna-beacon", Version: "1", Targets:
/// {"default": {Dir: "agents", Default: true}}}`, in field order with
/// yaml.v3's four-space indent; `"1"` is quoted because unquoted it would
/// read as an integer.
pub(crate) const DEFAULT_CONFIG_YAML: &str = concat!(
    "project: luuna-beacon\n",
    "version: \"1\"\n",
    "targets:\n",
    "    default:\n",
    "        dir: agents\n",
    "        default: true\n",
);

/// The part of config.yaml this library reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Config {
    pub(crate) project: String,
    pub(crate) version: String,
}

/// Go's `loadConfig` as far as `project` and `version`: runs the legacy
/// migration first (as `paths.Config` does), reads
/// `<root>/.luuna/beacon/config.yaml` (an unreadable or missing file is an
/// error), parses it as YAML (invalid YAML, or a top level that is neither a
/// mapping nor empty, is an error), and takes `project` and `version` when
/// they are non-empty strings, otherwise the defaults.
pub(crate) fn load(root: &Path) -> Result<Config, BeaconError> {
    paths::migrate_legacy(root)?;
    let path = paths::config_file(root);
    let content = fs::read(&path).map_err(|error| BeaconError::os("open", &path, error))?;
    parse(&content)
}

/// Parses config.yaml content as `loadConfig` does.
pub(crate) fn parse(content: &[u8]) -> Result<Config, BeaconError> {
    let text = std::str::from_utf8(content)
        .map_err(|error| BeaconError::Yaml(format!("invalid UTF-8: {error}")))?;
    let documents =
        YamlLoader::load_from_str(text).map_err(|error| BeaconError::Yaml(error.to_string()))?;
    // yaml.v3's Unmarshal decodes the first document only; no document, an
    // empty one or an explicit null leaves the map nil.
    let mapping = match documents.into_iter().next() {
        None | Some(Yaml::Null | Yaml::BadValue) => None,
        Some(Yaml::Hash(mapping)) => Some(mapping),
        Some(other) => {
            return Err(BeaconError::Yaml(format!(
                "cannot unmarshal {} into a mapping",
                kind(&other)
            )))
        }
    };
    let text_of = |key: &str| {
        mapping
            .as_ref()
            .and_then(|mapping| mapping.get(&Yaml::String(key.to_owned())))
            .and_then(|value| match value {
                Yaml::String(text) if !text.is_empty() => Some(text.clone()),
                _ => None,
            })
    };
    Ok(Config {
        project: text_of("project").unwrap_or_else(|| DEFAULT_PROJECT.to_owned()),
        version: text_of("version").unwrap_or_else(|| DEFAULT_VERSION.to_owned()),
    })
}

fn kind(value: &Yaml) -> &'static str {
    match value {
        Yaml::Array(_) => "a sequence",
        Yaml::String(_) => "a string",
        Yaml::Integer(_) => "an integer",
        Yaml::Real(_) => "a float",
        Yaml::Boolean(_) => "a boolean",
        _ => "a value",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(text: &str) -> String {
        parse(text.as_bytes())
            .unwrap_or_else(|error| panic!("{text:?}: {error}"))
            .project
    }

    #[test]
    fn default_config_reads_back_as_the_defaults() {
        let config = parse(DEFAULT_CONFIG_YAML.as_bytes()).unwrap();
        assert_eq!(
            config,
            Config {
                project: DEFAULT_PROJECT.to_owned(),
                version: DEFAULT_VERSION.to_owned(),
            }
        );
    }

    #[test]
    fn project_falls_back_to_the_default() {
        assert_eq!(project("project: demo-x\nversion: \"1\"\n"), "demo-x");
        assert_eq!(project("project: 'demo-q'\nversion: 1\n"), "demo-q");
        assert_eq!(
            project("# header\nproject: demo-c # trailing\ntargets: {default: {dir: agents, default: true}}\n"),
            "demo-c"
        );
        assert_eq!(project("project: \"123\"\n"), "123");
        assert_eq!(project("project: yes\n"), "yes");
        assert_eq!(project("base: &b demo-a\nproject: *b\n"), "demo-a");
        assert_eq!(project("project: one\n---\nproject: two\n"), "one");
        for text in [
            "project: 123\nversion: \"1\"\n",
            "project: 1.5\n",
            "project: true\nversion: \"1\"\n",
            "project: ''\nversion: \"1\"\n",
            "project:\nversion: \"1\"\n",
            "project: ~\n",
            "project: [a]\n",
            "project: {a: b}\n",
            "version: \"1\"\n",
            "",
            "# only a comment\n",
            "---\n",
            "null\n",
        ] {
            assert_eq!(project(text), DEFAULT_PROJECT, "{text:?}");
        }
        let config = parse(b"version: 2\n").unwrap();
        assert_eq!(
            config.version, DEFAULT_VERSION,
            "a numeric version falls back"
        );
        let config = parse(b"version: \"2\"\n").unwrap();
        assert_eq!(config.version, "2");
    }

    #[test]
    fn invalid_yaml_is_an_error() {
        for text in [
            "project: [unclosed\n",
            "- a\n- b\n",
            "project: a: b\n",
            "\t\n  bad: [\n",
            "just a string\n",
            "42\n",
            "project: a\nproject: b\n",
        ] {
            let error = parse(text.as_bytes()).expect_err(text);
            assert!(matches!(error, BeaconError::Yaml(_)), "{text:?}: {error}");
        }
        assert!(matches!(
            parse(b"project: \xff\n"),
            Err(BeaconError::Yaml(_))
        ));
    }

    #[test]
    fn a_missing_config_is_an_open_error() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let error = load(&root).unwrap_err();
        assert!(
            error.to_string().starts_with(&format!(
                "open {}: ",
                root.join(".luuna/beacon/config.yaml").display()
            )),
            "{error}"
        );
        fs::create_dir_all(paths::store_dir(&root)).unwrap();
        fs::write(paths::config_file(&root), DEFAULT_CONFIG_YAML).unwrap();
        assert_eq!(load(&root).unwrap().project, DEFAULT_PROJECT);
    }
}
