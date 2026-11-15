//! Byte-exact embedded Go shell completions.
use crate::command::{Context, Failure};
pub(super) fn run(context: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    if args.is_empty() {
        return Err(Failure::message("error: missing required argument 'shell'"));
    }
    if args.len() > 1 {
        return Err(Failure::message(format!(
            "error: too many arguments for 'completions'. Expected 1 argument but got {}.",
            args.len()
        )));
    }
    let script: &[u8] = match args[0].as_str() {
        "bash" => {
            include_bytes!("../../../assets/s4-missions-lib-2/completions/completion_bash.txt")
        }
        "zsh" => include_bytes!("../../../assets/s4-missions-lib-2/completions/completion_zsh.txt"),
        "fish" => {
            include_bytes!("../../../assets/s4-missions-lib-2/completions/completion_fish.txt")
        }
        shell => {
            context.exit_code = 1;
            context.stderr.extend_from_slice(
                format!("✗ Unknown shell: {shell}. Supported: bash, zsh, fish\n").as_bytes(),
            );
            return Ok(());
        }
    };
    context.stdout.extend_from_slice(script);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::dep::test_support::invoke;
    #[test]
    fn scripts_and_errors_follow_go() {
        let t = tempfile::tempdir().unwrap();
        for (name, bytes) in [
            (
                "bash",
                include_bytes!("../../../assets/s4-missions-lib-2/completions/completion_bash.txt")
                    .as_slice(),
            ),
            (
                "zsh",
                include_bytes!("../../../assets/s4-missions-lib-2/completions/completion_zsh.txt")
                    .as_slice(),
            ),
            (
                "fish",
                include_bytes!("../../../assets/s4-missions-lib-2/completions/completion_fish.txt")
                    .as_slice(),
            ),
        ] {
            let out = invoke(t.path(), &["completions", name]);
            assert_eq!(out.stdout, bytes);
            assert!(out.stderr.is_empty());
            assert_eq!(out.exit_code, 0);
        }
        let out = invoke(t.path(), &["completions", "--json"]);
        assert_eq!(
            out.stderr,
            "✗ Unknown shell: --json. Supported: bash, zsh, fish\n".as_bytes()
        );
        assert!(out.stdout.is_empty());
        assert_eq!(out.exit_code, 1);
        assert_eq!(
            invoke(t.path(), &["completions"]).stderr,
            b"Error: error: missing required argument 'shell'\n"
        );
        assert_eq!(
            invoke(t.path(), &["completions", "bash", "extra"]).stderr,
            b"Error: error: too many arguments for 'completions'. Expected 1 argument but got 2.\n"
        );
    }
}
