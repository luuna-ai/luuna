use super::parser::{OptionSpec, Spec};
use crate::{options::Context, records, registry::Registry, style};
pub(super) fn customize(spec: &mut Spec, registry: &Registry) {
    let old = spec.options.clone();
    let names: Vec<String> = registry.names().iter().map(|s| (*s).to_owned()).collect();
    for o in &mut spec.options {
        if o.name == "type" {
            o.choices = names.clone();
        }
    }
    if spec.name == "record" || spec.name == "edit" {
        for name in registry.names() {
            let def = registry.get(name).unwrap();
            if def.builtin {
                continue;
            }
            for field in def.required.iter().chain(&def.optional) {
                let flag = field.replace('_', "-");
                if !spec.options.iter().any(|o| o.name == flag) {
                    spec.options.push(OptionSpec {
                        name: flag.clone(),
                        long: format!("--{flag}"),
                        flags: format!("--{flag} <{field}>"),
                        required: true,
                        description: format!(
                            "{}{name} field: {field}",
                            if spec.name == "edit" { "update " } else { "" }
                        ),
                        ..OptionSpec::default()
                    });
                }
            }
        }
    }
    for o in &spec.options {
        if o.name == "type" && !o.choices.is_empty() {
            for prev in &old {
                if prev.name == "type" && !prev.choices.is_empty() {
                    let list = |a: &[String]| {
                        a.iter()
                            .map(|v| records::go_quote(v))
                            .collect::<Vec<_>>()
                            .join(", ")
                    };
                    spec.help = spec.help.replace(
                        &format!("choices: {}", list(&prev.choices)),
                        &format!("choices: {}", list(&o.choices)),
                    );
                }
            }
        }
    }
    let added: Vec<String> = spec
        .options
        .iter()
        .filter(|o| !old.iter().any(|p| p.name == o.name))
        .map(|o| {
            format!(
                "  {}{}{}",
                o.flags,
                " ".repeat(20usize.saturating_sub(o.flags.len()).max(2)),
                o.description
            )
        })
        .collect();
    if !added.is_empty() {
        spec.help = spec.help.replacen(
            "  -h, --help",
            &format!("{}\n  -h, --help", added.join("\n")),
            1,
        );
    }
}
pub(super) fn render(ctx: &Context, spec: &Spec) -> String {
    let force = ctx.env.var("FORCE_COLOR").unwrap_or_default();
    if ctx.env.var("NO_COLOR").is_some_and(|s| !s.is_empty())
        || force == "0"
        || force == "false"
        || (force.is_empty()
            && ctx.env.var("CLICOLOR_FORCE").is_none()
            && !ctx.env.stdout_is_terminal())
    {
        return spec.help.clone();
    }
    let level = style::color_level(&ctx.env);
    let st = |name: &str, text: &str| style::style(level, name, text);
    let mut section = "";
    spec.help
        .split('\n')
        .enumerate()
        .map(|(n, line)| {
            if n == 0 {
                return format!(
                    "{} {} — Structured expertise management",
                    st(
                        "brand",
                        &st(
                            "bold",
                            if spec.name == "luuna-atlas" {
                                "Luuna Atlas"
                            } else {
                                &spec.name
                            }
                        )
                    ),
                    st("muted", "v0.11.0")
                );
            }
            if line.starts_with("Usage: ") {
                return format!("Usage: {} <command> [options]", st("dim", &spec.name));
            }
            if line == "Commands:" {
                section = "commands"
            }
            if line == "Options:" {
                section = "options"
            }
            if line.starts_with("Run '") {
                return format!(
                    "Run '{} <command> --help' for command-specific help.",
                    st("dim", &spec.name)
                );
            }
            if line.starts_with("  ") && section == "commands" {
                let extra = Spec {
                    name: "help".into(),
                    description: "display help for command".into(),
                    ..Spec::default()
                };
                for child in spec.commands.iter().chain(std::iter::once(&extra)) {
                    let prefix = format!("  {}", child.name);
                    if !child.hidden && line.starts_with(&(prefix.clone() + " ")) {
                        let rest = &line[prefix.len()..];
                        if let Some(idx) = rest.rfind(&child.description) {
                            let term = &rest[..idx];
                            let args = term.trim_end_matches(' ');
                            return format!(
                                "  {}{}{}{}",
                                st("green", &child.name),
                                st("dim", args),
                                &term[args.len()..],
                                child.description
                            );
                        }
                        break;
                    }
                }
            }
            if line.starts_with("  ") && section == "options" {
                let extra = OptionSpec {
                    flags: "-h, --help".into(),
                    ..OptionSpec::default()
                };
                for o in spec.options.iter().chain(std::iter::once(&extra)) {
                    let prefix = format!("  {}", o.flags);
                    if let Some(rest) = line.strip_prefix(&prefix) {
                        return format!("  {}{rest}", st("dim", &o.flags));
                    }
                }
            }
            line.to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}
