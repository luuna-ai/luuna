//! The stable Roots command registry and help renderer.

use crate::io::Io;
use crate::output::Output;
use roots_core::args::{FlagKind, FlagSpec, Flags};
use roots_core::color::{Colors, pad_end};
use roots_core::errors::RootsResult;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandGroup {
    Setup,
    Capture,
    Structure,
    Agent,
    Read,
}

impl CommandGroup {
    const fn title(self) -> &'static str {
        match self {
            Self::Setup => "Setup",
            Self::Capture => "Capture and think (human)",
            Self::Structure => "Structure (human)",
            Self::Agent => "Agent-facing (agent)",
            Self::Read => "Read (anyone)",
        }
    }
}

const GROUPS: [CommandGroup; 5] = [
    CommandGroup::Setup,
    CommandGroup::Capture,
    CommandGroup::Structure,
    CommandGroup::Agent,
    CommandGroup::Read,
];

/// Inputs passed to a concrete command implementation.
pub struct CommandContext<'a> {
    pub command: &'static str,
    pub io: &'a mut dyn Io,
    pub out: &'a Output,
    pub args: &'a [String],
    pub flags: &'a Flags,
}

pub type CommandHandler = for<'a> fn(CommandContext<'a>) -> RootsResult<()>;

/// A public command definition. The registration order is observable in help.
#[derive(Clone, Copy)]
pub struct CommandDef {
    pub name: &'static str,
    pub group: CommandGroup,
    pub summary: &'static str,
    pub usage: &'static str,
    pub description: Option<&'static str>,
    pub flags: &'static [FlagSpec],
    pub min_args: usize,
    pub max_args: Option<usize>,
    pub run: CommandHandler,
}

pub const GLOBAL_FLAGS: &[FlagSpec] = &[
    FlagSpec::boolean("json", None, "Machine-readable JSON output"),
    FlagSpec::boolean("quiet", Some('q'), "Suppress non-essential output"),
    FlagSpec::boolean("help", Some('h'), "Show help for this command"),
];

const VERSION_FLAG: FlagSpec = FlagSpec::boolean("version", Some('v'), "Print the version");
const NONE: &[FlagSpec] = &[];
const SETUP: &[FlagSpec] = &[
    FlagSpec::boolean(
        "project",
        None,
        "Edit <project>/.claude/settings.json (default)",
    ),
    FlagSpec::boolean("user", None, "Edit ~/.claude/settings.json"),
    FlagSpec::boolean("remove", None, "Remove roots hooks"),
    FlagSpec::boolean("dry-run", None, "Show the result; write nothing"),
];
const PLANT: &[FlagSpec] = &[FlagSpec::string(
    "slug",
    None,
    false,
    "<slug>",
    "Slug to use instead of one derived from the statement",
)];
const THINK: &[FlagSpec] = &[FlagSpec::boolean(
    "no-split",
    None,
    "Do not open $EDITOR in a split pane",
)];
const ADOPT: &[FlagSpec] = &[
    FlagSpec::string(
        "slug",
        None,
        false,
        "<slug>",
        "Slug for the new idea (default: from your first line)",
    ),
    FlagSpec::boolean("no-split", None, "Do not open the sprout in a split pane"),
    FlagSpec::boolean(
        "endorse",
        None,
        "Accept the sprout as written; you write only your thoughts on it",
    ),
];
const TEND: &[FlagSpec] = &[FlagSpec::boolean(
    "no-split",
    None,
    "No split panes (think after a split; the sprout while adopting)",
)];
const ACCEPT: &[FlagSpec] = &[
    FlagSpec::string(
        "rel",
        None,
        false,
        "<serves|tension|replaces>",
        "Relation for a mention proposal",
    ),
    FlagSpec::string("keep", None, false, "<id>", "merge: the idea that survives"),
    FlagSpec::string("reason", None, false, "<text>", "Optional note"),
];
const REJECT: &[FlagSpec] = &[FlagSpec::string(
    "reason",
    None,
    false,
    "<text>",
    "Why (agents see this)",
)];
const SCAN: &[FlagSpec] = &[FlagSpec::boolean(
    "all",
    None,
    "Scan every idea, changed or not",
)];
const COMPOST: &[FlagSpec] = &[FlagSpec::string(
    "reason",
    None,
    false,
    "<text>",
    "Why it is retired",
)];
const CONTEXT: &[FlagSpec] = &[
    FlagSpec::string(
        "session",
        None,
        false,
        "<ss-id>",
        "Think session the packet is for (default: $ROOTS_SESSION)",
    ),
    FlagSpec::boolean("no-repo", None, "Skip git branch / recent commits"),
];
const PRIME: &[FlagSpec] = &[
    FlagSpec::string("scope", None, false, "<id>", "Only this idea's subtree"),
    FlagSpec::boolean(
        "hook",
        None,
        "Harness hook mode: print nothing (exit 0) outside a roots project",
    ),
];
const DRIFT: &[FlagSpec] = &[FlagSpec::string(
    "diff",
    None,
    false,
    "<rev>",
    "Compare the working tree against <rev> (default HEAD)",
)];
const ASK: &[FlagSpec] = &[
    FlagSpec::string(
        "as",
        None,
        false,
        "agent:<model>",
        "Agent identity (default: $ROOTS_AGENT)",
    ),
    FlagSpec::string(
        "session",
        None,
        false,
        "<ss-id>",
        "Attach to a running think session (default: $ROOTS_SESSION)",
    ),
];
const NOTE: &[FlagSpec] = &[
    FlagSpec::string(
        "file",
        None,
        false,
        "<path>",
        "The file to attach (required)",
    ),
    FlagSpec::string(
        "name",
        None,
        false,
        "<name>",
        "Name for the note (default: the file name)",
    ),
    FlagSpec::string(
        "question",
        None,
        false,
        "<q-id>",
        "Findings for this delegated question (reopens it for the human)",
    ),
    FlagSpec::string(
        "as",
        None,
        false,
        "agent:<model>",
        "Agent identity (default: $ROOTS_AGENT)",
    ),
];
const PROPOSE: &[FlagSpec] = &[
    FlagSpec::string(
        "reason",
        None,
        false,
        "<text>",
        "Why, in one line (required)",
    ),
    FlagSpec::string(
        "cite",
        None,
        true,
        "<id>:<quote>",
        "Exact quote from an idea (repeatable)",
    ),
    FlagSpec::boolean(
        "mention",
        None,
        "edge: cite only the mentioning line; roots cites the other idea's statement",
    ),
    FlagSpec::string(
        "as",
        None,
        false,
        "agent:<model>",
        "Agent identity (default: $ROOTS_AGENT)",
    ),
];
const SPROUT: &[FlagSpec] = &[
    FlagSpec::string("file", None, false, "<md>", "Prose after the statement"),
    FlagSpec::string(
        "slug",
        None,
        false,
        "<slug>",
        "Slug (default: from statement)",
    ),
    FlagSpec::string(
        "as",
        None,
        false,
        "agent:<model>",
        "Agent identity (default: $ROOTS_AGENT)",
    ),
];
const HEADING: &[FlagSpec] = &[
    FlagSpec::string(
        "cite",
        None,
        true,
        "<id>:<quote>",
        "Exact quote from an idea (repeatable, at least one)",
    ),
    FlagSpec::string("next", None, false, "<id>", "Idea to think about next"),
    FlagSpec::string(
        "flow",
        None,
        false,
        "<fl-id>",
        "The flow session (default: $ROOTS_FLOW)",
    ),
    FlagSpec::string(
        "as",
        None,
        false,
        "agent:<model>",
        "Agent identity (default: $ROOTS_AGENT)",
    ),
];
const LIST: &[FlagSpec] = &[
    FlagSpec::string(
        "status",
        None,
        false,
        "<status>",
        "Only nodes with this status",
    ),
    FlagSpec::string("kind", None, false, "<kind>", "idea or sprout"),
    FlagSpec::boolean("orphans", None, "Only nodes with no edges"),
    FlagSpec::boolean(
        "anchors",
        None,
        "Only anchors (ideas with no outgoing serves)",
    ),
    FlagSpec::boolean(
        "all",
        Some('a'),
        "Include composted ideas and closed sprouts",
    ),
];
const LOG: &[FlagSpec] = &[FlagSpec::string(
    "limit",
    Some('n'),
    false,
    "<n>",
    "Only the most recent n events",
)];
const VIEW: &[FlagSpec] = &[
    FlagSpec::string("from", None, false, "<id>", "Start from this idea"),
    FlagSpec::boolean("sprouts", None, "Append \"Agent proposals (not accepted)\""),
    FlagSpec::boolean("html", None, "A static page with a graph and timelines"),
    FlagSpec::string("out", None, false, "<file>", "Write to a file, not stdout"),
];

macro_rules! implemented_command {
    ($name:literal, $group:ident, $summary:literal, $usage:literal, $description:expr, $flags:ident, $min:expr, $max:expr, $run:path) => {
        CommandDef {
            name: $name,
            group: CommandGroup::$group,
            summary: $summary,
            usage: $usage,
            description: $description,
            flags: $flags,
            min_args: $min,
            max_args: $max,
            run: $run,
        }
    };
}

/// All commands in their historical `register-all.ts` order.
pub static COMMANDS: &[CommandDef] = &[
    implemented_command!(
        "init",
        Setup,
        "Initialize .luuna/roots/ in the current directory",
        "init",
        Some(
            "Creates .luuna/roots/ (config.yaml, JSONL stores, human/ and agent/ trees) and adds\n`.luuna/roots/*.jsonl merge=union` to .gitattributes. Fails if already initialized."
        ),
        NONE,
        0,
        Some(0),
        crate::commands::init_command
    ),
    implemented_command!(
        "setup",
        Setup,
        "Install harness hooks (claude): prime on start, guard on edits",
        "setup claude [--project|--user] [--remove] [--dry-run]",
        Some(
            "Merges roots hooks into .claude/settings.json (project, default) or\n~/.claude/settings.json (--user): SessionStart `luuna-roots prime --hook`, PreToolUse\n`luuna-roots guard` on Write|Edit|MultiEdit|NotebookEdit|Bash, and at tier 3 a Stop hook\n`luuna-roots drift --diff HEAD` (when available). Re-running is a no-op; other settings\nand hooks are kept. --remove takes out only roots hooks."
        ),
        SETUP,
        1,
        Some(1),
        crate::commands_review::setup
    ),
    implemented_command!(
        "guard",
        Setup,
        "PreToolUse hook: deny agent edits to .luuna/roots/human/ (reads hook JSON on stdin)",
        "guard",
        Some(
            "Claude Code PreToolUse hook handler. Reads {tool_name, tool_input, cwd} on stdin.\nDenies Write/Edit/MultiEdit/NotebookEdit whose target resolves (relative paths, `..`,\nsymlinks) under any .luuna/roots/human/, and, best effort, Bash commands that write there.\nAllows silently otherwise. Unparsable input is allowed with a note on stderr."
        ),
        NONE,
        0,
        Some(0),
        crate::commands_review::guard
    ),
    implemented_command!(
        "plant",
        Capture,
        "Create an idea. With no argument, opens $EDITOR",
        "plant [<statement>]",
        Some(
            "The first non-empty line of the idea is its statement: one claim, one sentence.\nRequires an interactive terminal and a human name (ROOTS_USER or git user.name)."
        ),
        PLANT,
        0,
        None,
        crate::commands_capture::plant_command
    ),
    implemented_command!(
        "think",
        Capture,
        "Run a think session. With no id, picks from the queue",
        "think [<id>]",
        Some(crate::commands_think::THINK_DESCRIPTION),
        THINK,
        0,
        Some(1),
        crate::commands_think::think_command
    ),
    implemented_command!(
        "flow",
        Capture,
        "Think, review and plant in one session; the agent works in the background",
        "flow [<id>]",
        Some(crate::commands_flow::FLOW_DESCRIPTION),
        THINK,
        0,
        Some(1),
        crate::commands_flow::flow_command
    ),
    implemented_command!(
        "adopt",
        Capture,
        "Create a new idea from a sprout (rewrite it, or --endorse it with your thoughts)",
        "adopt <sprout-id>",
        Some(
            "Opens $EDITOR on an empty idea.md with the sprout quoted below a scissors\nline for reference; everything from that line down is removed on save (also\nshown in a split pane under tmux/zellij). Write the idea in your own words;\nnothing is copied. Saving nothing above the line cancels.\n--endorse keeps the sprout as the agent's text (pinned by hash, quoted and\nlabeled in view/show/prime/context); you write only your thoughts on it.\nRecords `<new idea> derives <sprout>` and marks the sprout adopted. Needs an\ninteractive terminal and a human name (ROOTS_USER or git user.name)."
        ),
        ADOPT,
        1,
        Some(1),
        crate::commands_capture::adopt_command
    ),
    implemented_command!(
        "mv",
        Capture,
        "Rename an idea or sprout (the id is stable)",
        "mv <id> <new-slug>",
        Some("Renames the directory and updates the slug. Edges reference ids, so no link breaks."),
        NONE,
        2,
        Some(2),
        crate::commands::mv_command
    ),
    implemented_command!(
        "tend",
        Structure,
        "Interactive review: proposals, sprouts, expiring items",
        "tend",
        Some(crate::commands_tend::TEND_DESCRIPTION),
        TEND,
        0,
        Some(0),
        crate::commands_tend::tend_command
    ),
    implemented_command!(
        "accept",
        Structure,
        "Accept a proposal (s-: adopt a sprout)",
        "accept <p-id|s-id>",
        Some(
            "edge: creates the edge. A mention proposal names no relation: pass --rel.\nmerge: pass --keep <id> (the survivor replaces the other idea, which is composted).\nsplit: marks it accepted; then split the idea yourself in `luuna-roots think <id>`.\ncompost: composts the idea. Pending edge proposals on the same pair are decided\ntogether (one `tend` card).\nsprout (s-): accepting means adopting: same as `luuna-roots adopt <s-id>` (needs a TTY;\nyou write the idea yourself in $EDITOR)."
        ),
        ACCEPT,
        1,
        Some(1),
        crate::commands_review::accept
    ),
    implemented_command!(
        "reject",
        Structure,
        "Reject a proposal or sprout; remembered permanently",
        "reject <p-id|s-id> [--reason <t>]",
        Some(
            "Agents can never file the same proposal (or sprout statement) again. The reason\ngoes into their context packet, so say what was wrong."
        ),
        REJECT,
        1,
        Some(1),
        crate::commands_review::reject
    ),
    implemented_command!(
        "link",
        Structure,
        "Add an edge directly",
        "link <a> <b> <rel>",
        Some(
            "Relations: serves (a is a means to b), tension (a and b pull against each other),\nreplaces (a supersedes b; b is composted). serves and replaces stay acyclic;\ntension is stored once per pair. `derives` comes only from `luuna-roots adopt`."
        ),
        NONE,
        3,
        Some(3),
        crate::commands::link_command
    ),
    implemented_command!(
        "scan",
        Structure,
        "Find mentions in edits made outside a session",
        "scan [<id>]",
        Some(
            "Scans ideas whose idea.md changed since its last recorded hash (plant, think\nsession end, or a previous scan) and files link proposals for mentions of other\nideas (slug, id, or a distinctive phrase from their statement)."
        ),
        SCAN,
        0,
        Some(1),
        crate::commands_review::scan
    ),
    implemented_command!(
        "unlink",
        Structure,
        "Remove an edge",
        "unlink <edge-id>",
        Some(
            "Edge ids are listed by `luuna-roots show <id>`. Unlinking a `replaces` edge does not\nrevive the replaced idea. `derives` edges (adoption lineage) stay."
        ),
        NONE,
        1,
        Some(1),
        crate::commands::unlink_command
    ),
    implemented_command!(
        "commit",
        Structure,
        "Set status to committed",
        "commit <id>",
        Some(
            "You stand behind this idea. Only a shaping idea (thought about at least once) commits."
        ),
        NONE,
        1,
        Some(1),
        crate::commands::commit_command
    ),
    implemented_command!(
        "status",
        Structure,
        "Change an idea's status",
        "status <id> <status>",
        Some(
            "planted → shaping → committed → built; any live idea → composted.\nshaping is set by the first `luuna-roots think` session; composted ideas stay composted."
        ),
        NONE,
        2,
        Some(2),
        crate::commands::status_command
    ),
    implemented_command!(
        "compost",
        Structure,
        "Retire an idea",
        "compost <id> [--reason <t>]",
        None,
        COMPOST,
        1,
        Some(1),
        crate::commands::compost_command
    ),
    implemented_command!(
        "tier",
        Structure,
        "Per-idea agent tier override",
        "tier <id> [<0-3>|default]",
        Some(
            "Tiers: 0 off, 1 ask, 2 propose, 3 observe. `luuna-roots tier <id> 0` keeps agents out of an\nidea you want to think about alone; `default` removes the override so the idea\nfollows `tier:` in .luuna/roots/config.yaml. With no tier, prints the effective tier."
        ),
        NONE,
        1,
        Some(2),
        crate::commands::tier_command
    ),
    implemented_command!(
        "context",
        Agent,
        "Context packet for one idea (prose, neighbors, history)",
        "context <id>",
        Some(
            "Read-only. Prints the idea's prose, status, neighbors, prior questions (dismissed\nones included, so agents learn what not to ask), rejected proposals, agent notes,\nlinked Missions issues and Atlas expertise, the effective agent tier and limits, and\nrecent repo state."
        ),
        CONTEXT,
        1,
        Some(1),
        crate::commands_read::context_command
    ),
    implemented_command!(
        "prime",
        Agent,
        "Accepted graph as agent context (anchors → committed ideas)",
        "prime [--scope <id>]",
        Some(
            "Prints anchors (top-level goals) with the ideas that serve them, tensions, the agent\ntier and a short command guide. Sprouts, proposals and composted ideas are left out.\n--scope limits the tree to one idea, what serves it, and what it serves."
        ),
        PRIME,
        0,
        Some(0),
        crate::commands_read::prime_command
    ),
    implemented_command!(
        "drift",
        Agent,
        "Ideas a repo change touches (tier 3; the Stop hook)",
        "drift [--diff <rev>]",
        Some(
            "Read-only. Lists live ideas touched by changes since <rev> (default HEAD: uncommitted\nand untracked files): slug words in a changed path, idea/notes or a linked seeds issue\nnaming a changed file, or a commit in <rev>..HEAD citing the r- id; plus the ideas they\nserve. The agent may then `luuna-roots ask <id>` whether the idea still holds.\nHook-safe: never exits non-zero for git/tier/project reasons; below project tier 3\n(or with nothing touched) it prints nothing. --json always explains (`skipped`)."
        ),
        DRIFT,
        0,
        Some(0),
        crate::commands_integrity::drift_command
    ),
    implemented_command!(
        "ask",
        Agent,
        "Queue a question about an idea (tier ≥ 1)",
        "ask <id> <question>",
        Some(
            "The question shows up in the human's next `luuna-roots think` session on the idea (or the\ncurrent one). One line, ending with \"?\", specific to this idea. Refused when the tier\nis below 1, when the idea already has limits.questionsPerSession open agent\nquestions, or when the same question is open or was dismissed."
        ),
        ASK,
        2,
        None,
        crate::commands_agent::ask_command
    ),
    implemented_command!(
        "note",
        Agent,
        "Attach an artifact to an idea under agent/notes (tier ≥ 1)",
        "note <id> --file <path>",
        Some(
            "Copies the file (research, a diagram, HTML, an image) to\n.luuna/roots/agent/notes/<hex>/YYYY-MM-DD-<name>.<ext>. The human's idea.md is never\ntouched. Limit: 1 MiB per file. Refused below tier 1 (per-idea override, else\nconfig). `luuna-roots show` and `luuna-roots context` list an idea's notes.\n--question <q-id>: findings on a question the human handed to the agent ([a] in\nthink). Text only (.md/.txt); its first paragraph is shown under the question when\nit comes back to the human, who still answers it in idea.md."
        ),
        NOTE,
        1,
        Some(1),
        crate::commands_agent::note_command
    ),
    implemented_command!(
        "propose",
        Agent,
        "Propose an edge, split, merge or compost (tier ≥ 2)",
        "propose edge|split|merge|compost ...",
        Some(
            "  luuna-roots propose edge <a> <b> <serves|tension|replaces> --reason <t> --cite <id>:<quote> ...\n  luuna-roots propose split <id> --reason <how it splits> --cite <id>:<quote>\n  luuna-roots propose merge <a> <b> --reason <t> --cite <a>:<quote> --cite <b>:<quote>\n  luuna-roots propose compost <id> --reason <t> --cite <id>:<quote> ...\n\nEvery quote must be an exact substring of the cited idea's current idea.md (the\ntext after the FIRST colon; later colons are part of the quote). Edges and merges\ncite both ideas; splits and composts cite the idea. Nothing is linked until a human\naccepts it in `luuna-roots tend`. Refused below tier 2, at limits.proposals pending, or when\na human already rejected the same proposal (rejections are permanent).\n--mention: cite only the line that mentions the other idea; roots adds its statement."
        ),
        PROPOSE,
        1,
        Some(4),
        crate::commands_agent::propose_command
    ),
    implemented_command!(
        "sprout",
        Agent,
        "Propose a new idea in the agent tree (tier ≥ 2)",
        "sprout <statement> [--file <md>]",
        Some(
            "The statement is one claim in one line. --file adds prose after it (Markdown).\nA sprout never becomes an idea: a human may adopt it (they rewrite it in their own\nwords) or reject it, in `luuna-roots tend`. Refused below tier 2, at limits.sprouts open\nsprouts, or when the statement matches an idea, an open or adopted sprout, or a\nrejected one (rejections are permanent). Open sprouts expire after\nlimits.sproutTtlDays."
        ),
        SPROUT,
        1,
        None,
        crate::commands_agent::sprout_command
    ),
    implemented_command!(
        "heading",
        Agent,
        "Say where a flow session is going (tier ≥ 1)",
        "heading <text> --cite <id>:<quote> ... [--next <id>]",
        Some(
            "Shown in the human's `luuna-roots flow` pane, labeled [agent]; never in views. At most two\nsentences and 280 characters. Cite every idea the text names with an exact quote\nfrom its idea.md. --next suggests the idea to think about next. Refused outside a\nrunning flow ($ROOTS_FLOW or --flow) and for text a human dismissed before."
        ),
        HEADING,
        1,
        None,
        crate::commands_agent::heading_command
    ),
    implemented_command!(
        "show",
        Read,
        "Show an idea or sprout with edges, questions and history",
        "show <id>",
        Some(
            "<id> may be r-a1b2, a1b2, a slug, or a unique prefix of any of them.\nFor ideas, also lists linked Missions issues (.luuna/missions/issues.jsonl: an `intent` field, else an\nr- id in the title/description/labels) and Atlas records that cite the id (read-only)."
        ),
        NONE,
        1,
        Some(1),
        crate::commands_read::show_command
    ),
    implemented_command!(
        "blame",
        Read,
        "Show which question each line of an idea answered",
        "blame <id>",
        Some(
            "Prints idea.md with the id of the question each line answered in the margin, then\nthe questions. A think save that answers a question records a hash of each line it\nadded; blame matches the current lines against them. A line edited later shows no id\n(or the id of a later answer that rewrote it). idea.md is never changed."
        ),
        NONE,
        1,
        Some(1),
        crate::commands_read::blame_command
    ),
    implemented_command!(
        "list",
        Read,
        "List ideas and sprouts",
        "list [--status <s>] [--kind idea|sprout] [--orphans] [--anchors]",
        Some(
            "By default composted ideas and closed sprouts are hidden; use --all or --status.\n--orphans: no edges at all. --anchors: live ideas with no outgoing `serves`."
        ),
        LIST,
        0,
        Some(0),
        crate::commands::list_command
    ),
    implemented_command!(
        "queue",
        Read,
        "What needs attention: open questions, pending proposals, sprouts",
        "queue",
        Some(
            "Think next: ideas with open questions or questions the rules would ask, in the\norder `luuna-roots think` picks them. Then open questions, pending proposals and open\nsprouts (soonest expiry first). With seeds in the repo, committed ideas whose linked\nissues are all closed come first under \"Ready to mark built\" (roots never changes\nstatus itself)."
        ),
        NONE,
        0,
        Some(0),
        crate::commands_read::queue_command
    ),
    implemented_command!(
        "log",
        Read,
        "Event history (all, or for one node)",
        "log [<id>]",
        None,
        LOG,
        0,
        Some(1),
        crate::commands::log_command
    ),
    implemented_command!(
        "view",
        Read,
        "Compiled culmination: anchors, what serves them, tensions, questions",
        "view [--from <id>] [--sprouts] [--html] [--out <file>]",
        Some(
            "Anchors (committed/shaping ideas that serve nothing) with the ideas that serve them,\ndepth-first; an idea serving several anchors is printed once, then referenced. Then\ntensions and open questions. Composted ideas and sprouts are left out; planted ideas\nnot under an anchor are listed in one line each. Only human prose and human-accepted\nstructure. --sprouts adds a separate [agent] section. With view.write: true in\nconfig.yaml, ROOTS.md at the repo root is refreshed with the default view."
        ),
        VIEW,
        0,
        Some(0),
        crate::commands_read::view_command
    ),
    implemented_command!(
        "verify",
        Read,
        "Check the human/agent boundary + graph invariants",
        "verify",
        Some(
            "Checks: files under .luuna/roots/human/ that are not an idea's idea.md or assets/, symlinks\nleaving human/, idea.md edits outside a think session and not in a human commit (hash\nledger), authors (ideas human:*, sprouts agent:*, edges human:*, proposals agent:*/roots),\ngraph invariants, dangling references, missing/orphan directories, malformed JSONL\nlines, events without `by`; warns when a built idea has linked seeds issues still open.\nExits 6 on errors; warnings are listed but pass."
        ),
        NONE,
        0,
        Some(0),
        crate::commands_integrity::verify_command
    ),
];

pub fn find_command(name: &str) -> Option<&'static CommandDef> {
    COMMANDS.iter().find(|command| command.name == name)
}

pub fn flags_for(command: &CommandDef) -> Vec<FlagSpec> {
    GLOBAL_FLAGS
        .iter()
        .chain(command.flags.iter())
        .copied()
        .collect()
}

fn flag_label(spec: &FlagSpec) -> String {
    let short = spec
        .short
        .map(|short| format!("-{short}, "))
        .unwrap_or_else(|| "    ".to_owned());
    let value = if spec.kind == FlagKind::String {
        format!(" {}", spec.placeholder.unwrap_or("<value>"))
    } else {
        String::new()
    };
    format!("{short}--{}{}", spec.name, value)
}

fn flag_lines(specs: &[FlagSpec], colors: Colors) -> Vec<String> {
    let width = specs
        .iter()
        .map(|spec| flag_label(spec).len())
        .max()
        .unwrap_or(0)
        + 2;
    specs
        .iter()
        .map(|spec| {
            let label = flag_label(spec);
            format!(
                "  {}{}",
                pad_end(&colors.cyan(&label), width),
                spec.description
            )
        })
        .collect()
}

pub fn render_command_help(command: &CommandDef, colors: Colors) -> String {
    let mut lines = vec![
        format!("{} luuna-roots {}", colors.bold("Usage:"), command.usage),
        String::new(),
        command.summary.to_owned(),
    ];
    if let Some(description) = command.description {
        lines.push(String::new());
        lines.push(description.to_owned());
    }
    if !command.flags.is_empty() {
        lines.push(String::new());
        lines.push(colors.bold("Options:"));
        lines.extend(flag_lines(command.flags, colors));
    }
    lines.push(String::new());
    lines.push(colors.bold("Global options:"));
    lines.extend(flag_lines(GLOBAL_FLAGS, colors));
    format!("{}\n", lines.join("\n"))
}

pub fn render_root_help(definitions: &[CommandDef], version: &str, colors: Colors) -> String {
    let width = definitions
        .iter()
        .map(|command| command.usage.len())
        .max()
        .unwrap_or(0)
        .saturating_add(2)
        .min(30);
    let mut lines = vec![
        format!(
            "{} {}: human-owned intent for Luuna projects",
            colors.bold("luuna-roots"),
            colors.dim(&format!("v{version}"))
        ),
        String::new(),
        format!("{} luuna-roots <command> [options]", colors.bold("Usage:")),
    ];
    for group in GROUPS {
        let commands: Vec<&CommandDef> = definitions
            .iter()
            .filter(|command| command.group == group)
            .collect();
        if commands.is_empty() {
            continue;
        }
        lines.push(String::new());
        lines.push(colors.bold(group.title()));
        for command in commands {
            if command.usage.len() + 2 <= width {
                lines.push(format!(
                    "  {}{}",
                    pad_end(&colors.cyan(command.usage), width),
                    command.summary
                ));
            } else {
                lines.push(format!("  {}", colors.cyan(command.usage)));
                lines.push(format!("  {}{}", " ".repeat(width), command.summary));
            }
        }
    }
    let mut global = GLOBAL_FLAGS.to_vec();
    global.push(VERSION_FLAG);
    lines.push(String::new());
    lines.push(colors.bold("Global options:"));
    lines.extend(flag_lines(&global, colors));
    lines.push(String::new());
    lines.push(
        "Run `luuna-roots <command> --help` for details. Every command accepts --json.".to_owned(),
    );
    format!("{}\n", lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_registry_and_help_keep_the_public_contract() {
        assert_eq!(COMMANDS.len(), 33);
        assert_eq!(COMMANDS.first().unwrap().name, "init");
        assert_eq!(COMMANDS.last().unwrap().name, "verify");
        let help = render_command_help(find_command("list").unwrap(), Colors::new(false));
        assert!(help.contains("Usage: luuna-roots list"));
        assert!(help.contains("-a, --all"));
        assert!(help.contains("-q, --quiet"));
        let root = render_root_help(COMMANDS, "0.0.0", Colors::new(false));
        assert!(root.contains("Capture and think (human)"));
        assert!(root.contains("propose edge|split|merge|compost ..."));
    }
}
