//! Interactive-editor and reference-buffer primitives.
//!
//! These are deliberately separate from a command handler: `plant`, `adopt`,
//! and the later interactive sessions all need the same `$VISUAL`/`$EDITOR`
//! behavior and the same scissors contract for agent-provided reference text.

use roots_core::actor::Environment;
use roots_core::errors::{RootsError, RootsResult};
use roots_core::ids::random_hex;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

/// The familiar git-style line that separates human text from a read-only
/// reference in an editor buffer.
pub const SCISSORS: &str = "# ------------------------ >8 ------------------------";

/// `$VISUAL`, then `$EDITOR`, then the historical `vi` fallback.
#[must_use]
pub fn resolve_editor(env: &Environment) -> String {
    env.get("VISUAL")
        .map(String::as_str)
        .map(str::trim)
        .filter(|editor| !editor.is_empty())
        .or_else(|| {
            env.get("EDITOR")
                .map(String::as_str)
                .map(str::trim)
                .filter(|editor| !editor.is_empty())
        })
        .unwrap_or("vi")
        .to_owned()
}

/// Invoke an editor string exactly as the legacy CLI did.  The shell is
/// intentional: values such as `code -w` contain both a program and options,
/// and the file remains a separate positional shell argument.
pub fn open_editor(file: &Path, cwd: &Path, env: &Environment) -> RootsResult<()> {
    let editor = resolve_editor(env);
    let status = editor_status(&editor, file, cwd, env).map_err(|error| {
        RootsError::error(format!("could not start editor \"{editor}\": {error}"))
    })?;
    if status.success() {
        return Ok(());
    }
    let code = status
        .code()
        .map_or_else(|| "null".to_owned(), |code| code.to_string());
    Err(RootsError::error(format!(
        "editor \"{editor}\" exited with status {code}"
    )))
}

/// Edit an ephemeral `idea.md` buffer and return only the human portion.  The
/// buffer lives outside a Roots store so a cancelled editor never creates a
/// graph record.  `plant`, `adopt`, and Flow all share the exact scissors
/// semantics here.
pub fn compose_in_temp_editor(initial: &str, cwd: &Path, env: &Environment) -> RootsResult<String> {
    let mut directory = None;
    for _ in 0..20 {
        let candidate = std::env::temp_dir().join(format!("roots-plant-{}", random_hex(12)));
        match fs::create_dir(&candidate) {
            Ok(()) => {
                directory = Some(candidate);
                break;
            }
            Err(problem) if problem.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(problem) => return Err(RootsError::error(problem.to_string())),
        }
    }
    let directory = directory.ok_or_else(|| {
        RootsError::error("could not create a temporary directory for the editor")
    })?;
    let file = directory.join("idea.md");
    let result = (|| {
        fs::write(&file, initial).map_err(|problem| RootsError::error(problem.to_string()))?;
        open_editor(&file, cwd, env)?;
        let text =
            fs::read_to_string(&file).map_err(|problem| RootsError::error(problem.to_string()))?;
        Ok(strip_reference(&text))
    })();
    let _ = fs::remove_dir_all(directory);
    result
}

#[cfg(not(windows))]
fn editor_status(
    editor: &str,
    file: &Path,
    cwd: &Path,
    env: &Environment,
) -> std::io::Result<std::process::ExitStatus> {
    Command::new("sh")
        .args(["-c", &format!(r#"{editor} "$1""#), "roots-editor"])
        .arg(file)
        .current_dir(cwd)
        .env_clear()
        .envs(env)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
}

#[cfg(windows)]
fn editor_status(
    editor: &str,
    file: &Path,
    cwd: &Path,
    env: &Environment,
) -> std::io::Result<std::process::ExitStatus> {
    use std::os::windows::process::CommandExt;

    // The upstream implementation delegates editor strings to `sh`. Native
    // Windows does not normally ship a POSIX shell, so use its command shell
    // for the equivalent `EDITOR-with-arguments + quoted file` contract.
    // COMSPEC is read before clearing the child environment; ProcessIo still
    // passes the exact injected environment to the editor itself.
    let shell = std::env::var_os("COMSPEC").unwrap_or_else(|| "cmd.exe".into());
    let path = file.to_string_lossy().replace('"', "\\\"");
    let command = format!("{editor} \"{path}\"");
    let mut process = Command::new(shell);
    process.args(["/D", "/C"]);
    if editor.trim_start().starts_with('"') {
        // `cmd /C` does not use the normal Windows argv parser.  Passing a
        // quoted editor executable through `args` escapes its quotes into
        // literal backslashes, which breaks paths with spaces.  Give cmd its
        // required outer quote pair verbatim in that specific form.
        process.raw_arg(format!("\"{command}\""));
    } else {
        // Keep ordinary shell expressions (for example a redirection-based
        // EDITOR) unescaped, as `cmd /C` itself is the command parser. This
        // mirrors the flexible `sh -c` behavior of the legacy implementation.
        process.raw_arg(&command);
    }
    process
        .current_dir(cwd)
        .env_clear()
        .envs(env)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
}

/// An empty human editing area followed by a scissors line and reference.
#[must_use]
pub fn reference_buffer(reference: &[String]) -> String {
    let mut lines = vec![
        String::new(),
        String::new(),
        SCISSORS.to_owned(),
        "# Everything from the line above down is removed when you save.".to_owned(),
    ];
    lines.extend(reference.iter().cloned());
    format!("{}\n", lines.join("\n"))
}

/// Remove the scissors line and every line after it.  A buffer without the
/// marker is returned byte-for-byte, which matters for ordinary `plant`
/// prose.
#[must_use]
pub fn strip_reference(text: &str) -> String {
    // JavaScript's `/\r?\n/` split discards the CR in a CRLF delimiter.  Do
    // the same only while processing a buffer with scissors; ordinary prose
    // without scissors must remain byte-for-byte unchanged.
    let lines: Vec<&str> = text
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect();
    let Some(cut) = lines.iter().position(|line| line.trim_end() == SCISSORS) else {
        return text.to_owned();
    };
    let kept = lines[..cut].join("\n");
    let kept = kept.trim_end();
    if kept.is_empty() {
        String::new()
    } else {
        format!("{kept}\n")
    }
}

/// A terminal multiplexer supported by Roots split panes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Multiplexer {
    Tmux,
    Zellij,
}

impl Multiplexer {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tmux => "tmux",
            Self::Zellij => "zellij",
        }
    }
}

/// `$TMUX` takes precedence over `$ZELLIJ`, as in the TypeScript CLI.
#[must_use]
pub fn detect_multiplexer(env: &Environment) -> Option<Multiplexer> {
    if env.get("TMUX").is_some_and(|value| !value.is_empty()) {
        Some(Multiplexer::Tmux)
    } else if env.get("ZELLIJ").is_some_and(|value| !value.is_empty()) {
        Some(Multiplexer::Zellij)
    } else {
        None
    }
}

/// Shell-quote the file component used in a tmux command string.
#[must_use]
pub fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'/' | b'-'))
    {
        value.to_owned()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

/// Construct a multiplexer command that opens `command file` in a right-hand
/// pane.  `focus: false` lets adoption leave focus with the writer in tmux.
#[must_use]
pub fn split_argv(
    mux: Multiplexer,
    command: &str,
    file: &str,
    cwd: &Path,
    focus: bool,
) -> Vec<String> {
    let cwd = cwd.display().to_string();
    match mux {
        Multiplexer::Tmux => {
            let mut args = vec![
                "tmux".to_owned(),
                "split-window".to_owned(),
                "-h".to_owned(),
            ];
            if !focus {
                args.push("-d".to_owned());
            }
            args.extend([
                "-c".to_owned(),
                cwd,
                format!("{command} {}", shell_quote(file)),
            ]);
            args
        }
        Multiplexer::Zellij => vec![
            "zellij".to_owned(),
            "run".to_owned(),
            "--direction".to_owned(),
            "right".to_owned(),
            "--cwd".to_owned(),
            cwd,
            "--".to_owned(),
            "sh".to_owned(),
            "-c".to_owned(),
            format!(r#"{command} "$1""#),
            "roots-editor".to_owned(),
            file.to_owned(),
        ],
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpawnResult {
    pub status: Option<i32>,
    pub stdout: String,
}

pub type Spawner = dyn Fn(&[String], &Path) -> SpawnResult;

/// Real non-interactive process spawn used for best-effort split panes.
#[must_use]
pub fn real_spawner(argv: &[String], cwd: &Path) -> SpawnResult {
    let Some((program, args)) = argv.split_first() else {
        return SpawnResult {
            status: None,
            stdout: String::new(),
        };
    };
    match Command::new(program)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
    {
        Ok(output) => SpawnResult {
            status: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        },
        Err(_) => SpawnResult {
            status: None,
            stdout: String::new(),
        },
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SplitLaunch {
    pub mux: Option<Multiplexer>,
    pub ok: bool,
}

/// One tmux editor pane reused by a Flow.  A vim-family editor is switched to
/// the next idea with `:update | edit`; otherwise a normal split is opened.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EditorPane {
    pub id: Option<String>,
}

fn vim_family(editor: &str) -> bool {
    let program = editor.split_whitespace().next().unwrap_or_default();
    matches!(
        program.rsplit('/').next().unwrap_or(program),
        "vi" | "vim" | "nvim"
    )
}

fn vim_escape_file(file: &str) -> String {
    file.chars()
        .flat_map(|character| {
            if matches!(character, '\\' | ' ' | '%' | '#' | '|' | '"') {
                ['\\', character]
            } else {
                ['\0', character]
            }
        })
        .filter(|character| *character != '\0')
        .collect()
}

/// Show `file` in the Flow editor pane.  The explicit spawner keeps the
/// behavior fully testable and mirrors the legacy `editor-pane.ts` commands.
#[must_use]
pub fn show_in_editor_pane(
    env: &Environment,
    pane: &mut EditorPane,
    file: &str,
    cwd: &Path,
    spawner: Option<&Spawner>,
) -> SplitLaunch {
    let Some(Multiplexer::Tmux) = detect_multiplexer(env) else {
        return launch_editor_split(env, file, cwd, spawner);
    };
    let spawn = spawner.unwrap_or(&real_spawner);
    let editor = resolve_editor(env);
    if let Some(id) = pane.id.as_deref().filter(|_| vim_family(&editor)) {
        let checked = spawn(
            &[
                "tmux".to_owned(),
                "display-message".to_owned(),
                "-p".to_owned(),
                "-t".to_owned(),
                id.to_owned(),
                "#{pane_current_command}".to_owned(),
            ],
            cwd,
        );
        let running = checked.stdout.trim();
        if checked.status == Some(0) && matches!(running, "vi" | "vim" | "nvim") {
            let command = format!(":update | edit {}", vim_escape_file(file));
            let sent = spawn(
                &[
                    "tmux".to_owned(),
                    "send-keys".to_owned(),
                    "-t".to_owned(),
                    id.to_owned(),
                    "Escape".to_owned(),
                    command,
                    "Enter".to_owned(),
                ],
                cwd,
            );
            if sent.status == Some(0) {
                let _ = spawn(
                    &[
                        "tmux".to_owned(),
                        "select-pane".to_owned(),
                        "-t".to_owned(),
                        id.to_owned(),
                    ],
                    cwd,
                );
                return SplitLaunch {
                    mux: Some(Multiplexer::Tmux),
                    ok: true,
                };
            }
        }
    }
    let result = spawn(
        &[
            "tmux".to_owned(),
            "split-window".to_owned(),
            "-h".to_owned(),
            "-P".to_owned(),
            "-F".to_owned(),
            "#{pane_id}".to_owned(),
            "-c".to_owned(),
            cwd.display().to_string(),
            format!("{editor} {}", shell_quote(file)),
        ],
        cwd,
    );
    pane.id = (result.status == Some(0))
        .then(|| result.stdout.trim().to_owned())
        .filter(|id| !id.is_empty());
    SplitLaunch {
        mux: Some(Multiplexer::Tmux),
        ok: result.status == Some(0),
    }
}

pub struct SplitCommand<'a> {
    pub command: &'a str,
    pub file: &'a str,
    pub cwd: &'a Path,
    pub focus: bool,
}

/// Best-effort split launcher. Failure never blocks human adoption because the
/// same reference is printed to stderr and embedded below the scissors line.
#[must_use]
pub fn launch_in_split(
    env: &Environment,
    command: SplitCommand<'_>,
    spawner: Option<&Spawner>,
) -> SplitLaunch {
    let mux = detect_multiplexer(env);
    let Some(mux_value) = mux else {
        return SplitLaunch { mux, ok: false };
    };
    let argv = split_argv(
        mux_value,
        command.command,
        command.file,
        command.cwd,
        command.focus,
    );
    let result = match spawner {
        Some(spawn) => spawn(&argv, command.cwd),
        None => real_spawner(&argv, command.cwd),
    };
    SplitLaunch {
        mux,
        ok: result.status == Some(0),
    }
}

/// Try to open the configured editor in a right-hand split.  This is
/// deliberately best-effort: the caller still prints a path when no
/// multiplexer is present or the split command fails.
#[must_use]
pub fn launch_editor_split(
    env: &Environment,
    file: &str,
    cwd: &Path,
    spawner: Option<&Spawner>,
) -> SplitLaunch {
    let editor = resolve_editor(env);
    launch_in_split(
        env,
        SplitCommand {
            command: &editor,
            file,
            cwd,
            focus: true,
        },
        spawner,
    )
}

/// Human-facing editor guidance used in the think screen.
#[must_use]
pub fn editor_hint(launch: Option<SplitLaunch>, file: &str) -> String {
    if let Some(SplitLaunch {
        mux: Some(mux),
        ok: true,
    }) = launch
    {
        return format!("Editing {file} in a {} pane. Save to answer.", mux.as_str());
    }
    let failed = launch
        .and_then(|result| result.mux)
        .map(|mux| format!(" (could not open a {} split)", mux.as_str()))
        .unwrap_or_default();
    format!("Open {file} in your editor in another pane{failed}. Save to answer.")
}

/// `$PAGER`, falling back to `less` for the read-only adoption reference.
#[must_use]
pub fn resolve_pager(env: &Environment) -> String {
    env.get("PAGER")
        .map(String::as_str)
        .map(str::trim)
        .filter(|pager| !pager.is_empty())
        .unwrap_or("less")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    #[test]
    fn reference_scissors_keep_only_human_text() {
        let buffer = reference_buffer(&["> agent text".to_owned()]);
        assert!(buffer.starts_with("\n\n"));
        assert!(buffer.contains(SCISSORS));
        assert_eq!(strip_reference(&buffer), "");
        assert_eq!(
            strip_reference(&format!("Mine\n{SCISSORS}  \n> x\n")),
            "Mine\n"
        );
        assert_eq!(
            strip_reference(&format!("Mine\r\nOther\r\n{SCISSORS}\r\n> x\r\n")),
            "Mine\nOther\n"
        );
        assert_eq!(strip_reference("Plain\r\n"), "Plain\r\n");
    }

    #[test]
    fn editor_selection_and_split_commands_match_the_legacy_order() {
        let env = BTreeMap::from([
            ("VISUAL".to_owned(), "  code -w ".to_owned()),
            ("EDITOR".to_owned(), "vim".to_owned()),
            ("TMUX".to_owned(), "socket".to_owned()),
        ]);
        assert_eq!(resolve_editor(&env), "code -w");
        assert_eq!(resolve_pager(&env), "less");
        assert_eq!(detect_multiplexer(&env), Some(Multiplexer::Tmux));
        assert_eq!(shell_quote("sp ace.md"), "'sp ace.md'");
        assert_eq!(
            split_argv(
                Multiplexer::Tmux,
                "less",
                "agent/sprouts/a file.md",
                &PathBuf::from("/project"),
                false,
            ),
            vec![
                "tmux",
                "split-window",
                "-h",
                "-d",
                "-c",
                "/project",
                "less 'agent/sprouts/a file.md'",
            ]
        );
    }

    #[test]
    fn split_launcher_uses_injected_spawner() {
        let env = BTreeMap::from([("ZELLIJ".to_owned(), "session".to_owned())]);
        let launch = launch_in_split(
            &env,
            SplitCommand {
                command: "less",
                file: "f.md",
                cwd: Path::new("/project"),
                focus: false,
            },
            Some(&|argv, _| {
                assert_eq!(argv[0], "zellij");
                SpawnResult {
                    status: Some(0),
                    stdout: String::new(),
                }
            }),
        );
        assert_eq!(
            launch,
            SplitLaunch {
                mux: Some(Multiplexer::Zellij),
                ok: true
            }
        );
    }

    #[test]
    fn flow_reuses_a_live_vim_tmux_pane_before_opening_another_split() {
        let env = BTreeMap::from([
            ("TMUX".to_owned(), "socket".to_owned()),
            ("EDITOR".to_owned(), "nvim".to_owned()),
        ]);
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Vec<String>>::new()));
        let recorded = std::sync::Arc::clone(&calls);
        let spawn = move |argv: &[String], _cwd: &Path| {
            recorded.lock().unwrap().push(argv.to_vec());
            if argv.get(1).is_some_and(|part| part == "split-window") {
                SpawnResult {
                    status: Some(0),
                    stdout: "%12\n".to_owned(),
                }
            } else if argv.get(1).is_some_and(|part| part == "display-message") {
                SpawnResult {
                    status: Some(0),
                    stdout: "nvim\n".to_owned(),
                }
            } else {
                SpawnResult {
                    status: Some(0),
                    stdout: String::new(),
                }
            }
        };
        let mut pane = EditorPane::default();
        assert!(
            show_in_editor_pane(
                &env,
                &mut pane,
                "human/r-a/idea.md",
                Path::new("/project"),
                Some(&spawn),
            )
            .ok
        );
        assert_eq!(pane.id.as_deref(), Some("%12"));
        assert!(
            show_in_editor_pane(
                &env,
                &mut pane,
                "human/r-b/a space.md",
                Path::new("/project"),
                Some(&spawn),
            )
            .ok
        );
        let calls = calls.lock().unwrap();
        assert_eq!(calls[0][1], "split-window");
        assert_eq!(calls[1][1], "display-message");
        assert_eq!(calls[2][1], "send-keys");
        assert!(
            calls[2]
                .iter()
                .any(|part| part == ":update | edit human/r-b/a\\ space.md")
        );
        assert_eq!(calls[3][1], "select-pane");
    }

    #[cfg(windows)]
    #[test]
    fn native_windows_editor_receives_the_file_argument() {
        use std::fs;

        let root = std::env::temp_dir().join(format!(
            "roots-editor-test-{}",
            roots_core::ids::random_hex(8)
        ));
        fs::create_dir_all(&root).expect("temporary editor directory");
        let script = root.join("write idea.cmd");
        fs::write(&script, "@echo From native editor> \"%~1\"\r\n").expect("editor script");
        let file = root.join("idea.md");
        fs::write(&file, "").expect("editor file");
        let mut env = BTreeMap::from([("EDITOR".to_owned(), format!("\"{}\"", script.display()))]);
        for key in ["SystemRoot", "PATH", "PATHEXT"] {
            if let Ok(value) = std::env::var(key) {
                env.insert(key.to_owned(), value);
            }
        }
        open_editor(&file, &root, &env).expect("open editor");
        assert_eq!(
            fs::read_to_string(&file).expect("editor output").trim(),
            "From native editor"
        );
        fs::remove_dir_all(root).expect("cleanup");
    }
}
