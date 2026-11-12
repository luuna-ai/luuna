//! Colour level, terminal detection and the human error and warning lines.
//! Owner: s5-atlas-lib-2.
//!
//! A port of the Go tool's `format_color.go` (`ColorLevel`, `colorRGBCode`, `Style`) and
//! `format_color_terminal_unix.go` (`stdoutIsTerminal`). Like the Go tool it ignores
//! `NO_COLOR` (P2). Unlike it, the colour level reads the arguments and variables of the
//! [`Env`] it is given, never the process's (D3), and is passed to [`style`] explicitly.

use crate::options::Env;

/// Chalk's colour level for stdout (`ColorLevel`): 0 none, 1 basic 16 colours, 2 256
/// colours, 3 true colour. Reads only `env`'s arguments (up to `--`) and variables
/// (`FORCE_COLOR`, `TF_BUILD`, `AGENT_NAME`, `TERM`, `CI` and the CI vendors, `CI_NAME`,
/// `TEAMCITY_VERSION`, `COLORTERM`, `TERM_PROGRAM`, `TERM_PROGRAM_VERSION`) and
/// `env.stdout_is_terminal()`. As in Go, `FORCE_COLOR` is clamped above 3 but not below 0,
/// so the level can be negative (a suspected Go defect, kept).
pub fn color_level(env: &Env) -> i32 {
    // stub until t5
    let _ = env;
    0
}

/// `Style(name, text)` at colour `level`: wraps `text` in the open and close codes of the
/// named style, re-opening after every close code inside `text` and around every `\r?\n`.
/// Level 0, an empty text or an unknown name return `text` unchanged.
pub fn style(level: i32, name: &str, text: &str) -> String {
    // stub until t5: the text unchanged, as at level 0.
    let _ = (level, name);
    text.to_owned()
}

/// `colorRGBCode`: the SGR parameter for an RGB colour at `level` (true colour, the
/// 256-colour cube or grey ramp, or one of the 16 basic colours), rounding as Go's
/// `math.Round` (half away from zero).
#[cfg_attr(not(test), allow(dead_code))] // called by style from t5 on
fn rgb_code(r: u8, g: u8, b: u8, level: i32) -> String {
    // stub until t5
    let _ = (r, g, b, level);
    String::new()
}

/// The Go tool's `a.Error` human line: `Error: <message>` in red, with the newline
/// `fmt.Fprintln` adds.
pub fn error_line(level: i32, message: &str) -> String {
    // stub until t5
    let _ = (level, message);
    String::new()
}

/// The Go tool's `a.Warning` line: `Warning: <text>` in yellow, with the newline
/// `fmt.Fprintln` adds.
pub fn warning_line(level: i32, text: &str) -> String {
    // stub until t5
    let _ = (level, text);
    String::new()
}

/// Whether the process's stdout is a terminal: the `TIOCGWINSZ` ioctl on fd 1 succeeds
/// (unix), as the Go tool's `stdoutIsTerminal`; `std::io::IsTerminal` elsewhere. Read by
/// [`Env::from_process`]; everything else reads `Env::stdout_is_terminal`.
pub fn stdout_is_terminal() -> bool {
    // stub until t5: never a terminal.
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(vars: &[(&str, &str)], args: &[&str], terminal: bool) -> Env {
        Env::fixed(
            vars.iter().map(|(k, v)| (*k, *v)),
            args.iter().map(|s| (*s).to_owned()).collect(),
            terminal,
        )
    }

    #[test]
    fn force_color_levels() {
        let cases: &[(&str, i32)] = &[
            ("", 1),
            ("true", 1),
            ("false", 0),
            ("0", 0),
            ("1", 1),
            ("2", 2),
            ("3", 3),
            ("7", 3),
            (" \t2x", 2),
            ("+2", 2),
            ("-1", -1),
            ("abc", 0),
        ];
        for (value, want) in cases {
            let e = env(&[("FORCE_COLOR", value)], &[], false);
            assert_eq!(color_level(&e), *want, "FORCE_COLOR={value:?}");
        }
        // An invalid FORCE_COLOR does not stop detection on a terminal.
        let e = env(
            &[("FORCE_COLOR", "abc"), ("TERM", "xterm-256color")],
            &[],
            true,
        );
        assert_eq!(color_level(&e), 2);
        // Flags: --no-color wins over FORCE_COLOR's absence; --color=256 and --color=16m
        // apply even when stdout is not a terminal.
        assert_eq!(color_level(&env(&[], &["--no-color"], true)), 0);
        assert_eq!(color_level(&env(&[], &["--color=256"], false)), 2);
        assert_eq!(color_level(&env(&[], &["--color=16m"], false)), 3);
        assert_eq!(color_level(&env(&[], &["--color"], false)), 1);
        // Arguments after `--` are not read.
        assert_eq!(color_level(&env(&[], &["--", "--color=16m"], false)), 0);
        // TF_BUILD with AGENT_NAME is level 1 even without a terminal.
        let e = env(&[("TF_BUILD", ""), ("AGENT_NAME", "x")], &[], false);
        assert_eq!(color_level(&e), 1);
    }

    #[test]
    fn no_color_is_ignored() {
        let e = env(&[("NO_COLOR", "1"), ("FORCE_COLOR", "1")], &[], false);
        assert_eq!(color_level(&e), 1);
        let e = env(&[("NO_COLOR", "1"), ("TERM", "xterm")], &[], true);
        assert_eq!(color_level(&e), 1);
        assert_eq!(style(1, "red", "x"), "\x1b[31mx\x1b[39m");
    }

    #[test]
    fn not_a_terminal_is_zero_unless_forced() {
        let rich = [("TERM", "xterm-256color"), ("COLORTERM", "truecolor")];
        assert_eq!(color_level(&env(&rich, &[], false)), 0);
        assert_eq!(color_level(&env(&rich, &[], true)), 3);
        let forced = [
            ("TERM", "xterm-256color"),
            ("COLORTERM", "truecolor"),
            ("FORCE_COLOR", "1"),
        ];
        assert_eq!(color_level(&env(&forced, &[], false)), 3);
        // On a terminal: TERM=dumb gives the minimum, CI vendors and TERM patterns apply.
        assert_eq!(color_level(&env(&[("TERM", "dumb")], &[], true)), 0);
        let ci = [("CI", ""), ("GITHUB_ACTIONS", "true"), ("TERM", "dumb")];
        assert_eq!(color_level(&env(&ci, &[], true)), 0);
        let ci = [("CI", ""), ("GITHUB_ACTIONS", "true")];
        assert_eq!(color_level(&env(&ci, &[], true)), 3);
        let ci = [("CI", ""), ("GITLAB_CI", "1")];
        assert_eq!(color_level(&env(&ci, &[], true)), 1);
        let ci = [("CI", ""), ("CI_NAME", "codeship")];
        assert_eq!(color_level(&env(&ci, &[], true)), 1);
        assert_eq!(color_level(&env(&[("CI", "")], &[], true)), 0);
        let tc = [("TEAMCITY_VERSION", "9.1.3")];
        assert_eq!(color_level(&env(&tc, &[], true)), 1);
        let tc = [("TEAMCITY_VERSION", "9.0.1")];
        assert_eq!(color_level(&env(&tc, &[], true)), 0);
        let tc = [("TEAMCITY_VERSION", "10.0")];
        assert_eq!(color_level(&env(&tc, &[], true)), 1);
        let iterm = [
            ("TERM_PROGRAM", "iTerm.app"),
            ("TERM_PROGRAM_VERSION", "3.4"),
        ];
        assert_eq!(color_level(&env(&iterm, &[], true)), 3);
        let iterm = [
            ("TERM_PROGRAM", "iTerm.app"),
            ("TERM_PROGRAM_VERSION", "2.9"),
        ];
        assert_eq!(color_level(&env(&iterm, &[], true)), 2);
        assert_eq!(
            color_level(&env(&[("TERM", "screen-256COLOR")], &[], true)),
            2
        );
        assert_eq!(color_level(&env(&[("TERM", "vt100")], &[], true)), 1);
        assert_eq!(color_level(&env(&[("TERM", "my-ansi")], &[], true)), 1);
        assert_eq!(color_level(&env(&[("TERM", "wezterm")], &[], true)), 3);
        assert_eq!(color_level(&env(&[("COLORTERM", "1")], &[], true)), 1);
        assert_eq!(color_level(&env(&[("TERM", "unknown")], &[], true)), 0);
    }

    #[test]
    fn rgb_rounding_matches_go() {
        // brand (139,90,43), accent (255,183,77), muted (120,120,110).
        assert_eq!(rgb_code(139, 90, 43, 3), "38;2;139;90;43");
        assert_eq!(rgb_code(139, 90, 43, 2), "38;5;137");
        assert_eq!(rgb_code(255, 183, 77, 2), "38;5;222");
        assert_eq!(rgb_code(120, 120, 110, 2), "38;5;102");
        assert_eq!(rgb_code(139, 90, 43, 1), "31");
        assert_eq!(rgb_code(255, 183, 77, 1), "93");
        assert_eq!(rgb_code(120, 120, 110, 1), "30");
        // The grey ramp of the 256-colour palette.
        assert_eq!(rgb_code(5, 5, 5, 2), "38;5;16");
        assert_eq!(rgb_code(250, 250, 250, 2), "38;5;231");
        assert_eq!(rgb_code(128, 128, 128, 2), "38;5;244");
        assert_eq!(rgb_code(0, 0, 0, 1), "30");
        // A negative level (FORCE_COLOR=-1) takes the 16-colour branch, as in Go.
        assert_eq!(rgb_code(255, 183, 77, -1), "93");
        assert_eq!(style(2, "brand", "x"), "\x1b[38;5;137mx\x1b[39m");
        assert_eq!(style(3, "accent", "x"), "\x1b[38;2;255;183;77mx\x1b[39m");
    }

    #[test]
    fn nested_close_codes_reopen() {
        assert_eq!(
            style(1, "red", "a\x1b[39mb"),
            "\x1b[31ma\x1b[39m\x1b[31mb\x1b[39m"
        );
        assert_eq!(
            style(1, "bold", "a\r\nb\nc"),
            "\x1b[1ma\x1b[22m\r\n\x1b[1mb\x1b[22m\n\x1b[1mc\x1b[22m"
        );
        assert_eq!(style(0, "red", "a"), "a");
        assert_eq!(style(1, "red", ""), "");
        assert_eq!(style(1, "no-such-style", "a"), "a");
        assert_eq!(error_line(1, "boom"), "\x1b[31mError: boom\x1b[39m\n");
        assert_eq!(warning_line(0, "careful"), "Warning: careful\n");
    }
}
