//! The fixture groups of s4-missions-lib-3, the plan that owns this file.
//!
//! s4-missions-lib-3 adds a [`Group`] here for each fixture it moves into
//! scope, using only files from its own list in [`super::DEFERRED`], and
//! sets [`IN_SCOPE_COUNT`] to the number of files in its groups. Each group
//! is replayed by a `parity_*` wrapper in
//! `tests/later_plans/s4_missions_lib_3.rs`.

use super::Group;

/// The fixtures s4-missions-lib-3 has moved into scope, by parity test.
pub const IN_SCOPE: &[Group] = &[
    Group {
        test: "parity_config",
        command: "config",
        files: &[
            "015.json", "016.json", "021.json", "104.json", "105.json", "107.json", "108.json",
            "109.json", "110.json", "111.json", "112.json", "113.json", "114.json", "115.json",
            "116.json", "117.json",
        ],
    },
    Group {
        test: "parity_tpl",
        command: "tpl",
        files: &[
            "017.json", "018.json", "019.json", "020.json", "120.json", "121.json", "122.json",
            "123.json", "125.json", "126.json", "127.json", "128.json", "131.json", "132.json",
        ],
    },
    Group {
        test: "parity_tpl_child_show",
        command: "show",
        files: &["129.json"],
    },
    Group {
        test: "parity_setup",
        command: "setup",
        files: &["134.json", "136.json", "137.json"],
    },
    Group {
        test: "parity_onboard",
        command: "onboard",
        files: &["143.json", "144.json", "145.json", "146.json"],
    },
    Group {
        test: "parity_guard",
        command: "guard",
        files: &["138.json", "139.json", "140.json"],
    },
    Group {
        test: "parity_prime",
        command: "prime",
        files: &["141.json", "142.json"],
    },
    Group {
        test: "parity_migrate_from_beads",
        command: "migrate-from-beads",
        files: &["147.json", "148.json"],
    },
    Group {
        test: "parity_doctor",
        command: "doctor",
        files: &["035.json"],
    },
    Group {
        test: "parity_sync",
        command: "sync",
        files: &["036.json"],
    },
    Group {
        test: "parity_upgrade",
        command: "upgrade",
        files: &["149.json"],
    },
];

/// How many fixtures s4-missions-lib-3 has moved into scope.
pub const IN_SCOPE_COUNT: usize = 48;
