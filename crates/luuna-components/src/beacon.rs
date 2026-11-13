//! Bounded, in-process Beacon adapter. Unsupported operations never use a CLI.
use super::{
    adopt_legacy_layout, check_project_data_links, prepare_workspace, ComponentError, Result,
    MAX_STDOUT,
};
use luuna_beacon::{Beacon, CreatePrompt, CreateSchema, ListFilter, Rule, Section, UpdatePrompt};
use serde_json::{json, Value};
use std::{collections::BTreeMap, fs, io::Read, path::Path, sync::Mutex};

const MAX_INPUT: usize = 64 * 1024 * 1024;
// The gate lives in the blocking job: dropping an HTTP future cannot admit a
// second native call while an earlier writer is still completing. The runtime's
// durable intent marker reconciles uncertain completion as it did for the CLI.
static NATIVE_GATE: Mutex<()> = Mutex::new(());

/// Runtime Beacon calls need neither Bun nor a component source checkout.
/// Work is awaited on a blocking thread, with no synthetic timeout that could
/// falsely imply a running native writer was cancelled. No child is launched.
pub async fn call_beacon_native(workspace: &Path, args: &[String]) -> Result<Value> {
    let request = Request::parse(args)?;
    let root = workspace.to_owned();
    tokio::task::spawn_blocking(move || {
        // The mutex protects admission, not mutable state. A caught library
        // panic must not permanently disable every unrelated prompt store.
        let _guard = NATIVE_GATE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let root = prepare_workspace(&root)?;
        adopt_legacy_layout(&root)?;
        if !matches!(request, Request::Init) && !root.join(".luuna/beacon").is_dir() {
            return Err(ComponentError::Workspace);
        }
        check_project_data_links(&root, ".luuna/beacon")?;
        preflight(&root)?;
        let beacon = Beacon::open(&root).map_err(|_| ComponentError::Workspace)?;
        let value = request.execute(&beacon)?;
        if serde_json::to_vec(&value)
            .map_err(|_| ComponentError::Protocol)?
            .len()
            > MAX_STDOUT
        {
            return Err(ComponentError::OutputLimit);
        }
        Ok(value)
    })
    .await
    .map_err(|_| ComponentError::Refused)?
}

fn preflight(root: &Path) -> Result<()> {
    let mut remaining = MAX_INPUT;
    for name in ["config.yaml", "prompts.jsonl", "schemas.jsonl"] {
        let path = root.join(".luuna/beacon").join(name);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(ComponentError::Workspace),
        };
        if !metadata.is_file() {
            return Err(ComponentError::Workspace);
        }
        let mut file = match fs::File::open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(ComponentError::Workspace),
        };
        let metadata = file.metadata().map_err(|_| ComponentError::Workspace)?;
        if !metadata.is_file() {
            return Err(ComponentError::Workspace);
        }
        if metadata.len() > remaining as u64 {
            return Err(ComponentError::OutputLimit);
        }
        let mut bytes = Vec::new();
        file.by_ref()
            .take(remaining as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| ComponentError::Workspace)?;
        if bytes.len() > remaining {
            return Err(ComponentError::OutputLimit);
        }
        remaining -= bytes.len();
        if name.ends_with(".jsonl") {
            for line in bytes.split(|b| *b == b'\n') {
                if std::str::from_utf8(line).is_ok_and(|s| s.trim().is_empty()) {
                    continue;
                }
                // This bounds the JSON structure later materialised for an API
                // answer, before any write can append a new current record.
                serde_json::from_slice::<Value>(line).map_err(|_| ComponentError::Protocol)?;
            }
        }
    }
    Ok(())
}
fn parsed(text: &str) -> Result<Value> {
    if text.len() > MAX_STDOUT {
        return Err(ComponentError::OutputLimit);
    }
    serde_json::from_str(text).map_err(|_| ComponentError::Protocol)
}
fn record(prompt: &luuna_beacon::Prompt) -> Result<Value> {
    parsed(&prompt.to_go_json())
}
fn refused<T>(value: std::result::Result<T, luuna_beacon::BeaconError>) -> Result<T> {
    value.map_err(|_| ComponentError::Refused)
}

struct Flags {
    values: BTreeMap<String, Vec<String>>,
}
impl Flags {
    fn parse(args: &[String], allowed: &[&str]) -> Result<Self> {
        let mut values: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut i = 0;
        while i < args.len() {
            let key = args[i].as_str();
            if !allowed.contains(&key) || i + 1 == args.len() {
                return Err(ComponentError::Protocol);
            }
            values
                .entry(key.into())
                .or_default()
                .push(args[i + 1].clone());
            i += 2
        }
        Ok(Self { values })
    }
    fn one(&self, key: &str) -> String {
        self.values
            .get(key)
            .and_then(|v| v.first())
            .cloned()
            .unwrap_or_default()
    }
    fn optional(&self, key: &str) -> Option<String> {
        self.values.get(key).and_then(|v| v.first()).cloned()
    }
    fn many(&self, key: &str) -> Vec<String> {
        self.values.get(key).cloned().unwrap_or_default()
    }
    fn pairs(&self, key: &str) -> Vec<(String, String)> {
        self.many(key)
            .into_iter()
            .filter_map(|s| s.split_once('=').map(|(a, b)| (a.into(), b.into())))
            .collect()
    }
    fn sections(&self) -> Vec<Section> {
        self.pairs("--section")
            .into_iter()
            .map(|(name, body)| Section {
                name,
                body,
                required: None,
            })
            .collect()
    }
}
const CREATE_FLAGS: &[&str] = &[
    "--name",
    "--description",
    "--extends",
    "--schema",
    "--emit-as",
    "--emit-dir",
    "--status",
    "--mixin",
    "--tag",
    "--section",
    "--fm",
];
const UPDATE_FLAGS: &[&str] = &[
    "--name",
    "--description",
    "--extends",
    "--schema",
    "--emit-as",
    "--emit-dir",
    "--status",
    "--mixin",
    "--tag",
    "--section",
    "--fm",
    "--untag",
    "--remove-section",
    "--remove-mixin",
    "--remove-fm",
];
enum Request {
    Init,
    List(ListFilter),
    Create(CreatePrompt),
    Update(String, UpdatePrompt),
    Render(String),
    Validate(String),
    SchemaCreate(CreateSchema),
    SchemaShow(String),
    SchemaList,
    RuleAdd(String, Rule),
}
impl Request {
    fn parse(args: &[String]) -> Result<Self> {
        if args.is_empty()
            || args.len() > 128
            || args
                .iter()
                .any(|s| s.len() > 256 * 1024 || s.contains('\0'))
            || args.iter().map(String::len).sum::<usize>() > MAX_STDOUT
        {
            return Err(ComponentError::Protocol);
        }
        let positional = |at: usize| {
            args.get(at)
                .filter(|s| !s.starts_with("--") && !s.is_empty())
                .cloned()
                .ok_or(ComponentError::Protocol)
        };
        match args[0].as_str() {
            "init" if args.len() == 1 => Ok(Self::Init),
            "list" => {
                let f = Flags::parse(&args[1..], &["--status", "--tag", "--extends", "--mixin"])?;
                Ok(Self::List(ListFilter {
                    status: f.optional("--status"),
                    tag: f.optional("--tag"),
                    extends: f.optional("--extends"),
                    mixin: f.optional("--mixin"),
                }))
            }
            "create" => {
                let f = Flags::parse(&args[1..], CREATE_FLAGS)?;
                Ok(Self::Create(CreatePrompt {
                    name: f.one("--name"),
                    description: f.one("--description"),
                    extends: f.one("--extends"),
                    schema: f.one("--schema"),
                    emit_as: f.one("--emit-as"),
                    emit_dir: f.one("--emit-dir"),
                    status: f.one("--status"),
                    mixins: f.many("--mixin"),
                    tags: f.many("--tag"),
                    sections: f.sections(),
                    frontmatter: f.pairs("--fm"),
                }))
            }
            "update" => {
                let name = positional(1)?;
                let f = Flags::parse(&args[2..], UPDATE_FLAGS)?;
                Ok(Self::Update(
                    name,
                    UpdatePrompt {
                        name: f.optional("--name"),
                        description: f.optional("--description"),
                        status: f.optional("--status"),
                        extends: f.optional("--extends"),
                        schema: f.optional("--schema"),
                        emit_as: f.optional("--emit-as"),
                        emit_dir: f.optional("--emit-dir"),
                        add_tags: f.many("--tag"),
                        remove_tags: f.many("--untag"),
                        sections: f.sections(),
                        remove_sections: f.many("--remove-section"),
                        add_mixins: f.many("--mixin"),
                        remove_mixins: f.many("--remove-mixin"),
                        frontmatter: f.pairs("--fm"),
                        remove_frontmatter: f.many("--remove-fm"),
                    },
                ))
            }
            "render" if args.len() == 2 => Ok(Self::Render(positional(1)?)),
            "validate" if args.len() == 2 => Ok(Self::Validate(positional(1)?)),
            "schema" => match args.get(1).map(String::as_str) {
                Some("create") => {
                    let f = Flags::parse(&args[2..], &["--name", "--required", "--optional"])?;
                    Ok(Self::SchemaCreate(CreateSchema {
                        name: f.one("--name"),
                        required: f.many("--required"),
                        optional: f.many("--optional"),
                    }))
                }
                Some("show") if args.len() == 3 => Ok(Self::SchemaShow(positional(2)?)),
                Some("list") if args.len() == 2 => Ok(Self::SchemaList),
                Some("rule") if args.get(2).map(String::as_str) == Some("add") => {
                    let name = positional(3)?;
                    let f = Flags::parse(&args[4..], &["--section", "--pattern", "--message"])?;
                    Ok(Self::RuleAdd(
                        name,
                        Rule {
                            section: f.one("--section"),
                            pattern: f.one("--pattern"),
                            message: f.one("--message"),
                        },
                    ))
                }
                _ => Err(ComponentError::Protocol),
            },
            _ => Err(ComponentError::Protocol),
        }
    }
    fn execute(self, b: &Beacon) -> Result<Value> {
        match self {
            Self::Init => {
                let init = refused(b.init())?;
                Ok(json!({"success":true,"command":"init","dir":init.dir}))
            }
            Self::Create(input) => {
                let p = refused(b.create(&input))?;
                let value = record(&p)?;
                Ok(
                    json!({"success":true,"command":"create","id":p.id,"name":p.name,"version":p.version,"prompt":value}),
                )
            }
            Self::Update(name, input) => {
                let p = refused(b.update(&name, &input))?;
                let value = record(&p)?;
                Ok(
                    json!({"success":true,"command":"update","name":p.name,"version":p.version,"prompt":value}),
                )
            }
            Self::List(filter) => {
                let rows = refused(b.list(&filter))?;
                let mut text = String::from("[");
                for (i, p) in rows.iter().enumerate() {
                    let item = p.to_go_json();
                    if text.len() + item.len() + 2 > MAX_STDOUT {
                        return Err(ComponentError::OutputLimit);
                    }
                    if i > 0 {
                        text.push(',')
                    }
                    text.push_str(&item)
                }
                text.push(']');
                let rows = parsed(&text)?;
                let count = rows.as_array().ok_or(ComponentError::Protocol)?.len();
                Ok(json!({"success":true,"command":"list","prompts":rows,"count":count}))
            }
            Self::Render(name) => {
                let r = refused(b.render(&name))?;
                let mut raw = record(&r.prompt)?;
                let raw = raw.as_object_mut().ok_or(ComponentError::Protocol)?;
                let sections = raw.remove("sections").unwrap_or(Value::Null);
                let frontmatter = raw.remove("frontmatter").unwrap_or(Value::Null);
                let mut result = json!({"success":true,"command":"render","name":r.prompt.name,"version":r.prompt.version,"sections":sections,"frontmatter":frontmatter,"resolvedFrom":r.resolved_from});
                if let Some(atlas) = raw.remove("atlas") {
                    result
                        .as_object_mut()
                        .unwrap()
                        .insert("atlas".into(), atlas);
                }
                Ok(result)
            }
            Self::Validate(name) => {
                let v = refused(b.validate(&name))?;
                if !v.valid {
                    return Err(ComponentError::Refused);
                }
                Ok(v.to_named_payload())
            }
            Self::SchemaCreate(input) => {
                let s = refused(b.schema_create(&input))?;
                Ok(json!({"success":true,"command":"schema create","id":s.id,"name":s.name}))
            }
            Self::SchemaShow(name) => {
                let s = refused(b.schema_show(&name))?;
                Ok(
                    json!({"success":true,"command":"schema show","schema":parsed(&s.to_go_json())?}),
                )
            }
            Self::SchemaList => {
                let rows = refused(b.schema_list())?;
                let rows = rows
                    .iter()
                    .map(|s| parsed(&s.to_go_json()))
                    .collect::<Result<Vec<_>>>()?;
                Ok(
                    json!({"success":true,"command":"schema list","count":rows.len(),"schemas":rows}),
                )
            }
            Self::RuleAdd(name, rule) => {
                refused(b.schema_rule_add(&name, &rule))?;
                Ok(
                    json!({"success":true,"command":"schema rule add","schema":name,"rule":{"section":rule.section,"pattern":rule.pattern,"message":rule.message}}),
                )
            }
        }
    }
}
