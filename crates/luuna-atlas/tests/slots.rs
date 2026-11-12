//! Tests of the harness slot `Step::Command`. They need no Go and no reference clone.

mod support;

use luuna_atlas::commands;
use support::project::{Sandbox, Step};

#[test]
fn slots_step_command_routes_to_the_given_function() {
    let sandbox = Sandbox::without_go();
    let project = sandbox.project("rust", false);
    let step = Step::command(&["config", "show"], |dir, _| commands::run_config_show(dir));
    assert_eq!(project.rust(&step), commands::run_config_show(&project.dir));
}

#[test]
fn slots_step_command_label_is_the_go_args() {
    let step = Step::command(&["config", "show"], |dir, _| commands::run_config_show(dir));
    assert_eq!(step.label(), "config show");
}
