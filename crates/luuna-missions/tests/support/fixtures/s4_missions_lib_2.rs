//! The fixture groups of s4-missions-lib-2, the plan that owns this file.
//!
//! s4-missions-lib-2 adds a [`Group`] here for each fixture it moves into
//! scope, using only files from its own list in [`super::DEFERRED`], and
//! sets [`IN_SCOPE_COUNT`] to the number of files in its groups. Each group
//! is replayed by a `parity_*` wrapper in
//! `tests/later_plans/s4_missions_lib_2.rs`.

use super::Group;

/// The fixtures s4-missions-lib-2 has moved into scope, by parity test.
pub const IN_SCOPE: &[Group] = &[
    Group {
        test: "parity_dispatch",
        command: "",
        files: &[
            "001.json", "002.json", "003.json", "004.json", "005.json", "007.json", "008.json",
            "010.json",
        ],
    },
    Group {
        test: "parity_leading_format",
        command: "json",
        files: &["006.json", "009.json", "012.json"],
    },
    Group {
        test: "parity_version_after_command",
        command: "list",
        files: &["014.json"],
    },
    Group {
        test: "parity_unknown_command",
        command: "creat",
        files: &["038.json"],
    },
    Group {
        test: "parity_ready",
        command: "ready",
        files: &["025.json", "033.json", "044.json", "048.json", "067.json"],
    },
    Group {
        test: "parity_blocked",
        command: "blocked",
        files: &["026.json", "068.json", "087.json"],
    },
    Group {
        test: "parity_stats",
        command: "stats",
        files: &["034.json", "050.json", "069.json", "080.json"],
    },
    Group {
        test: "parity_close",
        command: "close",
        files: &["032.json", "075.json", "093.json"],
    },
    Group {
        test: "parity_dep",
        command: "dep",
        files: &[
            "024.json", "084.json", "085.json", "086.json", "088.json", "095.json",
        ],
    },
    Group {
        test: "parity_label",
        command: "label",
        files: &[
            "031.json", "096.json", "097.json", "098.json", "099.json", "101.json", "102.json",
            "103.json",
        ],
    },
    Group {
        test: "parity_unblock",
        command: "unblock",
        files: &["090.json", "092.json", "094.json"],
    },
    Group {
        test: "parity_completions",
        command: "completions",
        files: &["037.json", "150.json", "151.json"],
    },
];

/// How many fixtures s4-missions-lib-2 has moved into scope.
pub const IN_SCOPE_COUNT: usize = 48;
