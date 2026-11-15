#!/bin/sh
# Uninstall removes only receipt-owned links and LUUNA_INSTALL_HOME; project stores survive.
# The path of lib.sh is only known at run time (TESTS_DIR), so shellcheck cannot follow it.
# shellcheck disable=SC1091
. "$TESTS_DIR/lib.sh"

install_v() {
  make_fixture_release "$CASE_DIR/rel-$1" "$1"
  run_cmd "$SH" "$INSTALL" --manifest "$CASE_DIR/rel-$1/manifest.tsv" --asset-dir "$CASE_DIR/rel-$1/assets"
}
make_stores() {
  mkdir -p "$CASE_DIR/project/.luuna/missions/data" "$CASE_DIR/project/.luuna/atlas/data" "$HOME/.luuna"
  echo ticket > "$CASE_DIR/project/.luuna/missions/data/t1"
  echo expertise > "$CASE_DIR/project/.luuna/atlas/data/e1"
  echo keep > "$HOME/.luuna/keep"
}

t_uninstall() {
  make_stores
  install_v 1.0.0; assert_rc 0; install_v 2.0.0; assert_rc 0
  echo mine > "$LUUNA_INSTALL_BIN/my-tool"
  take_snapshot project "$CASE_DIR/project"; take_snapshot dotluuna "$HOME/.luuna"
  run_cmd "$SH" "$INSTALL" --uninstall; assert_rc 0
  assert_unchanged project "$CASE_DIR/project"; assert_unchanged dotluuna "$HOME/.luuna"
  for n in luuna lu luuna-runtime luuna-sbx; do assert_absent "$LUUNA_INSTALL_BIN/$n"; done
  assert_file_contains "$LUUNA_INSTALL_BIN/my-tool" mine
  assert_absent "$LUUNA_INSTALL_HOME"
  assert_exists "$XDG_DATA_HOME"
}

t_uninstall_nothing() {
  take_snapshot home "$HOME"
  run_cmd "$SH" "$INSTALL" --uninstall; assert_rc 0; assert_out_contains "nothing installed"
  assert_unchanged home "$HOME"
}
t_uninstall_dry_run() {
  install_v 1.0.0; assert_rc 0
  take_snapshot home "$HOME"
  run_cmd "$SH" "$INSTALL" --uninstall --dry-run; assert_rc 0; assert_out_contains "would remove"
  assert_unchanged home "$HOME"
}
t_uninstall_no_marker() {
  mkdir -p "$LUUNA_INSTALL_HOME/versions"; echo data > "$LUUNA_INSTALL_HOME/receipt.tsv"
  take_snapshot home "$HOME"
  run_cmd "$SH" "$INSTALL" --uninstall; assert_rc 6; assert_out_contains "marker"
  assert_unchanged home "$HOME"
}
t_uninstall_unknown_entry() {
  install_v 1.0.0; assert_rc 0
  echo precious > "$LUUNA_INSTALL_HOME/notes.txt"
  take_snapshot home "$HOME"
  run_cmd "$SH" "$INSTALL" --uninstall; assert_rc 6; assert_out_contains "not created by this installer"
  assert_unchanged home "$HOME"
}
t_uninstall_bin_mismatch() {
  install_v 1.0.0; assert_rc 0
  take_snapshot home "$HOME"
  LUUNA_INSTALL_BIN="$HOME/other-bin" run_cmd "$SH" "$INSTALL" --uninstall; assert_rc 6; assert_out_contains "rerun with"
  assert_unchanged home "$HOME"
}
t_uninstall_tampered_receipt() {
  install_v 1.0.0; assert_rc 0
  mkdir -p "$CASE_DIR/victim"; echo keep > "$CASE_DIR/victim/f"
  ln -s "$CASE_DIR/victim/f" "$LUUNA_INSTALL_BIN/evil-link"
  printf 'link\tevil-link\t%s\n' "$CASE_DIR/victim/f" >> "$LUUNA_INSTALL_HOME/receipt.tsv"
  printf 'link\t../../victim\t%s\n' "$CASE_DIR/victim" >> "$LUUNA_INSTALL_HOME/receipt.tsv"
  take_snapshot victim "$CASE_DIR/victim"
  run_cmd "$SH" "$INSTALL" --uninstall; assert_rc 0
  assert_unchanged victim "$CASE_DIR/victim"
  assert_exists "$LUUNA_INSTALL_BIN/evil-link"
  assert_absent "$LUUNA_INSTALL_HOME"
}
t_uninstall_link_replaced_by_user() {
  install_v 1.0.0; assert_rc 0
  rm "$LUUNA_INSTALL_BIN/lu"; echo mine > "$LUUNA_INSTALL_BIN/lu"
  run_cmd "$SH" "$INSTALL" --uninstall; assert_rc 0; assert_out_contains "left $LUUNA_INSTALL_BIN/lu in place"
  assert_file_contains "$LUUNA_INSTALL_BIN/lu" mine
  assert_absent "$LUUNA_INSTALL_BIN/luuna"
  assert_absent "$LUUNA_INSTALL_BIN/luuna-runtime"
  assert_absent "$LUUNA_INSTALL_HOME"
}
t_uninstall_symlinked_version_dir() {
  install_v 1.0.0; assert_rc 0
  mkdir -p "$CASE_DIR/victim"; echo keep > "$CASE_DIR/victim/f"
  ln -s "$CASE_DIR/victim" "$LUUNA_INSTALL_HOME/versions/9.9.9"
  take_snapshot victim "$CASE_DIR/victim"; take_snapshot home "$HOME"
  run_cmd "$SH" "$INSTALL" --uninstall; assert_rc 6
  assert_unchanged victim "$CASE_DIR/victim"; assert_unchanged home "$HOME"
}

run_case "uninstall removes owned links and home; stores and foreign files survive" t_uninstall
run_case "uninstall with nothing installed exits 0 and changes nothing" t_uninstall_nothing
run_case "uninstall --dry-run writes nothing" t_uninstall_dry_run
run_case "uninstall without marker refuses" t_uninstall_no_marker
run_case "uninstall with an unknown home entry refuses" t_uninstall_unknown_entry
run_case "uninstall with a different LUUNA_INSTALL_BIN refuses" t_uninstall_bin_mismatch
run_case "tampered receipt cannot remove foreign paths" t_uninstall_tampered_receipt
run_case "user-replaced lu is left in place" t_uninstall_link_replaced_by_user
run_case "symlinked version dir is never followed" t_uninstall_symlinked_version_dir
finish_tests
