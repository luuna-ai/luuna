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
    let args: Vec<&str> = env
        .args()
        .iter()
        .map(String::as_str)
        .take_while(|a| *a != "--")
        .collect();
    let has = |values: &[&str]| values.iter().any(|v| args.contains(v));
    let (mut force, mut defined) = (0i64, false);
    if has(&[
        "--no-color",
        "--no-colors",
        "--color=false",
        "--color=never",
    ]) {
        force = 0;
        defined = true;
    } else if has(&["--color", "--colors", "--color=true", "--color=always"]) {
        force = 1;
        defined = true;
    }
    let mut invalid_force = false;
    if let Some(raw) = env.var("FORCE_COLOR") {
        defined = true;
        match raw.as_str() {
            "true" | "" => force = 1,
            "false" => force = 0,
            other => match force_prefix(other) {
                None => {
                    invalid_force = true;
                    force = 0;
                }
                Some(n) => force = n.min(3),
            },
        }
    }
    if defined && force == 0 && !invalid_force {
        return 0;
    }
    if has(&["--color=16m", "--color=full", "--color=truecolor"]) {
        return 3;
    }
    if has(&["--color=256"]) {
        return 2;
    }
    let present = |key: &str| env.var(key).is_some();
    if present("TF_BUILD") && present("AGENT_NAME") {
        return 1;
    }
    if !env.stdout_is_terminal() && !defined {
        return 0;
    }
    // Go's int is 64 bits; the level is returned as i32, saturating (inferred, only
    // reachable through a FORCE_COLOR beyond i32).
    let minimum = if !defined || invalid_force {
        0
    } else {
        force.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
    };
    let term = env.var("TERM").unwrap_or_default();
    if term == "dumb" {
        return minimum;
    }
    if present("CI") {
        if ["GITHUB_ACTIONS", "GITEA_ACTIONS", "CIRCLECI"]
            .iter()
            .any(|k| present(k))
        {
            return 3;
        }
        if ["TRAVIS", "APPVEYOR", "GITLAB_CI", "BUILDKITE", "DRONE"]
            .iter()
            .any(|k| present(k))
        {
            return 1;
        }
        if env.var("CI_NAME").as_deref() == Some("codeship") {
            return 1;
        }
        return minimum;
    }
    if present("TEAMCITY_VERSION") {
        let version = env.var("TEAMCITY_VERSION").unwrap_or_default();
        return i32::from(teamcity_supports_color(&version));
    }
    if env.var("COLORTERM").as_deref() == Some("truecolor")
        || ["xterm-kitty", "xterm-ghostty", "wezterm"].contains(&term.as_str())
    {
        return 3;
    }
    if present("TERM_PROGRAM") {
        match env.var("TERM_PROGRAM").unwrap_or_default().as_str() {
            "iTerm.app" => {
                let full = env.var("TERM_PROGRAM_VERSION").unwrap_or_default();
                let major = full.split('.').next().unwrap_or("");
                return if go_atoi(major) >= 3 { 3 } else { 2 };
            }
            "Apple_Terminal" => return 2,
            _ => {}
        }
    }
    if term_is_256(&term) {
        return 2;
    }
    if term_is_basic_color(&term) {
        return 1;
    }
    if present("COLORTERM") {
        return 1;
    }
    minimum
}

/// `regexp.MustCompile("^[ \\t]*[+-]?[0-9]+").FindString(raw)` followed by
/// `strconv.Atoi(strings.TrimSpace(prefix))`: `None` when there is no match, else the
/// number, saturating at the 64-bit limits as Atoi's range error returns them.
fn force_prefix(raw: &str) -> Option<i64> {
    let rest = raw.trim_start_matches([' ', '\t']);
    let (negative, digits) = match rest.as_bytes().first() {
        Some(b'+') => (false, &rest[1..]),
        Some(b'-') => (true, &rest[1..]),
        _ => (false, rest),
    };
    let run = digits.bytes().take_while(u8::is_ascii_digit).count();
    if run == 0 {
        return None;
    }
    Some(saturating_decimal(&digits[..run], negative))
}

/// `strconv.Atoi` ignoring the error: 0 for text that is not a decimal integer (an
/// optional sign and ASCII digits only), the 64-bit limit for one that is out of range.
fn go_atoi(text: &str) -> i64 {
    let (negative, digits) = match text.as_bytes().first() {
        Some(b'+') => (false, &text[1..]),
        Some(b'-') => (true, &text[1..]),
        _ => (false, text),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return 0;
    }
    saturating_decimal(digits, negative)
}

fn saturating_decimal(digits: &str, negative: bool) -> i64 {
    let mut value: i128 = 0;
    for b in digits.bytes() {
        value = value * 10 + i128::from(b - b'0');
        if value > i128::from(i64::MAX) + 1 {
            break;
        }
    }
    if negative {
        i64::try_from(-value).unwrap_or(i64::MIN)
    } else {
        i64::try_from(value).unwrap_or(i64::MAX)
    }
}

/// `^(9\.(0*[1-9][0-9]*)\.|[0-9]{2,}\.)` on `TEAMCITY_VERSION`.
fn teamcity_supports_color(version: &str) -> bool {
    let digits = |s: &str| s.bytes().take_while(u8::is_ascii_digit).count();
    // `[0-9]{2,}\.`: a run of two or more digits at the start, then a dot.
    let first = digits(version);
    if first >= 2 && version.as_bytes().get(first) == Some(&b'.') {
        return true;
    }
    // `9\.(0*[1-9][0-9]*)\.`: nine, a dot, a digit run with a non-zero digit, a dot.
    if let Some(rest) = version.strip_prefix("9.") {
        let run = digits(rest);
        return rest.as_bytes()[..run].iter().any(|b| *b != b'0')
            && rest.as_bytes().get(run) == Some(&b'.');
    }
    false
}

/// Go's `(?i)` folding of one character of the ASCII patterns used below: ASCII case,
/// and U+017F (long s) for `s`, as Unicode simple folding has it.
fn fold_eq(c: char, want: char) -> bool {
    c.to_ascii_lowercase() == want || (want == 's' && c == '\u{17f}')
}

fn matches_at(chars: &[char], at: usize, word: &str) -> bool {
    let want: Vec<char> = word.chars().collect();
    chars.len() >= at + want.len() && want.iter().zip(&chars[at..]).all(|(w, c)| fold_eq(*c, *w))
}

/// `(?i)-256(color)?$`.
fn term_is_256(term: &str) -> bool {
    let chars: Vec<char> = term.chars().collect();
    [("-256color", 9usize), ("-256", 4)]
        .iter()
        .any(|(word, len)| chars.len() >= *len && matches_at(&chars, chars.len() - len, word))
}

/// `(?i)^screen|^xterm|^vt100|^vt220|^rxvt|color|ansi|cygwin|linux`.
fn term_is_basic_color(term: &str) -> bool {
    let chars: Vec<char> = term.chars().collect();
    if ["screen", "xterm", "vt100", "vt220", "rxvt"]
        .iter()
        .any(|w| matches_at(&chars, 0, w))
    {
        return true;
    }
    (0..chars.len()).any(|i| {
        ["color", "ansi", "cygwin", "linux"]
            .iter()
            .any(|w| matches_at(&chars, i, w))
    })
}

/// `Style(name, text)` at colour `level`: wraps `text` in the open and close codes of the
/// named style, re-opening after every close code inside `text` and around every `\r?\n`.
/// Level 0, an empty text or an unknown name return `text` unchanged.
pub fn style(level: i32, name: &str, text: &str) -> String {
    if level == 0 || text.is_empty() {
        return text.to_owned();
    }
    let (open, close) = match name {
        "brand" => (rgb_code(139, 90, 43, level), "39"),
        "accent" => (rgb_code(255, 183, 77, level), "39"),
        "muted" => (rgb_code(120, 120, 110, level), "39"),
        "red" => ("31".to_owned(), "39"),
        "green" => ("32".to_owned(), "39"),
        "yellow" => ("33".to_owned(), "39"),
        "blue" => ("34".to_owned(), "39"),
        "magenta" => ("35".to_owned(), "39"),
        "cyan" => ("36".to_owned(), "39"),
        "white" => ("37".to_owned(), "39"),
        "bold" => ("1".to_owned(), "22"),
        "dim" => ("2".to_owned(), "22"),
        _ => return text.to_owned(),
    };
    let start = format!("\x1b[{open}m");
    let end = format!("\x1b[{close}m");
    let reopened = text.replace(&end, &format!("{end}{start}"));
    // `\r?\n` -> end + match + start; a lone `\r` is left alone.
    let mut body = String::with_capacity(reopened.len());
    let mut chars = reopened.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\n' => {
                body.push_str(&end);
                body.push('\n');
                body.push_str(&start);
            }
            '\r' if chars.peek() == Some(&'\n') => {
                chars.next();
                body.push_str(&end);
                body.push_str("\r\n");
                body.push_str(&start);
            }
            other => body.push(other),
        }
    }
    format!("{start}{body}{end}")
}

/// `colorRGBCode`: the SGR parameter for an RGB colour at `level` (true colour, the
/// 256-colour cube or grey ramp, or one of the 16 basic colours), rounding as Go's
/// `math.Round` (half away from zero).
fn rgb_code(r: u8, g: u8, b: u8, level: i32) -> String {
    let (r, g, b) = (i32::from(r), i32::from(g), i32::from(b));
    if level >= 3 {
        return format!("38;2;{r};{g};{b}");
    }
    let round = |x: f64| x.round() as i32;
    if level == 2 {
        let n = if r == g && g == b {
            if r < 8 {
                16
            } else if r > 248 {
                231
            } else {
                round((f64::from(r) - 8.0) / 247.0 * 24.0) + 232
            }
        } else {
            16 + 36 * round(f64::from(r) / 255.0 * 5.0)
                + 6 * round(f64::from(g) / 255.0 * 5.0)
                + round(f64::from(b) / 255.0 * 5.0)
        };
        return format!("38;5;{n}");
    }
    let value = round(f64::from(r.max(g.max(b))) / 255.0 * 2.0);
    if value == 0 {
        return "30".to_owned();
    }
    let mut code = 30
        + ((round(f64::from(b) / 255.0) << 2)
            | (round(f64::from(g) / 255.0) << 1)
            | round(f64::from(r) / 255.0));
    if value == 2 {
        code += 60;
    }
    code.to_string()
}

/// The Go tool's `a.Error` human line: `Error: <message>` in red, with the newline
/// `fmt.Fprintln` adds.
pub fn error_line(level: i32, message: &str) -> String {
    format!("{}\n", style(level, "red", &format!("Error: {message}")))
}

/// The Go tool's `a.Warning` line: `Warning: <text>` in yellow, with the newline
/// `fmt.Fprintln` adds.
pub fn warning_line(level: i32, text: &str) -> String {
    format!("{}\n", style(level, "yellow", &format!("Warning: {text}")))
}

/// Whether the process's stdout is a terminal: the `TIOCGWINSZ` ioctl on fd 1 succeeds
/// (unix), as the Go tool's `stdoutIsTerminal`; `std::io::IsTerminal` elsewhere. Read by
/// [`Env::from_process`]; everything else reads `Env::stdout_is_terminal`.
#[cfg(unix)]
pub fn stdout_is_terminal() -> bool {
    let mut size = [0u16; 4];
    // SAFETY: TIOCGWINSZ on fd 1 only writes a `struct winsize` (four u16) through the
    // pointer; `size` is exactly that size and outlives the call, and a closed or
    // non-terminal fd just returns -1.
    let rc = unsafe { libc::ioctl(1, libc::TIOCGWINSZ as _, size.as_mut_ptr()) };
    rc == 0
}

/// See the unix version.
#[cfg(not(unix))]
pub fn stdout_is_terminal() -> bool {
    use std::io::IsTerminal;
    std::io::stdout().is_terminal()
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
