//! Native Atlas tooling and compatibility CLI front end.
pub mod audit;
pub mod bridge;
mod doctor;
mod doctor_fix;
mod help;
mod json_input;
mod json_rewrite;
pub mod onboard;
mod parser;
mod recipes;
mod setup;
mod setup_claude;
mod text_bytes;
pub mod transport;
use crate::{
    actions::record,
    git,
    json::Object,
    options::{Context, GlobalOptions},
    registry::Registry,
    CommandOutput,
};
use parser::{Parsed, Spec};
fn empty() -> CommandOutput {
    CommandOutput {
        exit_code: 0,
        stdout: String::new(),
        stderr: String::new(),
    }
}
fn stdout(text: String) -> CommandOutput {
    CommandOutput {
        stdout: text,
        ..empty()
    }
}
fn usage(ctx: &Context, spec: &Spec, err: bool) -> CommandOutput {
    let text = help::render(ctx, spec);
    if err {
        CommandOutput {
            exit_code: 1,
            stderr: text,
            ..empty()
        }
    } else {
        stdout(text)
    }
}
fn parser_error(text: &str) -> CommandOutput {
    CommandOutput::plain_failure(&format!("error: {text}"))
}
/// Execute an Atlas invocation with explicit cwd/environment/stdin. Store resolution occurs
/// before dispatch, while child commands retain the caller's actual working directory.
pub fn run(context: &Context, raw: &[String], input: &[u8]) -> CommandOutput {
    if context.env.var("LUUNA_ATLAS_API_BRIDGE").as_deref() == Some("1") {
        return bridge::run(context, input);
    }
    let mut ctx = context.clone();
    ctx.root = git::resolve_store_root(&ctx.cwd, &ctx.env);
    ctx.globals = GlobalOptions::prescan(raw);
    record::run(&ctx, "init", |a| {
        a.output = front(a.ctx, raw, input, &a.registry);
        Ok(())
    })
}
fn front(context: &Context, raw: &[String], input: &[u8], registry: &Registry) -> CommandOutput {
    let mut ctx = context.clone();
    let mut args = raw.to_vec();
    let root = Spec::read(
        crate::lifecycle::contract::contract()
            .as_object()
            .unwrap()
            .get("cli")
            .unwrap(),
    );
    if ctx.globals.json && args.iter().any(|s| s == "--version" || s == "-v") {
        let mut o = Object::new();
        o.insert("name", "@luuna/atlas-cli");
        o.insert("version", "0.11.0");
        o.insert("runtime", "bun");
        let os = if cfg!(target_os = "macos") {
            "darwin"
        } else if cfg!(windows) {
            "win32"
        } else {
            std::env::consts::OS
        };
        let arch = match std::env::consts::ARCH {
            "x86_64" => "x64",
            "aarch64" => "arm64",
            a => a,
        };
        o.insert("platform", format!("{os}-{arch}"));
        o.remember_order(["name", "version", "runtime", "platform"]);
        return CommandOutput::emit(&o);
    }
    let mut index = 0;
    while index < args.len() && args[index].starts_with('-') {
        let token = &args[index];
        if token == "--" {
            index += 1;
            continue;
        }
        if let Some(exp) = parser::expand(&root.options, token) {
            args.splice(index..=index, exp);
            continue;
        }
        if token == "--help" || token == "-h" {
            return usage(&ctx, &root, false);
        }
        if token == "--version" || token == "-v" {
            return stdout("0.11.0\n".into());
        }
        let Some((o, inline)) = parser::find(&root.options, token) else {
            return match parser::parse(&root, &root.options, &args[index..], &mut ctx.globals) {
                Ok(Parsed::Help) => usage(&ctx, &root, false),
                Ok(Parsed::Version) => stdout("0.11.0\n".into()),
                Err(e) => parser_error(&e),
                Ok(Parsed::Invocation(_)) => parser_error("unknown option"),
            };
        };
        match parser::consume(o, &args, index, inline) {
            Ok((v, next)) => {
                ctx.globals.set(&o.name, &v);
                index = next
            }
            Err(e) => return parser_error(&e),
        }
    }
    if index >= args.len() {
        return usage(&ctx, &root, true);
    }
    let name = args[index].clone();
    index += 1;
    let Some(mut spec) = root.commands.iter().find(|s| s.name == name).cloned() else {
        let suggestion = parser::suggest(&name, &root.commands);
        let mut message = format!("Unknown command: {name}");
        if ctx.globals.json {
            if !suggestion.is_empty() {
                message += &format!(". Did you mean '{suggestion}'?")
            }
            return CommandOutput::failure(&name, &message);
        }
        if !suggestion.is_empty() {
            message += &format!("\nDid you mean '{suggestion}'?")
        }
        message += "\nRun 'luuna-atlas --help' for usage.";
        return CommandOutput::plain_failure(&message);
    };
    if !spec.commands.is_empty() {
        while index < args.len() && args[index].starts_with('-') {
            if args[index] == "--help" || args[index] == "-h" {
                return usage(&ctx, &spec, false);
            }
            let Some((o, inline)) = parser::find(&root.options, &args[index]) else {
                break;
            };
            if o.name == "version" {
                return stdout("0.11.0\n".into());
            }
            match parser::consume(o, &args, index, inline) {
                Ok((v, next)) => {
                    ctx.globals.set(&o.name, &v);
                    index = next
                }
                Err(e) => return parser_error(&e),
            }
        }
        if index >= args.len() {
            return usage(&ctx, &spec, true);
        }
        let child = args[index].clone();
        index += 1;
        if child == "help" {
            if index == args.len() {
                return usage(&ctx, &spec, false);
            }
            return spec
                .commands
                .iter()
                .find(|s| s.name == args[index])
                .map_or_else(|| usage(&ctx, &spec, true), |s| usage(&ctx, s, false));
        }
        let Some(child_spec) = spec.commands.iter().find(|s| s.name == child).cloned() else {
            if args[index..].iter().any(|s| s == "--help" || s == "-h") {
                return usage(&ctx, &spec, false);
            }
            return parser_error(&format!("unknown command '{child}'"));
        };
        spec = child_spec;
    }
    if ["record", "edit", "search", "rank"].contains(&name.as_str()) {
        help::customize(&mut spec, registry)
    }
    let i = match parser::parse(&spec, &root.options, &args[index..], &mut ctx.globals) {
        Ok(Parsed::Help) => return usage(&ctx, &spec, false),
        Ok(Parsed::Version) => return stdout("0.11.0\n".into()),
        Err(e) => return parser_error(&e),
        Ok(Parsed::Invocation(i)) => i,
    };
    let mut positionals = i.positionals.clone();
    if name == "config" {
        positionals.insert(0, spec.name.clone())
    }
    let start = std::time::Instant::now();
    let mut out = dispatch(&ctx, &name, &positionals, &i.flat(), input);
    if raw.iter().any(|s| s == "--timing") {
        out.stderr += &format!("Done in {}ms\n", start.elapsed().as_millis());
    }
    out
}
fn dispatch(
    ctx: &Context,
    name: &str,
    args: &[String],
    flags: &[(String, String)],
    input: &[u8],
) -> CommandOutput {
    let get = |k: &str| record::get(flags, k);
    let opt = |k: &str| record::has(flags, k).then(|| get(k));
    match name {
        "record" => {
            if record::has(flags, "batch") || record::boolean(flags, "stdin") {
                crate::actions::bulk::run_record_bulk(
                    ctx,
                    &args[0],
                    if record::has(flags, "batch") {
                        crate::actions::bulk::BulkInput::Batch(std::path::PathBuf::from(get(
                            "batch",
                        )))
                    } else {
                        crate::actions::bulk::BulkInput::Stdin(input.to_vec())
                    },
                    &crate::actions::record::RecordOptions {
                        content: args.get(1).cloned(),
                        flags: flags.to_vec(),
                    },
                )
            } else {
                crate::actions::record::run_record(
                    ctx,
                    &args[0],
                    &crate::actions::record::RecordOptions {
                        content: args.get(1).cloned(),
                        flags: flags.to_vec(),
                    },
                )
            }
        }
        "edit" => crate::actions::edit::run_edit(
            ctx,
            &args[0],
            &args[1],
            &crate::actions::edit::EditOptions {
                flags: flags.to_vec(),
            },
        ),
        "delete" => crate::actions::delete::run_delete(
            ctx,
            &args[0],
            args.get(1).map(String::as_str),
            &crate::actions::delete::DeleteOptions {
                records: opt("records"),
                all_except: opt("all-except"),
                dry_run: record::boolean(flags, "dry-run"),
            },
        ),
        "search" => crate::actions::search::run_search(
            ctx,
            args.first().map(String::as_str),
            &crate::actions::search::SearchOptions {
                flags: flags.to_vec(),
            },
        ),
        "show" => crate::actions::show::run_show(ctx, args),
        "prime" | "hook" | "learn" | "diff" | "sync" => crate::prime::run(
            ctx,
            name,
            args,
            &crate::prime::Options {
                flags: flags.to_vec(),
                input: input.to_vec(),
            },
        ),
        "query" | "rank" | "ready" | "status" | "validate" | "outcome" | "archive" | "restore"
        | "delete-domain" | "move" | "stale" | "prune" | "compact" | "config" => {
            crate::lifecycle::run(
                ctx,
                name,
                args,
                &crate::lifecycle::Options {
                    flags: flags.to_vec(),
                    input: input.to_vec(),
                },
            )
        }
        "doctor" => record::run(ctx, name, |a| doctor::run(a, flags)),
        "audit" => record::run(ctx, name, |a| audit::run(a, flags)),
        "setup" => record::run(ctx, name, |a| setup::run(a, args, flags)),
        "onboard" => record::run(ctx, name, |a| onboard::run(a, flags)),
        "completions" => record::run(ctx, name, |a| {
            let shell = args[0].to_lowercase();
            let text = completion_script(&shell);
            if text.is_empty() {
                a.output.exit_code = 1;
                a.output.stderr += &format!(
                    "{} {}\n",
                    a.styled("red", "✗"),
                    a.styled(
                        "red",
                        &format!(
                            "Unsupported shell: \"{}\". Supported: bash, zsh, fish",
                            args[0]
                        )
                    )
                );
            } else {
                a.output.stdout += text;
            }
            Ok(())
        }),
        "init" => record::run(ctx, name, |a| {
            let attributes = ctx.root.join(".gitattributes");
            if attributes.is_file() && std::fs::read(&attributes).is_err() {
                return Err(record::message("Refusing to initialize: existing .gitattributes cannot be read and its rules could be lost. No initialization files were written."));
            }
            let r = crate::commands::init(&ctx.root)?;
            if ctx.globals.json {
                let mut o = Object::new();
                o.insert("created", r.created);
                o.insert("path", r.path.to_string_lossy().into_owned());
                a.json(name, o)
            } else {
                a.human(a.styled(
                    "brand",
                    &if r.created {
                        format!("Initialized .luuna/atlas/ in {}", ctx.root.display())
                    } else {
                        "Updated .luuna/atlas/ — filled in any missing artifacts.".into()
                    },
                ));
            }
            Ok(())
        }),
        "add" => record::run(ctx, name, |a| {
            crate::commands::add(&ctx.root, &args[0])?;
            if ctx.globals.json {
                let mut o = Object::new();
                o.insert("domain", args[0].clone());
                a.json(name, o)
            } else {
                a.human(a.styled(
                    "green",
                    &format!("Added domain {}.", crate::records::go_quote(&args[0])),
                ));
            }
            Ok(())
        }),
        "upgrade" => record::run(ctx, name, |a| {
            let text = "Luuna Atlas is not published; update through a reviewed Luuna checkout.";
            if ctx.globals.json {
                let mut o = Object::new();
                o.insert("success", false);
                o.insert("command", name);
                o.insert("error", text);
                o.insert("updated", false);
                a.output.stdout += &CommandOutput::emit(&o).stdout;
                a.output.exit_code = 1;
            } else {
                a.error(name, text)
            }
            Ok(())
        }),
        "update" => record::run(ctx, name, |a| {
            let text = "'luuna-atlas update' is deprecated. Use 'luuna-atlas upgrade' instead.";
            a.output.exit_code = 1;
            if ctx.globals.json {
                a.error(name, text)
            } else {
                a.output.stdout += &format!("⚠ {text}\n");
            }
            Ok(())
        }),
        _ => record::run(ctx, name, |_| {
            Err(record::message(format!("Unknown command: {name}")))
        }),
    }
}

/// Exact pinned shell completion templates. CLI aliases are integrated by the wire plan.
pub fn completion_script(shell: &str) -> &'static str {
    match shell {
        "bash" => include_str!("completions/bash"),
        "zsh" => include_str!("completions/zsh"),
        "fish" => include_str!("completions/fish"),
        _ => "",
    }
}
