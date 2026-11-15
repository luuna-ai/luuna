//! The stored artifact records, one JSON object per line in
//! `artifacts.jsonl`, written byte for byte as Go's `json.Marshal` writes
//! `model.Artifact` and read by Go's `encoding/json` rules
//! (`internal/model/model.go:113-129`).

use crate::gojson::{self, GoValue, ObjectWriter};
use crate::model::Record;

/// One file of an artifact.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArtifactFile {
    pub path: String,
    pub encoding: String,
    pub content: String,
    pub sha256: String,
    pub executable: bool,
}

/// One version of an artifact, as stored in `artifacts.jsonl`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Artifact {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub version: i64,
    pub files: Vec<ArtifactFile>,
    pub sha256: String,
    pub created_at: String,
}

const FILE_FIELDS: &[&str] = &["path", "encoding", "content", "sha256", "executable"];
const ARTIFACT_FIELDS: &[&str] = &[
    "id",
    "kind",
    "name",
    "version",
    "files",
    "sha256",
    "createdAt",
];

fn write_file(out: &mut String, file: &ArtifactFile) {
    let mut object = ObjectWriter::new(out);
    object.string("path", &file.path);
    object.string("encoding", &file.encoding);
    object.string("content", &file.content);
    object.string("sha256", &file.sha256);
    gojson::write_bool(object.key("executable"), file.executable);
    object.finish();
}

fn file_into(file: &mut ArtifactFile, value: &GoValue) -> Result<(), String> {
    let Some(entries) = gojson::object_entries(value, "model.ArtifactFile")? else {
        return Ok(());
    };
    for (key, item) in entries {
        let Some(name) = gojson::field(key, FILE_FIELDS) else {
            continue;
        };
        match name {
            "path" => gojson::string_into(&mut file.path, item)?,
            "encoding" => gojson::string_into(&mut file.encoding, item)?,
            "content" => gojson::string_into(&mut file.content, item)?,
            "sha256" => gojson::string_into(&mut file.sha256, item)?,
            "executable" => gojson::bool_into(&mut file.executable, item)?,
            _ => unreachable!("every artifact file field is handled"),
        }
    }
    Ok(())
}

impl Artifact {
    /// The line Go's `json.Marshal` writes for this record, without the
    /// newline: fields `id`, `kind`, `name`, `version`, `files`, `sha256`,
    /// `createdAt`, in that order.
    pub fn to_go_json(&self) -> String {
        let mut out = String::with_capacity(
            256 + self
                .files
                .iter()
                .map(|file| file.content.len() + file.path.len() + 160)
                .sum::<usize>(),
        );
        let mut object = ObjectWriter::new(&mut out);
        object.string("id", &self.id);
        object.string("kind", &self.kind);
        object.string("name", &self.name);
        object.int("version", self.version);
        {
            let files = object.key("files");
            files.push('[');
            for (index, file) in self.files.iter().enumerate() {
                if index > 0 {
                    files.push(',');
                }
                write_file(files, file);
            }
            files.push(']');
        }
        object.string("sha256", &self.sha256);
        object.string("createdAt", &self.created_at);
        object.finish();
        out
    }

    /// Decodes one stored line as Go's `json.Unmarshal` into
    /// `model.Artifact`: keys match case-insensitively, unknown keys are
    /// ignored, `null` leaves a value unchanged.
    pub(crate) fn from_go_json(line: &[u8]) -> Result<Artifact, String> {
        let value = gojson::parse(line)?;
        let mut artifact = Artifact::default();
        let Some(entries) = gojson::object_entries(&value, "model.Artifact")? else {
            return Ok(artifact);
        };
        for (key, item) in entries {
            let Some(name) = gojson::field(key, ARTIFACT_FIELDS) else {
                continue;
            };
            match name {
                "id" => gojson::string_into(&mut artifact.id, item)?,
                "kind" => gojson::string_into(&mut artifact.kind, item)?,
                "name" => gojson::string_into(&mut artifact.name, item)?,
                "version" => gojson::int_into(&mut artifact.version, item)?,
                "files" => {
                    let mut files = Some(std::mem::take(&mut artifact.files));
                    let result = gojson::slice_into(&mut files, item, file_into);
                    artifact.files = files.unwrap_or_default();
                    result?;
                }
                "sha256" => gojson::string_into(&mut artifact.sha256, item)?,
                "createdAt" => gojson::string_into(&mut artifact.created_at, item)?,
                _ => unreachable!("every artifact field is handled"),
            }
        }
        Ok(artifact)
    }
}

impl Record for Artifact {
    fn from_go_json(line: &[u8]) -> Result<Self, String> {
        Artifact::from_go_json(line)
    }
    fn to_go_json(&self) -> String {
        Artifact::to_go_json(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_line_round_trips_byte_for_byte() {
        let line = concat!(
            "{\"id\":\"artifact-1\",\"kind\":\"skill\",\"name\":\"n\",\"version\":3,",
            "\"files\":[",
            "{\"path\":\"SKILL.md\",\"encoding\":\"utf8\",",
            "\"content\":\"\\u003cb\\u003e\\u0026 \\u2028\\u2029 \\\"q\\\" \\\\ \\t\\r\\n\\u0001\u{7f} é 🌙\",",
            "\"sha256\":\"abc\",\"executable\":true},",
            "{\"path\":\"b.bin\",\"encoding\":\"base64\",\"content\":\"AP/+\",",
            "\"sha256\":\"def\",\"executable\":false}],",
            "\"sha256\":\"manifest\",\"createdAt\":\"2001-02-03T04:05:06.5Z\"}"
        );
        let artifact = Artifact::from_go_json(line.as_bytes()).unwrap();
        assert_eq!(artifact.version, 3);
        assert_eq!(artifact.files.len(), 2);
        assert!(artifact.files[0].executable);
        assert_eq!(
            artifact.files[0].content,
            "<b>& \u{2028}\u{2029} \"q\" \\ \t\r\n\u{1}\u{7f} é 🌙"
        );
        assert_eq!(artifact.to_go_json(), line);
        assert_eq!(<Artifact as Record>::to_go_json(&artifact), line);

        let empty = Artifact::default();
        assert_eq!(
            empty.to_go_json(),
            "{\"id\":\"\",\"kind\":\"\",\"name\":\"\",\"version\":0,\"files\":[],\"sha256\":\"\",\"createdAt\":\"\"}"
        );
        assert_eq!(
            Artifact::from_go_json(empty.to_go_json().as_bytes()),
            Ok(empty)
        );
    }

    #[test]
    fn keys_are_matched_case_insensitively_and_unknown_keys_ignored() {
        let line = br#"{"ID":"a","KIND":"agent","Name":"n","VERSION":2,"extra":[1,{"x":null}],
            "Files":[{"PATH":"p","Encoding":"base64","CONTENT":"AA==","SHA256":"s","Executable":true,"more":1}],
            "Sha256":"m","CREATEDAT":"t","createdat":"t2"}"#;
        let artifact = Artifact::from_go_json(line).unwrap();
        assert_eq!(artifact.id, "a");
        assert_eq!(artifact.kind, "agent");
        assert_eq!(artifact.name, "n");
        assert_eq!(artifact.version, 2);
        assert_eq!(artifact.sha256, "m");
        assert_eq!(artifact.created_at, "t2", "a later repeated key wins");
        assert_eq!(
            artifact.files,
            vec![ArtifactFile {
                path: "p".into(),
                encoding: "base64".into(),
                content: "AA==".into(),
                sha256: "s".into(),
                executable: true,
            }]
        );
        // null leaves a value as it was; a type mismatch is an error.
        let artifact = Artifact::from_go_json(br#"{"name":"n","name":null,"files":null}"#).unwrap();
        assert_eq!(artifact.name, "n");
        assert!(artifact.files.is_empty());
        assert!(Artifact::from_go_json(br#"{"version":"1"}"#).is_err());
        assert!(Artifact::from_go_json(br#"{"files":[1]}"#).is_err());
        assert_eq!(Artifact::from_go_json(b"null"), Ok(Artifact::default()));
        assert!(Artifact::from_go_json(b"[]").is_err());
    }
}
