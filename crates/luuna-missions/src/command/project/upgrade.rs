//! The pinned Go upgrade command's fixed refusal; no process or network access.
use crate::command::{options, output::Envelope, Context, Failure};
pub(super) fn run(context: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    let opts = options::parse(args, &[], &["check", "json"])?;
    if !opts.positionals.is_empty() {
        return Err(Failure::message("too many arguments for 'upgrade'"));
    }
    let message = "Luuna Missions is not published; update through a reviewed Luuna checkout.";
    context.exit_code = 1;
    if opts.has("json") {
        context.json(
            Envelope::new(false, "upgrade")
                .string("error", message)
                .boolean("updated", false),
        );
    } else {
        context.stderr.extend_from_slice(message.as_bytes());
        context.stderr.push(b'\n');
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::config_tree::tree::test_support::{invoke, project};
    #[test]
    fn always_refuses_like_go() {
        let t = project("project: demo\nversion: \"1\"\n");
        let out = invoke(t.path(), &["upgrade", "--check", "--json"]);
        assert_eq!(out.exit_code, 1);
        assert!(out.stderr.is_empty());
        assert_eq!(out.stdout,b"{\n  \"success\": false,\n  \"command\": \"upgrade\",\n  \"error\": \"Luuna Missions is not published; update through a reviewed Luuna checkout.\",\n  \"updated\": false\n}\n");
        assert_eq!(
            invoke(t.path(), &["upgrade"]).stderr,
            b"Luuna Missions is not published; update through a reviewed Luuna checkout.\n"
        );
        assert_eq!(
            std::fs::read_dir(t.path().join(".luuna/missions"))
                .unwrap()
                .count(),
            1
        );
    }
}
