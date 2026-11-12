//! The parity tests of s4-missions-lib-3, the plan that owns this file.
//!
//! For each group in `tests/support/fixtures/s4_missions_lib_3.rs`, this
//! file holds one `parity_*` wrapper that calls
//! `crate::support::replay_group` with the group's name.

#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn parity_config() {
    crate::support::replay_group("parity_config");
}

#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn parity_tpl() {
    crate::support::replay_group("parity_tpl");
}

#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn parity_tpl_child_show() {
    crate::support::replay_group("parity_tpl_child_show");
}

#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn parity_setup() {
    crate::support::replay_group("parity_setup");
}

#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn parity_onboard() {
    crate::support::replay_group("parity_onboard");
}

#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn parity_guard() {
    crate::support::replay_group("parity_guard");
}

#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn parity_prime() {
    crate::support::replay_group("parity_prime");
}

#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn parity_migrate_from_beads() {
    crate::support::replay_group("parity_migrate_from_beads");
}

#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn parity_doctor() {
    crate::support::replay_group("parity_doctor");
}

#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn parity_sync() {
    crate::support::replay_group("parity_sync");
}

#[test]
#[ignore = "requires LUUNA_MISSIONS_REFERENCE and LUUNA_MISSIONS_GO"]
fn parity_upgrade() {
    crate::support::replay_group("parity_upgrade");
}
