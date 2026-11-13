//! Frozen fixture allocation for s4-missions-lib-4.
use super::Group;
pub const IN_SCOPE: &[Group] = &[
    Group {
        test: "parity_plan_ticket_tpl",
        command: "tpl",
        files: &["124.json"],
    },
    Group {
        test: "parity_plan_ticket_close",
        command: "close",
        files: &["130.json"],
    },
    Group {
        test: "parity_plan_ticket_setup",
        command: "setup",
        files: &["135.json"],
    },
    Group {
        test: "parity_plan_ticket_create",
        command: "create",
        files: &[
            "153.json", "167.json", "176.json", "187.json", "190.json", "193.json",
        ],
    },
    Group {
        test: "parity_plan_templates",
        command: "plan",
        files: &["154.json"],
    },
    Group {
        test: "parity_plan_prompt",
        command: "plan",
        files: &["155.json", "188.json", "192.json"],
    },
    Group {
        test: "parity_plan_submit",
        command: "plan",
        files: &["156.json", "186.json", "189.json", "191.json", "194.json"],
    },
    Group {
        test: "parity_plan_show",
        command: "plan",
        files: &["157.json", "185.json"],
    },
    Group {
        test: "parity_plan_list",
        command: "plan",
        files: &["158.json", "175.json"],
    },
    Group {
        test: "parity_plan_validate",
        command: "plan",
        files: &["159.json", "164.json"],
    },
    Group {
        test: "parity_plan_edit",
        command: "plan",
        files: &["160.json", "161.json", "162.json", "165.json", "166.json"],
    },
    Group {
        test: "parity_plan_ticket_show",
        command: "show",
        files: &["163.json"],
    },
    Group {
        test: "parity_plan_adopt",
        command: "plan",
        files: &["168.json", "184.json"],
    },
    Group {
        test: "parity_plan_reorder",
        command: "plan",
        files: &["169.json", "170.json"],
    },
    Group {
        test: "parity_plan_release",
        command: "plan",
        files: &["171.json", "172.json"],
    },
    Group {
        test: "parity_plan_review",
        command: "plan",
        files: &["173.json"],
    },
    Group {
        test: "parity_plan_outcome",
        command: "plan",
        files: &["174.json"],
    },
    Group {
        test: "parity_plan_create",
        command: "plan",
        files: &["177.json"],
    },
    Group {
        test: "parity_plan_ticket_list",
        command: "list",
        files: &["178.json"],
    },
    Group {
        test: "parity_plan_ticket_ready",
        command: "ready",
        files: &["179.json"],
    },
    Group {
        test: "parity_plan_ticket_search",
        command: "search",
        files: &["180.json"],
    },
    Group {
        test: "parity_plan_ticket_block",
        command: "block",
        files: &["181.json"],
    },
    Group {
        test: "parity_plan_ticket_blocked",
        command: "blocked",
        files: &["182.json"],
    },
    Group {
        test: "parity_plan_ticket_unblock",
        command: "unblock",
        files: &["183.json"],
    },
];
pub const IN_SCOPE_COUNT: usize = 45;
