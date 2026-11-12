#!/bin/sh
# Upgrade keeps exactly one previous, failed upgrades leave current intact, rollback swaps.
# The path of lib.sh is only known at run time (TESTS_DIR), so shellcheck cannot follow it.
# SC2012: ls lists the version directories the test created. SC2016: the shim body is literal text that runs later.
# shellcheck disable=SC1091,SC2012,SC2016
. "$TESTS_DIR/lib.sh"

install_v() { # VERSION [KIND...]
  _v=$1; shift
  make_fixture_release "$CASE_DIR/rel-$_v" "$_v" "$@"
  run_cmd "$SH" "$INSTALL" --manifest "$CASE_DIR/rel-$_v/manifest.tsv" --asset-dir "$CASE_DIR/rel-$_v/assets"
}
current() { cat "$LUUNA_INSTALL_HOME/current"; }
previous() { cat "$LUUNA_INSTALL_HOME/previous" 2>/dev/null || echo none; }
versions() { ls "$LUUNA_INSTALL_HOME/versions" | tr '\n' ' '; }

t_upgrade() {
  install_v 1.0.0; assert_rc 0
  install_v 2.0.0; assert_rc 0
  assert_eq "$(current)" 2.0.0; assert_eq "$(previous)" 1.0.0
  run_cmd lu; assert_out_contains "stub:luuna-tui:2.0.0"
  install_v 3.0.0; assert_rc 0
  assert_eq "$(current)" 3.0.0; assert_eq "$(previous)" 2.0.0
  assert_eq "$(versions)" "2.0.0 3.0.0 "
  assert_file_contains "$LUUNA_INSTALL_HOME/receipt.tsv" "previous${TAB}2.0.0"
}

t_failed_upgrade_keeps_current() {
  install_v 1.0.0; assert_rc 0
  install_v 2.0.0; assert_rc 0
  take_snapshot home "$HOME"
  install_v 3.0.0 corrupt; assert_rc 4
  assert_unchanged home "$HOME"
  install_v 3.0.0 symlink; assert_rc 5
  assert_unchanged home "$HOME"
  assert_eq "$(current)" 2.0.0; assert_eq "$(previous)" 1.0.0
  run_cmd luuna-runtime; assert_rc 0; assert_out_contains "stub:luuna-runtime:2.0.0"
}

t_rollback_twice() {
  install_v 1.0.0; assert_rc 0
  install_v 2.0.0; assert_rc 0
  run_cmd "$SH" "$INSTALL" --rollback; assert_rc 0
  assert_eq "$(current)" 1.0.0; assert_eq "$(previous)" 2.0.0
  run_cmd lu x; assert_out_contains "stub:luuna-tui:1.0.0"
  run_cmd luuna x; assert_out_contains "stub:luuna-tui:1.0.0"
  run_cmd luuna-runtime x; assert_out_contains "stub:luuna-runtime:1.0.0"
  run_cmd "$SH" "$INSTALL" --rollback; assert_rc 0
  assert_eq "$(current)" 2.0.0; assert_eq "$(previous)" 1.0.0
  run_cmd lu x; assert_out_contains "stub:luuna-tui:2.0.0"
}

t_rollback_without_previous() {
  install_v 1.0.0; assert_rc 0
  take_snapshot home "$HOME"
  run_cmd "$SH" "$INSTALL" --rollback; assert_rc 2; assert_out_contains "no previous version"
  assert_unchanged home "$HOME"
}
t_rollback_nothing_installed() {
  take_snapshot home "$HOME"
  run_cmd "$SH" "$INSTALL" --rollback; assert_rc 2
  assert_unchanged home "$HOME"
}
t_rollback_dry_run() {
  install_v 1.0.0; assert_rc 0; install_v 2.0.0; assert_rc 0
  take_snapshot home "$HOME"
  run_cmd "$SH" "$INSTALL" --rollback --dry-run; assert_rc 0; assert_out_contains "would roll back"
  assert_unchanged home "$HOME"
}
t_rollback_conflict() {
  install_v 1.0.0; assert_rc 0; install_v 2.0.0; assert_rc 0
  rm "$LUUNA_INSTALL_BIN/lu"; echo mine > "$LUUNA_INSTALL_BIN/lu"
  take_snapshot home "$HOME"
  run_cmd "$SH" "$INSTALL" --rollback; assert_rc 6
  assert_unchanged home "$HOME"
}
t_reinstall_previous() {
  install_v 1.0.0; assert_rc 0; install_v 2.0.0; assert_rc 0
  install_v 1.0.0; assert_rc 0
  assert_eq "$(current)" 1.0.0; assert_eq "$(previous)" 2.0.0
  run_cmd lu; assert_out_contains "stub:luuna-tui:1.0.0"
}

# install_with SHIM_DIR VERSION: install VERSION with SHIM_DIR prepended to the installer's PATH only.
install_with() {
  make_fixture_release "$CASE_DIR/rel-$2" "$2"
  run_cmd env "PATH=$1:$PATH" "$SH" "$INSTALL" --manifest "$CASE_DIR/rel-$2/manifest.tsv" --asset-dir "$CASE_DIR/rel-$2/assets"
}
# The last public name relinked is luuna-runtime, so a failure there leaves luuna and lu already moved.
LAST_LINK_GLOB='*/.luuna-install.tmp.*.luuna-runtime'

t_rollback_bin_mismatch() {
  install_v 1.0.0; assert_rc 0; install_v 2.0.0; assert_rc 0
  take_snapshot home "$HOME"
  run_cmd env "LUUNA_INSTALL_BIN=$HOME/other-bin" "$SH" "$INSTALL" --rollback
  assert_rc 6; assert_out_contains "rerun with"
  assert_unchanged home "$HOME"
  run_cmd env "LUUNA_INSTALL_BIN=$HOME/other-bin" "$SH" "$INSTALL" --rollback --dry-run
  assert_rc 6
  assert_unchanged home "$HOME"
}

t_upgrade_relink_failure_restores() {
  install_v 1.0.0; assert_rc 0; install_v 2.0.0; assert_rc 0
  make_shim "$CASE_DIR/shim" ln "$LAST_LINK_GLOB" 'exit 1'
  take_snapshot home "$HOME"
  install_with "$CASE_DIR/shim" 3.0.0
  assert_rc 6; assert_out_contains "restored the previous installation"
  assert_exists "$CASE_DIR/fault.ln.fired"
  assert_unchanged home "$HOME"
  run_cmd luuna-runtime; assert_out_contains "stub:luuna-runtime:2.0.0"
  run_cmd lu; assert_out_contains "stub:luuna-tui:2.0.0"
  run_cmd luuna; assert_out_contains "stub:luuna-tui:2.0.0"
  assert_eq "$(versions)" "1.0.0 2.0.0 "
  install_v 3.0.0; assert_rc 0
  assert_eq "$(current)" 3.0.0; assert_eq "$(previous)" 2.0.0
}

t_reinstall_previous_relink_failure_restores_old_dir() {
  install_v 1.0.0; assert_rc 0; install_v 2.0.0; assert_rc 0
  make_shim "$CASE_DIR/shim" ln "$LAST_LINK_GLOB" 'exit 1'
  take_snapshot home "$HOME"
  install_with "$CASE_DIR/shim" 1.0.0
  assert_rc 6; assert_out_contains "restored the previous installation"
  assert_unchanged home "$HOME"
}

t_receipt_write_failure_restores() {
  install_v 1.0.0; assert_rc 0; install_v 2.0.0; assert_rc 0
  make_shim "$CASE_DIR/shim" mv '*/luuna-install/receipt.tsv' 'exit 1'
  take_snapshot home "$HOME"
  install_with "$CASE_DIR/shim" 3.0.0
  assert_rc_nonzero; assert_out_contains "restored the previous installation"
  assert_exists "$CASE_DIR/fault.mv.fired"
  assert_unchanged home "$HOME"
}

t_pointer_write_failure_restores() {
  install_v 1.0.0; assert_rc 0; install_v 2.0.0; assert_rc 0
  make_shim "$CASE_DIR/shim" mv '*/luuna-install/current' 'exit 1'
  take_snapshot home "$HOME"
  install_with "$CASE_DIR/shim" 3.0.0
  assert_rc_nonzero; assert_out_contains "restored the previous installation"
  assert_exists "$CASE_DIR/fault.mv.fired"
  assert_unchanged home "$HOME"
}

t_rollback_relink_failure_restores() {
  install_v 1.0.0; assert_rc 0; install_v 2.0.0; assert_rc 0
  make_shim "$CASE_DIR/shim" ln "$LAST_LINK_GLOB" 'exit 1'
  take_snapshot home "$HOME"
  run_cmd env "PATH=$CASE_DIR/shim:$PATH" "$SH" "$INSTALL" --rollback
  assert_rc 6; assert_out_contains "restored the previous installation"
  assert_unchanged home "$HOME"
  assert_eq "$(current)" 2.0.0; assert_eq "$(previous)" 1.0.0
  run_cmd "$SH" "$INSTALL" --rollback; assert_rc 0
  assert_eq "$(current)" 1.0.0
}

t_conflict_appears_mid_relink() {
  install_v 1.0.0; assert_rc 0; install_v 2.0.0; assert_rc 0
  # A racing writer replaces luuna-runtime with a foreign file while lu is being relinked.
  make_shim "$CASE_DIR/shim" ln '*/.luuna-install.tmp.*.lu' \
    'rm -f "$LUUNA_INSTALL_BIN/luuna-runtime"; echo foreign > "$LUUNA_INSTALL_BIN/luuna-runtime"'
  take_snapshot home "$HOME"
  install_with "$CASE_DIR/shim" 3.0.0
  assert_rc 6; assert_out_contains "left"; assert_out_contains "not owned"
  # Snapshot lines are "T MODE PATH ..."; PATH starts at column 14 and may contain spaces.
  drop="$LUUNA_INSTALL_BIN/luuna-runtime"
  only_other() { awk -v p="$drop" '!(substr($0, 14, length(p)) == p && substr($0, 14 + length(p), 1) == " ")'; }
  only_other < "$SANDBOX/snaps/home" > "$SANDBOX/snaps/home.filtered"
  snapshot "$HOME" | only_other > "$SANDBOX/snaps/home.filtered.after"
  [ "$(wc -l < "$SANDBOX/snaps/home")" -eq $(($(wc -l < "$SANDBOX/snaps/home.filtered") + 1)) ] || fail "filter did not drop exactly one line"
  if ! cmp -s "$SANDBOX/snaps/home.filtered" "$SANDBOX/snaps/home.filtered.after"; then
    diff "$SANDBOX/snaps/home.filtered" "$SANDBOX/snaps/home.filtered.after" >&2 || true
    fail "filesystem changed beyond the foreign luuna-runtime entry"
  fi
  assert_eq "$(cat "$LUUNA_INSTALL_BIN/luuna-runtime")" foreign
  [ ! -L "$LUUNA_INSTALL_BIN/luuna-runtime" ] || fail "foreign luuna-runtime was replaced"
  assert_eq "$(current)" 2.0.0
}

run_case "upgrade keeps exactly one previous" t_upgrade
run_case "failed upgrade (corrupt, then unsafe archive) leaves current intact" t_failed_upgrade_keeps_current
run_case "rollback swaps current and previous, twice" t_rollback_twice
run_case "rollback with no previous changes nothing" t_rollback_without_previous
run_case "rollback with nothing installed changes nothing" t_rollback_nothing_installed
run_case "rollback --dry-run writes nothing" t_rollback_dry_run
run_case "rollback refuses a foreign bin entry" t_rollback_conflict
run_case "reinstalling the previous version re-stages it" t_reinstall_previous
run_case "rollback with a different LUUNA_INSTALL_BIN than the receipt refuses" t_rollback_bin_mismatch
run_case "upgrade undone after a relink failure" t_upgrade_relink_failure_restores
run_case "reinstall of the previous version undone, old copy restored" t_reinstall_previous_relink_failure_restores_old_dir
run_case "upgrade undone after a receipt write failure" t_receipt_write_failure_restores
run_case "upgrade undone after a pointer write failure" t_pointer_write_failure_restores
run_case "rollback undone after a relink failure" t_rollback_relink_failure_restores
run_case "foreign bin entry appearing mid-relink is left in place" t_conflict_appears_mid_relink
finish_tests
