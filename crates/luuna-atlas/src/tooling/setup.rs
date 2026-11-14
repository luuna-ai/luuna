use super::{onboard, setup_claude};
use crate::{
    actions::record::{boolean, message, Action},
    json::{Object, Value},
    Result,
};
use std::{
    fs,
    path::{Path, PathBuf},
};
struct Recipe {
    ok: bool,
    message: String,
    writes: Vec<(PathBuf, Option<String>, bool)>,
}
impl Recipe {
    fn result(ok: bool, text: impl Into<String>) -> Self {
        Self {
            ok,
            message: text.into(),
            writes: Vec::new(),
        }
    }
    fn write(mut self, path: PathBuf, text: Option<String>, chmod: bool) -> Self {
        self.writes.push((path, text, chmod));
        self
    }
}
const CODEX_START: &str = "# luuna-atlas:start — managed by `luuna-atlas setup codex`";
const CODEX_END: &str = "# luuna-atlas:end";
fn codex_block() -> String {
    format!("{CODEX_START}\n[features]\ncodex_hooks = true\n\n[[hooks.SessionStart]]\n\n[[hooks.SessionStart.hooks]]\ntype = \"command\"\ncommand = \"luuna-atlas prime\"\nstatusMessage = \"Loading luuna-atlas expertise\"\n{CODEX_END}")
}
fn trim(c: &str) -> &str {
    c.trim_end_matches([' ', '\r', '\n', '\t'])
}
fn append(c: &str, section: &str) -> String {
    if c.is_empty() {
        format!("{section}\n")
    } else {
        format!("{}\n\n{section}\n", trim(c))
    }
}
fn remove_codex(c: &str) -> String {
    let Some(start) = c.find(CODEX_START) else {
        return c.into();
    };
    let Some(end) = c[start..].find(CODEX_END) else {
        return c.into();
    };
    let end = start + end;
    let cut = c[end..].find('\n').map_or(c.len(), |n| end + n + 1);
    let text = onboard::collapse(&format!("{}{}", &c[..start], &c[cut..]));
    let text = trim(&text);
    if text.is_empty() {
        String::new()
    } else {
        format!("{text}\n")
    }
}
fn builtin(a: &Action<'_>, provider: &str, action: &str) -> Result<Recipe> {
    let cwd = &a.ctx.cwd;
    let check = action == "check";
    let remove = action == "remove";
    match provider {
        "claude" => {
            let (ok, text, next) = setup_claude::prepare(cwd, action)?;
            let mut r = Recipe::result(ok, text);
            if let Some(next) = next {
                r = r.write(cwd.join(".claude/settings.json"), Some(next), false)
            }
            Ok(r)
        }
        "cursor" => {
            let path = cwd.join(".cursor/rules/luuna-atlas.mdc");
            let exists = path.exists();
            let content = if check || remove {
                onboard::inspect(&path)
            } else {
                onboard::read(&path)?
            };
            let expected = onboard::footer("cursor", &a.config().unwrap_or_default());
            if check {
                return Ok(if !exists {
                    Recipe::result(false, "Cursor rule file not found.")
                } else if content != expected {
                    Recipe::result(false, "Cursor rule file exists but has been modified.")
                } else {
                    Recipe::result(true, "Cursor rule is installed and correct.")
                });
            }
            if remove {
                return Ok(if exists {
                    Recipe::result(true, "Removed Cursor rule file.").write(path, None, false)
                } else {
                    Recipe::result(true, "Cursor rule not found; nothing to remove.")
                });
            }
            if exists && content == expected {
                Ok(Recipe::result(true, "Cursor rule already installed."))
            } else {
                if exists && !content.is_empty() {
                    return Err(message("Refusing to overwrite modified Cursor rule body; move or explicitly remove it before setup. Nothing was written."));
                }
                Ok(Recipe::result(
                    true,
                    "Installed Cursor rule at .cursor/rules/luuna-atlas.mdc.",
                )
                .write(path, Some(expected), false))
            }
        }
        "codex" => {
            let agents = cwd.join("AGENTS.md");
            let toml = cwd.join(".codex/config.toml");
            let ac = if check || remove {
                onboard::inspect(&agents)
            } else {
                onboard::read(&agents)?
            };
            let tc = if check || remove {
                onboard::inspect(&toml)
            } else {
                onboard::read(&toml)?
            };
            let aa = onboard::has_marker_section(&ac);
            let ta = tc.contains(CODEX_START);
            if check {
                return Ok(if !agents.exists() {
                    Recipe::result(false, "AGENTS.md not found.")
                } else if !aa {
                    Recipe::result(false, "AGENTS.md exists but has no luuna-atlas section.")
                } else if !toml.exists() {
                    Recipe::result(false, ".codex/config.toml not found.")
                } else if !ta {
                    Recipe::result(
                        false,
                        ".codex/config.toml exists but has no luuna-atlas SessionStart hook.",
                    )
                } else {
                    Recipe::result(
                        true,
                        "Codex integration installed (AGENTS.md + .codex/config.toml).",
                    )
                });
            }
            let mut r = Recipe::result(true, "");
            let mut items = Vec::new();
            if remove {
                if aa {
                    r = r.write(agents, Some(onboard::remove_marker_section(&ac)), false);
                    items.push("AGENTS.md luuna-atlas section")
                }
                if ta {
                    r = r.write(toml, Some(remove_codex(&tc)), false);
                    items.push(".codex/config.toml SessionStart hook")
                }
                r.message = if items.is_empty() {
                    "No Codex integration found; nothing to remove.".into()
                } else {
                    format!("Removed Codex integration: {}.", items.join(", "))
                }
            } else {
                if !aa {
                    let section = onboard::footer("codex", &a.config().unwrap_or_default());
                    r = r.write(agents, Some(append(&ac, &section)), false);
                    items.push("AGENTS.md luuna-atlas section")
                }
                if !ta {
                    let next = if tc.trim().is_empty() {
                        format!("{}\n", codex_block())
                    } else {
                        append(&tc, &codex_block())
                    };
                    r = r.write(toml, Some(next), false);
                    items.push(".codex/config.toml SessionStart hook")
                }
                r.message = if items.is_empty() {
                    "Codex integration already installed (AGENTS.md + .codex/config.toml).".into()
                } else {
                    format!("Installed Codex integration: {}.", items.join(", "))
                }
            }
            Ok(r)
        }
        _ => unreachable!(),
    }
}
fn git_hook(cwd: &Path, action: &str) -> Result<Recipe> {
    let path = cwd.join(".git/hooks/pre-commit");
    let content = if action == "check" || action == "remove" {
        onboard::inspect(&path)
    } else {
        onboard::read(&path)?
    };
    let exists = path.exists();
    let has = content.contains("# luuna-atlas:start");
    if action == "check" {
        return Ok(if !exists {
            Recipe::result(false, "Git pre-commit hook not found.")
        } else if !has {
            Recipe::result(
                false,
                "Git pre-commit hook exists but has no luuna-atlas section.",
            )
        } else {
            Recipe::result(true, "Git pre-commit hook is installed.")
        });
    }
    if action == "remove" {
        if !exists {
            return Ok(Recipe::result(
                true,
                "Git pre-commit hook not found; nothing to remove.",
            ));
        }
        if !has {
            return Ok(Recipe::result(
                true,
                "No luuna-atlas section in pre-commit hook; nothing to remove.",
            ));
        }
        let clean = onboard::collapse(&super::text_bytes::remove_git_section(&content));
        let clean = clean.trim();
        return Ok(if clean.is_empty() || clean == "#!/bin/sh" {
            Recipe::result(true, "Removed luuna-atlas pre-commit hook (file deleted).")
                .write(path, None, false)
        } else {
            Recipe::result(true, "Removed luuna-atlas section from pre-commit hook.").write(
                path,
                Some(format!("{clean}\n")),
                false,
            )
        });
    }
    if !cwd.join(".git").exists() {
        return Ok(Recipe::result(
            false,
            "Not a git repository — .git directory not found.",
        ));
    }
    if has {
        return Ok(Recipe::result(
            true,
            "Git pre-commit hook already installed.",
        ));
    }
    let next = if content.is_empty() {
        format!("#!/bin/sh\n\n{}\n", crate::assets::get("git_hook"))
    } else {
        append(&content, crate::assets::get("git_hook"))
    };
    Ok(
        Recipe::result(true, "Installed luuna-atlas pre-commit git hook.").write(
            path,
            Some(next),
            true,
        ),
    )
}
fn apply(r: &Recipe) -> Result<()> {
    for (path, text, chmod) in &r.writes {
        if let Some(text) = text {
            onboard::write(path, text)?;
        } else {
            fs::remove_file(path)
                .map_err(|e| message(format!("remove {}: {e}", path.display())))?;
        }
        if *chmod {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(path, fs::Permissions::from_mode(0o755))
                    .map_err(|e| message(format!("chmod {}: {e}", path.display())))?;
            }
        }
    }
    Ok(())
}
fn emit(a: &mut Action<'_>, r: &Recipe, provider: Option<&super::recipes::Provider>, action: &str) {
    if a.ctx.globals.json {
        let mut o = Object::new();
        o.insert("success", r.ok);
        o.insert("command", "setup");
        if let Some(provider) = provider {
            o.insert("provider", provider.name.clone());
            o.insert("source", provider.source.clone());
            if !provider.path.as_os_str().is_empty() {
                o.insert("path", provider.path.to_string_lossy().into_owned());
            }
        } else {
            o.insert("target", "hooks");
        }
        o.insert("action", action);
        o.insert("message", r.message.clone());
        o.remember_order([
            "success", "command", "target", "provider", "source", "path", "action", "message",
        ]);
        a.output.stdout += &crate::CommandOutput::emit(&o).stdout;
    } else if r.ok {
        a.output.stdout += &format!("{}\n", a.styled("green", &format!("✔ {}", r.message)))
    } else if provider.is_some() && action == "check" {
        a.output.stdout += &format!("{}\n", a.styled("yellow", &format!("✖ {}", r.message)))
    } else {
        a.output.stderr += &format!(
            "{}\n",
            a.styled(
                "red",
                &format!(
                    "{}{}",
                    if provider.is_some() {
                        "Error: "
                    } else {
                        "✖ "
                    },
                    r.message
                )
            )
        )
    }
    if !r.ok {
        a.output.exit_code = 1;
    }
}
pub(crate) fn run(a: &mut Action<'_>, args: &[String], flags: &[(String, String)]) -> Result<()> {
    if !a.ctx.root.join(".luuna/atlas").exists() {
        return Err(message(
            "No .luuna/atlas/ directory found. Run `luuna-atlas init` first.",
        ));
    }
    if boolean(flags, "list") {
        let providers = super::recipes::listings(a);
        if a.ctx.globals.json {
            let items = providers
                .iter()
                .map(|r| {
                    let mut o = Object::new();
                    o.insert("name", r.name.clone());
                    o.insert("source", r.source.clone());
                    if !r.path.as_os_str().is_empty() {
                        o.insert(
                            "path",
                            r.path
                                .strip_prefix(&a.ctx.cwd)
                                .unwrap_or(&r.path)
                                .to_string_lossy()
                                .into_owned(),
                        );
                    }
                    if !r.shadow.is_empty() {
                        o.insert("shadowed_by", r.shadow.clone());
                    }
                    o.remember_order(["name", "source", "path", "shadowed_by"]);
                    Value::Object(o)
                })
                .collect();
            let mut o = Object::new();
            o.insert("action", "list");
            o.insert("providers", Value::Array(items));
            a.json("setup", o);
        } else {
            a.output.stdout += &format!("{}\n", a.styled("bold", "Available providers:"));
            let width = providers
                .iter()
                .map(|r| r.name.len())
                .max()
                .unwrap_or(6)
                .max(6);
            for r in providers {
                let (mut label, marker) = if r.shadow.is_empty() {
                    ("built-in".to_owned(), a.styled("green", "✓"))
                } else {
                    (
                        a.styled("dim", &format!("built-in (shadowed by {})", r.shadow)),
                        a.styled("dim", "·"),
                    )
                };
                if r.source != "builtin" {
                    label = format!(
                        "{}: {}",
                        r.source,
                        r.path.strip_prefix(&a.ctx.cwd).unwrap_or(&r.path).display()
                    );
                }
                a.output.stdout += &format!("  {marker} {:width$}  {label}\n", r.name);
            }
            a.output.stdout+=&format!("\n{}\n",a.styled("dim","Resolution order: filesystem (.luuna/atlas/recipes/<name>.{ts,sh}) → npm (luuna-atlas-recipe-<name>) → built-in."));
        }
        return Ok(());
    }
    let provider = args.first().map(String::as_str).unwrap_or("");
    let hooks = boolean(flags, "hooks");
    if provider.is_empty() && !hooks {
        a.error(
            "setup",
            if a.ctx.globals.json {
                "Specify a provider, use --hooks, or use --list."
            } else {
                "specify a provider, use --hooks, or use --list."
            },
        );
        return Ok(());
    }
    let action = if boolean(flags, "check") {
        "check"
    } else if boolean(flags, "remove") {
        "remove"
    } else {
        "install"
    };
    let hook = if hooks {
        Some(git_hook(&a.ctx.cwd, action)?)
    } else {
        None
    };
    let resolved = if provider.is_empty() {
        None
    } else {
        super::recipes::resolve(a, provider)
    };
    let r = if let Some(r) = resolved.as_ref().filter(|r| r.source == "builtin") {
        Some(match builtin(a, &r.name, action) {
            Ok(r) => r,
            Err(e) => {
                if e.to_string().starts_with("Refusing") {
                    return Err(e);
                }
                Recipe::result(
                    false,
                    format!("recipe \"{provider}\" {action} threw (built-in): {e}"),
                )
            }
        })
    } else {
        None
    };
    if let Some(hook) = hook {
        apply(&hook)?;
        emit(a, &hook, None, action)
    }
    if provider.is_empty() {
        return Ok(());
    }
    if let Some(r) = r {
        apply(&r)?;
        emit(a, &r, resolved.as_ref(), action)
    } else if let Some(resolved) = resolved {
        match super::recipes::external(a, &resolved, action) {
            Ok((ok, text)) => emit(a, &Recipe::result(ok, text), Some(&resolved), action),
            Err(e) => a.error("setup", &e.to_string()),
        }
    } else {
        a.error("setup",&format!("Unknown provider \"{provider}\". Run `luuna-atlas setup --list` to see discovered providers, or add a recipe at .luuna/atlas/recipes/{provider}.{{ts,sh}} or install luuna-atlas-recipe-{provider}."));
    }
    Ok(())
}
