//! Whole-domain deletion and explicit confirmation.
use super::{health::object, Options};
use crate::{
    actions::record::{boolean, message, Action},
    config,
    json::Value,
    records, store, AtlasError, Result,
};
pub(crate) fn confirm(a: &mut Action<'_>, prompt: &str, input: &[u8]) -> bool {
    a.output.stdout.push_str(&format!("{prompt} (y/N): "));
    let line = input.split(|b| *b == b'\n').next().unwrap_or_default();
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    line.eq_ignore_ascii_case(b"y") || line.eq_ignore_ascii_case(b"yes")
}
pub(crate) fn run(a: &mut Action<'_>, args: &[String], options: &Options) -> Result<()> {
    let domain = &args[0];
    let mut cfg = a.config()?;
    if !records::obj(cfg.get("domains")).contains_key(domain) {
        if a.ctx.globals.json {
            a.domain(&cfg, domain, "delete-domain");
        } else {
            let tick = char::from(96);
            a.output.exit_code = 1;
            a.output.stderr.push_str(&format!("{}\n{}\n",a.styled("red",&format!("Error: domain {} not found in config.",records::go_quote(domain))),a.styled("red",&format!("Hint: Run {tick}luuna-atlas add {domain}{tick} to create it, or check {tick}luuna-atlas status{tick} for existing domains."))));
        }
        return Ok(());
    }
    let path = a.path(domain)?;
    let rows = store::read_records(&path, &a.registry, a.ctx.globals.allow_unknown_types)?;
    let plural = if rows.len() == 1 { "" } else { "s" };
    if boolean(&options.flags, "dry-run") {
        if a.ctx.globals.json {
            a.json(
                "delete-domain",
                object([
                    ("domain", domain.clone().into()),
                    ("dryRun", true.into()),
                    ("recordCount", (rows.len() as i64).into()),
                ]),
            );
        } else {
            a.human(format!(
                "{} Would delete domain {} ({} record{plural}) and its expertise file.",
                a.styled("yellow", "[DRY RUN]"),
                a.styled("accent", domain),
                rows.len()
            ));
        }
        return Ok(());
    }
    if !a.ctx.globals.json
        && !boolean(&options.flags, "yes")
        && !confirm(
            a,
            &format!(
                "This will delete domain {} ({} record{plural}) and its expertise file. Continue?",
                records::go_quote(domain),
                rows.len()
            ),
            &options.input,
        )
    {
        a.output
            .stdout
            .push_str(&format!("{}\n", a.styled("yellow", "Cancelled.")));
        return Ok(());
    }
    store::with_file_lock(&path, || {
        config::validate_domain_name(domain)?;
        if let Some(domains) = cfg.get_mut("domains").and_then(Value::as_object_mut) {
            domains.remove(domain);
        } else {
            return Err(message(format!(
                "Domain {} not found in config.",
                records::go_quote(domain)
            )));
        }
        let atlas = a.ctx.root.join(".luuna/atlas");
        config::check_lossless_rewrite(&atlas, &cfg, &[vec!["domains".into(), domain.clone()]])?;
        config::write_config(&atlas, &cfg)?;
        if path.exists() {
            std::fs::remove_file(&path).map_err(|source| AtlasError::PathOp {
                op: "remove",
                path: path.clone(),
                source,
            })?;
        }
        Ok(())
    })?;
    if a.ctx.globals.json {
        a.json(
            "delete-domain",
            object([
                ("domain", domain.clone().into()),
                ("deletedFile", true.into()),
                ("recordCount", (rows.len() as i64).into()),
            ]),
        );
    } else {
        a.human(format!(
            "{} {} {} and deleted expertise file.",
            a.styled("brand", "✓"),
            a.styled("brand", "Removed domain"),
            a.styled("accent", domain)
        ));
    }
    Ok(())
}
