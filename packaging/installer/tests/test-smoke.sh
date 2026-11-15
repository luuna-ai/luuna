#!/bin/sh
# Smoke test: the dependency-free tool set the installer tests rely on exists, and the sandbox is isolated.
# The path of lib.sh is only known at run time (TESTS_DIR), so shellcheck cannot follow it.
# shellcheck disable=SC1091
. "$TESTS_DIR/lib.sh"

t_tools() {
  for tool in sh tar gzip mktemp grep sed awk ln mv cp rm find sort cksum readlink od dd tr wc cmp env; do
    command -v "$tool" >/dev/null 2>&1 || fail "required tool missing: $tool"
  done
  command -v sha256sum >/dev/null 2>&1 || command -v shasum >/dev/null 2>&1 || fail "need sha256sum or shasum"
}

t_isolated() {
  case "$HOME" in "$SANDBOX"/*) ;; *) fail "HOME not sandboxed" ;; esac
  case "$XDG_DATA_HOME" in "$HOME"/*) ;; *) fail "XDG_DATA_HOME not under sandbox HOME" ;; esac
  case "$LUUNA_INSTALL_HOME" in "$HOME"/*) ;; *) fail "LUUNA_INSTALL_HOME not under sandbox HOME" ;; esac
  case "$LUUNA_INSTALL_BIN" in "$HOME"/*) ;; *) fail "LUUNA_INSTALL_BIN not under sandbox HOME" ;; esac
  case "$TMPDIR" in "$SANDBOX"*) ;; *) fail "TMPDIR not sandboxed" ;; esac
  case "$PATH" in "$LUUNA_INSTALL_BIN":*) ;; *) fail "PATH does not start with the sandbox bin" ;; esac
  assert_eq "$LUUNA_INSTALL_UNAME_S" Linux
  assert_eq "$LUUNA_INSTALL_UNAME_M" x86_64
}

t_repo_root() {
  [ -f "$REPO_ROOT/packaging/installer/tests/run.sh" ] || fail "REPO_ROOT is not the repository root: $REPO_ROOT"
  [ -f "$TESTS_DIR/run.sh" ] || fail "TESTS_DIR does not hold run.sh: $TESTS_DIR"
  case "$SANDBOX" in "$REPO_ROOT"/*) fail "the sandbox must not be inside the repository" ;; esac
}

t_tar_writer() {
  raw="$CASE_DIR/t.raw"; : > "$raw"
  tar_add "$raw" "bin/" 5 755
  tar_add "$raw" "bin/hello" 0 755 "hello world"
  tar_close "$raw" "$CASE_DIR/t.tar.gz"
  names=$(tar -tzf "$CASE_DIR/t.tar.gz" | tr '\n' ' ')
  assert_eq "$names" "bin/ bin/hello "
  mkdir "$CASE_DIR/x" && tar -xzf "$CASE_DIR/t.tar.gz" -C "$CASE_DIR/x"
  assert_eq "$(cat "$CASE_DIR/x/bin/hello")" "hello world"
}

run_case "required tools exist" t_tools
run_case "sandbox is isolated" t_isolated
run_case "REPO_ROOT is the repository root" t_repo_root
run_case "portable ustar writer round-trips" t_tar_writer
if command -v git >/dev/null 2>&1; then run_case "git available" true; else skip_case "git available" "git not installed: git-based cases will skip"; fi
finish_tests
