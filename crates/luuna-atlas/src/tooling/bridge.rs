//! Private IPC wrapper over native typed Atlas APIs.
use super::transport;
use crate::{
    actions::{
        api,
        record::{message, text},
    },
    config, git,
    json::{self, Object, Value},
    options::Context,
    records,
    registry::Registry,
    store, AtlasStore, CommandOutput, Result,
};
use std::{
    fs,
    path::{Path, PathBuf},
};
fn object(fields: impl IntoIterator<Item = (&'static str, Value)>) -> Object {
    let mut o = Object::new();
    for (k, v) in fields {
        o.insert_unordered(k, v);
    }
    o
}
fn rows(v: Option<&Value>) -> Vec<Object> {
    records::any_list(v)
        .iter()
        .filter_map(|v| match v {
            Value::Object(o) => Some(o.clone()),
            Value::Array(a) => Some(Object::from_record_array(a.clone())),
            _ => None,
        })
        .collect()
}
fn values(rows: Vec<Object>) -> Value {
    Value::Array(rows.into_iter().map(Value::Object).collect())
}
fn resolve(ctx: &Context, cwd: &str) -> PathBuf {
    let cwd = if cwd.is_empty() {
        ctx.cwd.clone()
    } else if Path::new(cwd).is_absolute() {
        PathBuf::from(cwd)
    } else {
        ctx.cwd.join(cwd)
    };
    git::resolve_store_root(&crate::commands::clean_path(&cwd), &ctx.env)
}
fn path(ctx: &Context, name: &str) -> PathBuf {
    let path = Path::new(name);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        ctx.cwd.join(path)
    }
}
fn defaults(request: &Object) -> Object {
    if let Some(Value::Object(o)) = request.get("defaults") {
        return o.clone();
    }
    let mut gov = Object::new();
    for (k, n) in [
        ("max_entries", 100),
        ("warn_entries", 150),
        ("hard_limit", 200),
    ] {
        gov.insert(k, n);
    }
    gov.remember_order(["max_entries", "warn_entries", "hard_limit"]);
    let mut shelf = Object::new();
    shelf.insert("tactical", 14);
    shelf.insert("observational", 30);
    shelf.remember_order(["tactical", "observational"]);
    let mut cd = Object::new();
    cd.insert("shelf_life", shelf);
    let mut out = Object::new();
    out.insert("version", "1");
    out.insert("domains", Object::new());
    out.insert("governance", gov);
    out.insert("classification_defaults", cd);
    out.remember_order([
        "version",
        "domains",
        "governance",
        "classification_defaults",
    ]);
    out
}
fn merge(mut base: Object, user: &Object) -> Object {
    for (k, v) in user.iter() {
        base.insert_go_key(user.key_bytes(k), v.clone());
    }
    base
}
fn read_config(root: &Path, defaults: &Object) -> Result<Object> {
    let mut cfg = config::read_config(&AtlasStore::open(root)?.atlas_dir())?;
    let bytes =
        fs::read(root.join(".luuna/atlas/config.yaml")).map_err(|e| message(e.to_string()))?;
    let raw = crate::yaml::parse(&String::from_utf8_lossy(&bytes))?;
    let raw = records::obj(Some(&raw));
    let gov = merge(
        records::obj(defaults.get("governance")).clone(),
        records::obj(raw.get("governance")),
    );
    let user_cd = records::obj(raw.get("classification_defaults"));
    let default_cd = records::obj(defaults.get("classification_defaults"));
    let shelf = merge(
        records::obj(default_cd.get("shelf_life")).clone(),
        records::obj(user_cd.get("shelf_life")),
    );
    let mut cd = merge(default_cd.clone(), user_cd);
    cd.insert("shelf_life", shelf);
    cfg.insert("governance", gov);
    cfg.insert("classification_defaults", cd);
    Ok(cfg)
}
fn mutation(result: Result<Value>, mutated: Value, empty: Value) -> Value {
    let mut o = object([("result", empty), ("mutation", mutated)]);
    match result {
        Ok(v) => o.insert_unordered("result", v),
        Err(e) => o.insert_unordered("error", e.to_string()),
    };
    o.into()
}
fn indexes(original: &[Object], selected: &[Object]) -> Value {
    let key = |r: &Object| {
        let value = Value::Object(r.clone());
        (
            json::to_compact_string(&value).unwrap_or_default(),
            json::to_compact_string(&transport::collect(&value)).unwrap_or_default(),
        )
    };
    let mut positions = std::collections::BTreeMap::<_, std::collections::VecDeque<usize>>::new();
    for (n, r) in original.iter().enumerate() {
        positions.entry(key(r)).or_default().push_back(n);
    }
    Value::Array(
        selected
            .iter()
            .filter_map(|r| positions.get_mut(&key(r)).and_then(|v| v.pop_front()))
            .map(|n| Value::Integer(n as i128))
            .collect(),
    )
}
/// Execute one restored request. Registry defaults are local, never process-global.
pub fn execute(ctx: &Context, request: Value) -> Result<Value> {
    let request = transport::restore(request);
    let req = records::obj(Some(&request));
    let operation = text(req.get("operation"));
    let args = records::any_list(req.get("args"));
    let arg = |n| args.get(n);
    let opts = |n| records::obj(arg(n));
    let record = |n| records::obj(arg(n)).clone();
    let root = |n| resolve(ctx, &text(opts(n).get("cwd")));
    let registry = Registry::from_config(None)?;
    match operation.as_str() {
        "configSchema" | "recordSchema" => Ok(crate::lifecycle::contract::contract()
            .as_object()
            .unwrap()
            .get(&operation)
            .unwrap()
            .clone()),
        "defaultConfig" => Ok(defaults(req).into()),
        "readConfig" => Ok(read_config(&resolve(ctx, &text(arg(0))), &defaults(req))?.into()),
        "getExpertisePath" => {
            let requested = text(arg(1));
            let root = resolve(ctx, &requested);
            let actual = AtlasStore::open(&root)?.expertise_path(&text(arg(0)))?;
            let display = if !requested.is_empty()
                && !Path::new(&requested).is_absolute()
                && root == crate::commands::clean_path(&ctx.cwd.join(&requested))
            {
                crate::commands::clean_path(
                    &Path::new(&requested).join(actual.strip_prefix(&root).unwrap()),
                )
            } else {
                actual
            };
            Ok(display.to_string_lossy().into_owned().into())
        }
        "recordExpertise" | "recordExpertiseTransport" => {
            let mut r = record(1);
            let result = api::record_expertise_mut(
                &root(2),
                &text(arg(0)),
                &mut r,
                &api::RecordOptions {
                    force: crate::registry::go_bool(opts(2).get("force")),
                },
                None,
            )
            .map(|r| r.to_value());
            if operation.ends_with("Transport") {
                Ok(mutation(
                    result,
                    r.into(),
                    object([("action", "".into()), ("record", Value::Null)]).into(),
                ))
            } else {
                result
            }
        }
        "searchExpertise" => Ok(Value::Array(
            api::search_expertise(
                &root(1),
                &text(arg(0)),
                &api::SearchOptions {
                    domain: text(opts(1).get("domain")),
                    type_name: text(opts(1).get("type")),
                    tag: text(opts(1).get("tag")),
                    classification: text(opts(1).get("classification")),
                    file: text(opts(1).get("file")),
                },
                None,
            )?
            .iter()
            .map(|r| r.to_value())
            .collect(),
        )),
        "queryDomain" => Ok(values(api::query_domain(
            &root(1),
            &text(arg(0)),
            &api::QueryOptions {
                type_name: text(opts(1).get("type")),
                classification: text(opts(1).get("classification")),
                file: text(opts(1).get("file")),
            },
            None,
        )?)),
        "editRecord" => {
            Ok(api::edit_record(&root(3), &text(arg(0)), &text(arg(1)), &record(2), None)?.into())
        }
        "appendOutcome" => {
            Ok(
                api::append_outcome(&root(3), &text(arg(0)), &text(arg(1)), &record(2), None)?
                    .to_value(),
            )
        }
        "appendRecord" | "appendRecordTransport" => {
            let mut r = record(1);
            let requested = text(arg(0));
            let actual = path(ctx, &requested);
            let result = store::append_record(&actual, &mut r, &registry)
                .map(|_| r.clone().into())
                .map_err(|e| {
                    message(if Path::new(&requested).is_relative() {
                        e.to_string()
                            .replace(&actual.to_string_lossy().to_string(), &requested)
                    } else {
                        e.to_string()
                    })
                });
            if operation.ends_with("Transport") {
                Ok(mutation(result.map(|_| Value::Null), r.into(), Value::Null))
            } else {
                result
            }
        }
        "readExpertiseFile" => Ok(values(store::read_records(
            &path(ctx, &text(arg(0))),
            &registry,
            crate::registry::go_bool(opts(1).get("allowUnknownTypes")),
        )?)),
        "writeExpertiseFile" | "writeExpertiseFileTransport" => {
            let path = path(ctx, &text(arg(0)));
            let mut r = rows(arg(1));
            let result = (|| {
                records::check_retained_ids(&path, r.iter().map(|r| (r, r)))?;
                records::check_retained_numbers(&path, r.iter().map(|r| (r, r)))?;
                store::write_records(&path, &mut r, &registry)?;
                Ok(values(r.clone()))
            })();
            if operation.ends_with("Transport") {
                Ok(mutation(
                    result.map(|_| Value::Null),
                    values(r),
                    Value::Null,
                ))
            } else {
                result
            }
        }
        "findDuplicate" => {
            let r = rows(arg(0));
            Ok(records::find_duplicate(&r, &record(1), &registry)
                .map(|n| {
                    Value::Object(object([
                        ("index", Value::Integer(n as i128)),
                        ("record", r[n].clone().into()),
                    ]))
                })
                .unwrap_or(Value::Null))
        }
        "generateRecordId" => Ok(records::generate_record_id(&record(0), &registry)?.into()),
        "searchRecords" | "searchRecordIndexes" => {
            let r = rows(arg(0));
            let selected = records::search_records(
                &r,
                &text(arg(1)),
                records::number(opts(2).get("boostFactor")),
            );
            Ok(if operation.ends_with("Indexes") {
                indexes(&r, &selected)
            } else {
                values(selected)
            })
        }
        "computeConfirmationScore" => Ok(records::confirmation_score(&record(0)).into()),
        "getSuccessCount" => Ok((records::success_count(&record(0)) as i64).into()),
        "getFailureCount" => Ok((records::failure_count(&record(0)) as i64).into()),
        "getTotalApplications" => Ok((records::total_applications(&record(0)) as i64).into()),
        "getSuccessRate" => Ok(records::success_rate(&record(0)).into()),
        "applyConfirmationBoost" => Ok(records::apply_confirmation_boost(
            records::number(arg(0)),
            &record(1),
            if arg(2).is_none() || matches!(arg(2), Some(Value::Null)) {
                0.1
            } else {
                records::number(arg(2))
            },
        )
        .into()),
        "sortByConfirmationScore" | "sortByConfirmationScoreIndexes" => {
            let r = rows(arg(0));
            let sorted = records::sort_by_confirmation_score(&r);
            Ok(if operation.ends_with("Indexes") {
                indexes(&r, &sorted)
            } else {
                values(sorted)
            })
        }
        _ => Err(message(format!(
            "Unknown Atlas library operation: {operation}"
        ))),
    }
}
fn migration(ctx: &Context, request: &Value) -> Result<Option<String>> {
    let req = records::obj(Some(request));
    let args = records::any_list(req.get("args"));
    let op = text(req.get("operation"));
    let cwd = match op.as_str() {
        "readConfig" => text(args.first()),
        "getExpertisePath" => {
            config::validate_domain_name(&text(args.first()))?;
            text(args.get(1))
        }
        "recordExpertise" | "recordExpertiseTransport" => {
            text(records::obj(args.get(2)).get("cwd"))
        }
        "searchExpertise" | "queryDomain" => text(records::obj(args.get(1)).get("cwd")),
        "editRecord" | "appendOutcome" => text(records::obj(args.get(3)).get("cwd")),
        _ => return Ok(None),
    };
    crate::legacy::migrate_legacy_store(&resolve(ctx, &cwd))
}
/// Bridge mode precedes all CLI prelude and writes both successes and failures to stdout.
pub fn run(ctx: &Context, input: &[u8]) -> CommandOutput {
    let mut notice = None;
    let result = super::json_input::parse(input).and_then(|r| {
        let r = transport::restore(r);
        match migration(ctx, &r) {
            Ok(n) => {
                notice = n;
                execute(ctx, r)
            }
            Err(e) => {
                let req = records::obj(Some(&r));
                if text(req.get("operation")) == "recordExpertiseTransport" {
                    let args = records::any_list(req.get("args"));
                    Ok(mutation(
                        Err(e),
                        records::obj(args.get(1)).clone().into(),
                        object([("action", "".into()), ("record", Value::Null)]).into(),
                    ))
                } else {
                    Err(e)
                }
            }
        }
    });
    let mut out = match result {
        Ok(result) => {
            let numbers = transport::collect(&result);
            CommandOutput::emit(&object([
                ("success", true.into()),
                ("result", result),
                ("numbers", numbers),
            ]))
        }
        Err(e) => {
            let mut out = CommandOutput::emit(&object([
                ("success", false.into()),
                ("error", e.to_string().into()),
            ]));
            out.exit_code = 1;
            out
        }
    };
    if let Some(notice) = notice {
        out.stderr = format!("{notice}\n{}", out.stderr);
    }
    out
}
