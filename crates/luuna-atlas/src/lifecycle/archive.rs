//! Archive transfers with preflight of both stored collections.
use super::health::object;
use crate::{
    actions::record::{boolean, domain_names, get, has, message, text, Action},
    json::{Object, Value},
    records, store, timefmt, AtlasStore, Result,
};
use std::path::{Path, PathBuf};
pub(crate) fn path(a: &Action<'_>, domain: &str) -> Result<PathBuf> {
    AtlasStore::open(&a.ctx.root)?.archive_path(domain)
}
pub(crate) fn info(r: &Object, a: &Action<'_>) -> Object {
    object([
        ("id", r.get("id").cloned().unwrap_or(Value::Null)),
        ("type", r.get("type").cloned().unwrap_or(Value::Null)),
        (
            "summary",
            Value::from(records::record_summary(r, &a.registry)),
        ),
    ])
}
pub(crate) fn preflight_live(
    a: &Action<'_>,
    path: &Path,
    before: &[Object],
    after: &mut [Object],
    removed: &[usize],
) -> Result<()> {
    records::check_lossless_rewrite(path, &a.registry, removed)?;
    let retained = before
        .iter()
        .enumerate()
        .filter(|(i, _)| !removed.contains(i))
        .map(|(_, r)| r);
    records::check_retained_ids(path, retained.clone().zip(after.iter()))?;
    records::check_retained_numbers(path, retained.zip(after.iter()))?;
    records::assign_record_ids(after, &a.registry)?;
    Ok(())
}
pub(crate) fn preflight_archive(
    a: &Action<'_>,
    path: &Path,
    before: &[Object],
    removed: &[usize],
) -> Result<()> {
    records::check_lossless_rewrite(path, &a.registry, removed)?;
    // Archive array rows have no record fields. Walk their raw JSON value
    // through the existing iterative number guard instead of losing that seam.
    if let Ok(raw) = std::fs::read(path) {
        let mut index = 0;
        for line in raw.split(|b| *b == b'\n') {
            let line = line.trim_ascii();
            if line.is_empty() || line.starts_with(b"#") {
                continue;
            }
            let excluded = removed.contains(&index);
            index += 1;
            if excluded {
                continue;
            }
            if let Ok(value) = crate::json::parse(&String::from_utf8_lossy(line)) {
                if matches!(value, Value::Array(_)) {
                    let mut wrapper = Object::new();
                    wrapper.insert("row", value);
                    records::check_retained_numbers(path, [(&wrapper, &wrapper)])?;
                }
            }
        }
    }
    records::check_retained_numbers(
        path,
        before
            .iter()
            .enumerate()
            .filter(|(i, _)| !removed.contains(i))
            .map(|(_, r)| (r, r)),
    )?;
    Ok(())
}
pub(crate) fn stamp(rows: &[Object], reason: &str) -> Vec<Object> {
    rows.iter()
        .map(|r| {
            let mut r = r.clone();
            let mut keys = r.keys().map(str::to_owned).collect::<Vec<_>>();
            r.insert_unordered("status", "archived");
            for (key, value) in [
                ("archived_at", timefmt::now_iso()),
                ("archive_reason", reason.to_owned()),
            ] {
                if r.get(key).is_none_or(Value::is_null) {
                    r.insert_unordered(key, value);
                }
            }
            keys.extend([
                "status".into(),
                "archived_at".into(),
                "archive_reason".into(),
            ]);
            r.remember_order(keys);
            r
        })
        .collect()
}
pub(crate) fn archive(
    a: &mut Action<'_>,
    args: &[String],
    flags: &[(String, String)],
) -> Result<()> {
    let domain = &args[0];
    let cfg = a.config()?;
    if !a.domain(&cfg, domain, "archive") {
        return Ok(());
    }
    let reason = get(flags, "reason");
    let reason = reason.trim();
    if reason.is_empty() {
        return Err(message("--reason must not be empty."));
    }
    let has_id = args.len() > 1;
    if !has_id && !has(flags, "records") {
        return Err(message(if a.ctx.globals.json {
            "Must provide a record ID or --records."
        } else {
            "must provide a record ID or --records."
        }));
    }
    if has_id && has(flags, "records") {
        return Err(message(if a.ctx.globals.json {
            "Cannot combine a record ID with --records. Use only one mode."
        } else {
            "cannot combine a record ID with --records. Use only one mode."
        }));
    }
    let ids = if has_id {
        vec![args[1].clone()]
    } else {
        records::split_csv(&get(flags, "records"))
    };
    if ids.is_empty() {
        return Err(message("--records requires at least one ID."));
    }
    let reason = format!("manual: {reason}");
    let live = a.path(domain)?;
    let archive = path(a, domain)?;
    let dry = boolean(flags, "dry-run");
    let (selected, kept) = store::with_file_lock(&live, || {
        let rows = store::read_records(&live, &a.registry, a.ctx.globals.allow_unknown_types)?;
        let mut indices = Vec::new();
        for id in &ids {
            indices.push(records::resolve_record_id(&rows, id)?);
        }
        let selected = rows
            .iter()
            .enumerate()
            .filter(|(i, _)| indices.contains(i))
            .map(|(_, r)| r.clone())
            .collect::<Vec<_>>();
        let mut kept = rows
            .iter()
            .enumerate()
            .filter(|(i, _)| !indices.contains(i))
            .map(|(_, r)| r.clone())
            .collect::<Vec<_>>();
        if !dry {
            // Archive metadata is explicitly replaced by this operation; all
            // other selected data and every preexisting archive row must survive.
            records::check_lossless_rewrite(&live, &a.registry, &[])?;
            records::check_retained_numbers(&live, selected.iter().map(|r| (r, r)))?;
            let old_archive = store::read_archive(&archive, &a.registry)?;
            preflight_archive(a, &archive, &old_archive, &[])?;
            preflight_live(a, &live, &rows, &mut kept, &indices)?;
            store::write_records(&live, &mut kept, &a.registry)?;
        }
        Ok((selected, kept))
    })?;
    if !dry && !selected.is_empty() {
        std::fs::create_dir_all(archive.parent().unwrap()).map_err(|source| {
            crate::AtlasError::PathOp {
                op: "mkdir",
                path: archive.parent().unwrap().to_owned(),
                source,
            }
        })?;
        store::with_file_lock(&archive, || {
            let mut rows = store::read_archive(&archive, &a.registry)?;
            preflight_archive(a, &archive, &rows, &[])?;
            rows.extend(stamp(&selected, &reason));
            store::write_archive(&archive, &rows)
        })?;
    }
    if a.ctx.globals.json {
        let infos = selected
            .iter()
            .map(|r| {
                let mut i = info(r, a);
                i.insert_unordered("reason", reason.clone());
                Value::Object(i)
            })
            .collect();
        a.json(
            "archive",
            object([
                ("domain", domain.clone().into()),
                ("dryRun", dry.into()),
                ("archived", Value::Array(infos)),
                ("kept", (kept.len() as i64).into()),
            ]),
        );
    } else {
        let prefix = if dry {
            format!("{} ", a.styled("yellow", "[DRY RUN]"))
        } else {
            String::new()
        };
        let verb = if dry { "Would archive" } else { "Archived" };
        for r in &selected {
            let id = text(r.get("id"));
            let id = if id.is_empty() {
                String::new()
            } else {
                format!(" {}", a.styled("accent", &id))
            };
            a.human(format!(
                "{prefix}{}{id} {} ({reason}): {}",
                a.styled("brand", &format!("{verb} {}", text(r.get("type")))),
                a.styled("brand", &format!("from {domain}")),
                records::record_summary(r, &a.registry)
            ));
        }
        if !dry && selected.len() > 1 {
            a.human(a.styled(
                "brand",
                &format!("✓ Archived {} records from {domain}", selected.len()),
            ));
        }
    }
    Ok(())
}
pub(crate) fn restore(a: &mut Action<'_>, args: &[String]) -> Result<()> {
    let id = &args[0];
    let cfg = a.config()?;
    let mut matches = Vec::new();
    for domain in domain_names(&cfg) {
        let archive = path(a, &domain)?;
        let rows = store::read_archive(&archive, &a.registry)?;
        if let Ok(index) = records::resolve_record_id(&rows, id) {
            matches.push((domain, rows[index].clone()));
        }
    }
    let tick = char::from(96);
    if matches.is_empty() {
        return Err(message(format!("Archived record {} not found in any domain. Run {tick}luuna-atlas search --archived <query>{tick} to browse archives.",records::go_quote(id))));
    }
    if matches.len() > 1 {
        return Err(message(format!(
            "Identifier {} matches archived records in multiple domains: {}. Use a longer prefix.",
            records::go_quote(id),
            matches
                .iter()
                .map(|(d, r)| format!("{d} ({})", text(r.get("id"))))
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    let (domain, target) = matches.pop().unwrap();
    let rid = text(target.get("id"));
    if rid.is_empty() {
        return Err(message("Archived record has no id; cannot restore safely."));
    }
    let live_path = a.path(&domain)?;
    let archive = path(a, &domain)?;
    let existing = store::read_records(&live_path, &a.registry, a.ctx.globals.allow_unknown_types)?;
    for row in &existing {
        if text(row.get("id")) == rid {
            return Err(message(format!("Live record {} already exists in domain {} (classification: {}). Delete or rename the live record before restoring, or run {tick}luuna-atlas search --archived {rid}{tick} to inspect the archived copy.",records::go_quote(&rid),records::go_quote(&domain),text(row.get("classification")))));
        }
    }
    // Go rereads with allowUnknown=false after deleting the archive row.
    // Prove that read and the live rewrite safe before removing anything.
    let current = store::read_records(&live_path, &a.registry, false).map_err(|error| message(format!("Refusing to restore: the required live-file read would fail after removing the archive row: {error}. Files are unchanged.")))?;
    let mut stripped = target.clone();
    for key in ["status", "archived_at", "archive_reason"] {
        stripped.remove(key);
    }
    let mut retained = current.clone();
    preflight_live(a, &live_path, &current, &mut retained, &[])?;
    records::check_retained_numbers(&archive, [(&stripped, &stripped)])?;
    let removed = store::with_file_lock(&archive, || {
        let mut rows = store::read_archive(&archive, &a.registry)?;
        let Some(index) = rows.iter().position(|r| text(r.get("id")) == rid) else {
            return Ok(None);
        };
        preflight_archive(a, &archive, &rows, &[index])?;
        // The moved row is retained, not an intentional deletion.
        records::check_lossless_rewrite(&archive, &a.registry, &[])?;
        let removed = rows.remove(index);
        store::write_archive(&archive, &rows)?;
        Ok(Some(removed))
    })?
    .ok_or_else(|| {
        message(format!(
            "Archived record {} disappeared between read and write (concurrent restore?).",
            records::go_quote(&rid)
        ))
    })?;
    let reason = text(removed.get("archive_reason"));
    let reason = if reason.is_empty() {
        "manual".into()
    } else {
        reason
    };
    let mut restored = removed.clone();
    for key in ["status", "archived_at", "archive_reason"] {
        restored.remove(key);
    }
    let conflict = store::with_file_lock(&live_path, || {
        let mut rows = store::read_records(&live_path, &a.registry, false)?;
        if let Some(r) = rows.iter().find(|r| text(r.get("id")) == rid) {
            return Ok(Some(r.clone()));
        }
        let before = rows.clone();
        preflight_live(a, &live_path, &before, &mut rows, &[])?;
        rows.push(restored.clone());
        records::assign_record_ids(&mut rows, &a.registry)?;
        store::write_records(&live_path, &mut rows, &a.registry)?;
        Ok(None)
    })?;
    if let Some(conflict) = conflict {
        store::with_file_lock(&archive, || {
            let mut rows = store::read_archive(&archive, &a.registry)?;
            preflight_archive(a, &archive, &rows, &[])?;
            rows.extend(stamp(&[restored.clone()], &reason));
            store::write_archive(&archive, &rows)
        })?;
        return Err(message(format!("Live record {} appeared in domain {} while restore was in flight (classification: {}). Re-archived; nothing changed.",records::go_quote(&rid),records::go_quote(&domain),text(conflict.get("classification")))));
    }
    if a.ctx.globals.json {
        let mut data = info(&restored, a);
        data.insert_unordered("domain", domain);
        data.insert_unordered("archive_reason", reason);
        a.json("restore", data);
    } else {
        a.human(format!(
            "{} {} {} {} (archived: {reason}): {}",
            a.styled("brand", "✓"),
            a.styled("brand", &format!("Restored {}", text(restored.get("type")))),
            a.styled("accent", &rid),
            a.styled("brand", &format!("to {domain}")),
            records::record_summary(&restored, &a.registry)
        ));
    }
    Ok(())
}
