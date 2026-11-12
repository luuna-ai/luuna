//! The allocation of the reference's 194 regression fixtures
//! (`internal/regression/testdata/*.json`) to plans, by literal file name.
//!
//! Fixtures are read in place from the reference clone and never copied.
//! [`IN_SCOPE`] holds the foundation's groups, and [`DEFERRED`] is each later
//! plan's allocation: the fixtures that s4-missions-lib-2, -3 or -4 makes
//! pass. Fixtures 124 (`tpl step add --help`) and 135
//! (`setup claude --help`) are answered by s4-missions-lib-2's command-help
//! hook and 130 runs -2's `close`, so they belong to -4, which is built after
//! -2 and -3.
//!
//! This file is frozen for the later plans. Each later plan moves fixtures
//! from its own deferred list into scope only in its own slot file,
//! `tests/support/fixtures/<plan>.rs`, listed in [`LATER_PLAN_GROUPS`];
//! nothing is removed or loosened.

use std::collections::BTreeMap;
use std::fs;

use serde::Deserialize;

use super::reference::Reference;

mod s4_missions_lib_2;
mod s4_missions_lib_3;
mod s4_missions_lib_4;

/// A group of in-scope fixtures, replayed by one parity test.
#[derive(Debug)]
pub struct Group {
    /// The parity test that replays the group.
    pub test: &'static str,
    /// The command every fixture of the group runs: the first argument that
    /// does not begin with `-`. For a later plan's group only, `""` means
    /// that no argument of the group's fixtures lacks a leading `-`.
    pub command: &'static str,
    /// The fixture files.
    pub files: &'static [&'static str],
}

/// The fixtures in scope, by parity test.
pub const IN_SCOPE: &[Group] = &[
    Group {
        test: "parity_init",
        command: "init",
        files: &[
            "013.json", "039.json", "081.json", "106.json", "118.json", "119.json", "133.json",
            "152.json",
        ],
    },
    Group {
        test: "parity_create",
        command: "create",
        files: &[
            "022.json", "023.json", "040.json", "041.json", "042.json", "082.json", "083.json",
        ],
    },
    Group {
        test: "parity_update",
        command: "update",
        files: &[
            "030.json", "043.json", "045.json", "070.json", "071.json", "072.json", "073.json",
            "074.json", "078.json",
        ],
    },
    Group {
        test: "parity_block",
        command: "block",
        files: &["089.json", "091.json"],
    },
    Group {
        test: "parity_show",
        command: "show",
        files: &[
            "028.json", "046.json", "063.json", "064.json", "077.json", "079.json",
        ],
    },
    Group {
        test: "parity_list",
        command: "list",
        files: &[
            "011.json", "027.json", "047.json", "051.json", "052.json", "053.json", "054.json",
            "055.json", "056.json", "057.json", "058.json", "059.json", "060.json", "061.json",
            "062.json", "076.json", "100.json",
        ],
    },
    Group {
        test: "parity_search",
        command: "search",
        files: &["029.json", "049.json", "065.json", "066.json"],
    },
];

/// How many fixtures are in scope.
pub const IN_SCOPE_COUNT: usize = 53;

pub const DEFERRED_TO_S4_MISSIONS_LIB_2: &[&str] = &[
    "001.json", "002.json", "003.json", "004.json", "005.json", "006.json", "007.json", "008.json",
    "009.json", "010.json", "012.json", "014.json", "024.json", "025.json", "026.json", "031.json",
    "032.json", "033.json", "034.json", "037.json", "038.json", "044.json", "048.json", "050.json",
    "067.json", "068.json", "069.json", "075.json", "080.json", "084.json", "085.json", "086.json",
    "087.json", "088.json", "090.json", "092.json", "093.json", "094.json", "095.json", "096.json",
    "097.json", "098.json", "099.json", "101.json", "102.json", "103.json", "150.json", "151.json",
];
pub const DEFERRED_TO_S4_MISSIONS_LIB_3: &[&str] = &[
    "015.json", "016.json", "017.json", "018.json", "019.json", "020.json", "021.json", "035.json",
    "036.json", "104.json", "105.json", "107.json", "108.json", "109.json", "110.json", "111.json",
    "112.json", "113.json", "114.json", "115.json", "116.json", "117.json", "120.json", "121.json",
    "122.json", "123.json", "125.json", "126.json", "127.json", "128.json", "129.json", "131.json",
    "132.json", "134.json", "136.json", "137.json", "138.json", "139.json", "140.json", "141.json",
    "142.json", "143.json", "144.json", "145.json", "146.json", "147.json", "148.json", "149.json",
];
pub const DEFERRED_TO_S4_MISSIONS_LIB_4: &[&str] = &[
    "124.json", "130.json", "135.json", "153.json", "154.json", "155.json", "156.json", "157.json",
    "158.json", "159.json", "160.json", "161.json", "162.json", "163.json", "164.json", "165.json",
    "166.json", "167.json", "168.json", "169.json", "170.json", "171.json", "172.json", "173.json",
    "174.json", "175.json", "176.json", "177.json", "178.json", "179.json", "180.json", "181.json",
    "182.json", "183.json", "184.json", "185.json", "186.json", "187.json", "188.json", "189.json",
    "190.json", "191.json", "192.json", "193.json", "194.json",
];

/// The deferred fixtures, by plan, with the expected count of each list.
pub const DEFERRED: &[(&str, &[&str], usize)] = &[
    ("s4-missions-lib-2", DEFERRED_TO_S4_MISSIONS_LIB_2, 48),
    ("s4-missions-lib-3", DEFERRED_TO_S4_MISSIONS_LIB_3, 48),
    ("s4-missions-lib-4", DEFERRED_TO_S4_MISSIONS_LIB_4, 45),
];

/// The later plans' fixture groups, by plan, with the number of fixtures
/// each plan has moved into scope. Each entry comes from the plan's own slot
/// file; the plans are those of [`DEFERRED`], in the same order.
pub const LATER_PLAN_GROUPS: &[(&str, &[Group], usize)] = &[
    (
        "s4-missions-lib-2",
        s4_missions_lib_2::IN_SCOPE,
        s4_missions_lib_2::IN_SCOPE_COUNT,
    ),
    (
        "s4-missions-lib-3",
        s4_missions_lib_3::IN_SCOPE,
        s4_missions_lib_3::IN_SCOPE_COUNT,
    ),
    (
        "s4-missions-lib-4",
        s4_missions_lib_4::IN_SCOPE,
        s4_missions_lib_4::IN_SCOPE_COUNT,
    ),
];

/// The in-scope group replayed by `test`.
///
/// It is one of the foundation's groups in [`IN_SCOPE`] or one of a later
/// plan's groups in [`LATER_PLAN_GROUPS`].
pub fn group(test: &str) -> &'static Group {
    IN_SCOPE
        .iter()
        .chain(
            LATER_PLAN_GROUPS
                .iter()
                .flat_map(|(_, groups, _)| groups.iter()),
        )
        .find(|group| group.test == test)
        .unwrap_or_else(|| panic!("no fixture group for {test}"))
}

/// One regression case, as `internal/regression/compatibility_test.go`
/// decodes it.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fixture {
    /// The case name.
    pub name: String,
    /// The arguments after the program name.
    pub args: Vec<String>,
    /// Stdin.
    pub input: String,
    /// The capture root, replaced by the real project root.
    pub root: String,
    /// The project files before the run.
    pub before: BTreeMap<String, String>,
    /// The expected streams and exit code.
    pub result: Expected,
    /// The project files after the run.
    pub after: BTreeMap<String, String>,
    /// Captured ids by label.
    pub ids: BTreeMap<String, String>,
    /// Environment; only `PATH` is honoured.
    pub env: BTreeMap<String, String>,
}

/// The expected streams and exit code of a case.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expected {
    /// Expected stdout.
    pub stdout: String,
    /// Expected stderr.
    pub stderr: String,
    /// Expected exit code.
    pub exit: i32,
}

/// Every `*.json` file name in the reference's fixture folder, sorted.
pub fn testdata_files(reference: &Reference) -> Vec<String> {
    let dir = reference.testdata();
    let mut names: Vec<String> = fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("read {}: {error}", dir.display()))
        .map(|entry| entry.expect("directory entry"))
        .filter(|entry| entry.path().is_file())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".json"))
        .collect();
    names.sort();
    names
}

/// Reads and decodes one fixture from the reference.
pub fn load(reference: &Reference, file: &str) -> Fixture {
    let path = reference.testdata().join(file);
    let data = fs::read(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_slice(&data)
        .unwrap_or_else(|error| panic!("decode {}: {error}", path.display()))
}
