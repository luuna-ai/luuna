//! Native terminal frontend. Environments and working directories are explicit.
//! Raw library operations retain their reference semantics; managed callers admit
//! bounded inputs before invoking this frontend.
use crate::*;
use std::{collections::BTreeMap, path::Path, time::Instant};
mod artifacts;
mod authoring;
mod prompts;
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Output {
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}
struct Context<'a> {
    beacon: Beacon,
    json: bool,
    quiet: bool,
    env: &'a BTreeMap<String, String>,
    managed: bool,
    out: Output,
}
type Result<T> = std::result::Result<T, String>;
fn result<T>(r: std::result::Result<T, BeaconError>) -> Result<T> {
    r.map_err(|e| e.to_string())
}
fn quote(s: &str) -> String {
    let mut out = String::new();
    crate::gojson::write_string(&mut out, s);
    out
}
fn object(fields: impl IntoIterator<Item = (impl AsRef<str>, String)>) -> String {
    let fields: BTreeMap<String, String> = fields
        .into_iter()
        .map(|(k, v)| (k.as_ref().to_owned(), v))
        .collect();
    format!(
        "{{{}}}",
        fields
            .iter()
            .map(|(k, v)| format!("{}:{v}", quote(k)))
            .collect::<Vec<_>>()
            .join(",")
    )
}
fn array(items: impl IntoIterator<Item = String>) -> String {
    format!("[{}]", items.into_iter().collect::<Vec<_>>().join(","))
}
fn strings(items: &[String]) -> String {
    array(items.iter().map(|s| quote(s)))
}
fn pretty(raw: &str) -> String {
    let chars: Vec<char> = raw.chars().collect();
    let mut out = String::new();
    let mut depth = 0usize;
    let mut string = false;
    let mut escaped = false;
    for (i, &c) in chars.iter().enumerate() {
        if string {
            out.push(c);
            if escaped {
                escaped = false
            } else if c == '\\' {
                escaped = true
            } else if c == '"' {
                string = false
            }
            continue;
        }
        match c {
            '"' => {
                string = true;
                out.push(c)
            }
            '{' | '[' => {
                out.push(c);
                depth += 1;
                if chars.get(i + 1).is_some_and(|c| *c != '}' && *c != ']') {
                    out.push('\n');
                    out.push_str(&"  ".repeat(depth));
                }
            }
            '}' | ']' => {
                depth = depth.saturating_sub(1);
                if i > 0 && chars[i - 1] != '{' && chars[i - 1] != '[' {
                    out.push('\n');
                    out.push_str(&"  ".repeat(depth));
                }
                out.push(c)
            }
            ',' => {
                out.push_str(",\n");
                out.push_str(&"  ".repeat(depth));
            }
            ':' => out.push_str(": "),
            c if c.is_ascii_whitespace() => {}
            _ => out.push(c),
        }
    }
    out.push('\n');
    out
}
impl Context<'_> {
    fn json(&mut self, raw: String) {
        self.out.stdout.extend_from_slice(pretty(&raw).as_bytes());
    }
    fn respond(
        &mut self,
        command: &str,
        fields: impl IntoIterator<Item = (&'static str, String)>,
        human: &str,
    ) {
        self.respond_success(command, fields, human, true)
    }
    fn respond_success(
        &mut self,
        command: &str,
        fields: impl IntoIterator<Item = (&'static str, String)>,
        human: &str,
        success: bool,
    ) {
        if self.json {
            let mut f = vec![
                ("command", quote(command)),
                ("success", success.to_string()),
            ];
            f.extend(fields);
            self.json(object(f));
        } else if !self.quiet {
            self.out.stdout.extend_from_slice(human.as_bytes());
            self.out.stdout.push(b'\n');
        }
    }
    fn error(&mut self, command: &str, error: &str) {
        if self.json {
            self.json(object([
                ("command", quote(command)),
                ("success", "false".into()),
                ("error", quote(error)),
            ]));
        } else {
            self.out
                .stderr
                .extend_from_slice(format!("Error: {error}\n").as_bytes());
        }
    }
}
fn has(args: &[String], key: &str) -> bool {
    args.iter().any(|a| a == key)
}
fn option<'a>(args: &'a [String], key: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|w| w[0] == key && !w[1].starts_with("--"))
        .map(|w| w[1].as_str())
}
fn value<'a>(args: &'a [String], key: &str) -> &'a str {
    option(args, key).unwrap_or("")
}
fn options(args: &[String], key: &str) -> Vec<String> {
    args.windows(2)
        .filter(|w| w[0] == key && !w[1].starts_with("--"))
        .map(|w| w[1].clone())
        .collect()
}
fn required(args: &[String], at: usize, message: &str) -> Result<String> {
    args.get(at).cloned().ok_or_else(|| message.into())
}
/// Executes a complete legacy command without a Go executable, Bun or source lookup.
pub fn run(cwd: &Path, args: &[String], env: &BTreeMap<String, String>) -> Output {
    run_with_policy(cwd, args, env, false, None)
}
/// Embedded terminal execution with an admission callback after help/version parsing.
/// The callback must finish all resource checks before the first command mutation.
pub fn run_managed_checked(
    cwd: &Path,
    args: &[String],
    env: &BTreeMap<String, String>,
    check: &dyn Fn() -> std::result::Result<(), String>,
) -> Output {
    run_with_policy(cwd, args, env, true, Some(check))
}
fn run_with_policy(
    cwd: &Path,
    args: &[String],
    env: &BTreeMap<String, String>,
    managed: bool,
    check: Option<&dyn Fn() -> std::result::Result<(), String>>,
) -> Output {
    let start = Instant::now();
    let json = has(args, "--json");
    let quiet = has(args, "--quiet") || has(args, "-q");
    let timing = has(args, "--timing");
    let args: Vec<String> = args
        .iter()
        .filter(|a| !matches!(a.as_str(), "--json" | "--quiet" | "-q" | "--timing"))
        .cloned()
        .collect();
    let mut out = Output::default();
    if has(&args, "--version") || has(&args, "-v") {
        let os = if cfg!(target_os = "macos") {
            "darwin"
        } else {
            std::env::consts::OS
        };
        let arch = match std::env::consts::ARCH {
            "x86_64" => "amd64",
            "aarch64" => "arm64",
            other => other,
        };
        out.stdout = if json {
            pretty(&object([
                ("name", quote("@luuna/beacon-cli")),
                ("version", quote("0.2.6")),
                ("runtime", quote("go")),
                ("platform", quote(&format!("{os}-{arch}"))),
            ]))
            .into_bytes()
        } else {
            b"0.2.6\n".to_vec()
        };
        return out;
    }
    if args.is_empty() || matches!(args[0].as_str(), "--help" | "-h") {
        out.stdout="Luuna Beacon v0.2.6 — Prompt management & composition\n\nUsage: luuna-beacon <command> [options]\n".as_bytes().to_vec();
        return out;
    }
    if has(&args[1..], "--help") || has(&args[1..], "-h") {
        out.stdout = format!("Usage: luuna-beacon {} [options]\n", args.join(" ")).into_bytes();
        return out;
    }
    let beacon = match Beacon::open(cwd) {
        Ok(b) => b,
        Err(e) => {
            out.exit_code = 1;
            out.stderr = format!("{e}\n").into_bytes();
            return out;
        }
    };
    let mut ctx = Context {
        beacon,
        json,
        quiet,
        env,
        managed,
        out,
    };
    let legacy_before = cwd.join(".canopy").exists();
    let admitted = check.map_or(Ok(()), |check| check());
    if let Err(e) = admitted.and_then(|()| dispatch(&mut ctx, &args)) {
        ctx.error(&args[0], &e);
        ctx.out.exit_code = 1;
    }
    if legacy_before && !cwd.join(".canopy").exists() && cwd.join(".luuna/beacon").is_dir() {
        let mut notice = b"Migrated .canopy to .luuna/beacon\n".to_vec();
        notice.extend(ctx.out.stderr);
        ctx.out.stderr = notice;
    }
    if timing && ctx.out.exit_code == 0 {
        ctx.out
            .stderr
            .extend_from_slice(format!("[timing] {}ms\n", start.elapsed().as_millis()).as_bytes());
    }
    ctx.out
}
fn dispatch(c: &mut Context<'_>, args: &[String]) -> Result<()> {
    match args[0].as_str() {
        "init" => {
            let init = result(c.beacon.init())?;
            c.respond(
                "init",
                [("dir", quote(&init.dir.to_string_lossy()))],
                "Initialized .luuna/beacon",
            );
            Ok(())
        }
        "create" | "update" | "archive" | "list" | "show" | "render" | "history" | "schema"
        | "validate" => prompts::dispatch(c, &args[0], &args[1..]),
        "emit" | "stats" | "tree" | "pin" | "unpin" | "config" | "import" | "diff" => {
            authoring::dispatch(c, &args[0], &args[1..])
        }
        "skill" | "agent" | "bundle" | "prime" | "onboard" | "doctor" | "sync" | "completions" => {
            artifacts::dispatch(c, &args[0], &args[1..])
        }
        "upgrade" => {
            let message =
                "Luuna Beacon is not published; update through a reviewed Luuna checkout.";
            if c.json {
                c.respond_success(
                    "upgrade",
                    [("error", quote(message)), ("updated", "false".into())],
                    "",
                    false,
                );
            } else {
                c.out
                    .stderr
                    .extend_from_slice(format!("{message}\n").as_bytes());
            }
            c.out.exit_code = 1;
            Ok(())
        }
        _ => Err(format!("unknown command: {}", args[0])),
    }
}
