#!/bin/sh
# Run every packaging/installer/tests/test-*.sh in its own throwaway sandbox.
#
# Each test file gets a fresh `mktemp -d` sandbox. HOME, XDG_DATA_HOME, LUUNA_INSTALL_HOME,
# LUUNA_INSTALL_BIN, TMPDIR and PATH all point inside it, so no test can touch the real home
# directory, the real PATH, the network, or a real `luuna`/`lu`. Exits non-zero if any case fails.
#
# Usage: sh packaging/installer/tests/run.sh [packaging/installer/tests/test-NAME.sh ...]
# Set TEST_SHELL=dash (or bash) to run the test files, and the installer, under that shell.
set -eu

tests_dir=$(cd "$(dirname "$0")" && pwd -P)
repo_root=$(cd "$tests_dir/../../.." && pwd -P)
real_home=${HOME:-}
real_home_phys=""
if [ -n "$real_home" ] && [ -d "$real_home" ]; then real_home_phys=$(cd "$real_home" && pwd -P); fi
shell=${TEST_SHELL:-sh}

# Tools resolved from the caller's PATH that the sandbox PATH must still provide.
tool_links=""
for tool in git bash; do
  if p=$(command -v "$tool" 2>/dev/null); then tool_links="$tool_links $tool=$p"; fi
done

if [ $# -gt 0 ]; then files="$*"; else files=$(ls "$tests_dir"/test-*.sh); fi

failed=0
for file in $files; do
  [ -f "$file" ] || { echo "not ok - missing test file $file"; failed=1; continue; }
  sandbox=$(mktemp -d "${TMPDIR:-/tmp}/luuna-install-test.XXXXXX")
  sandbox=$(cd "$sandbox" && pwd -P)
  mkdir -p "$sandbox/home" "$sandbox/tools"
  for pair in $tool_links; do ln -s "${pair#*=}" "$sandbox/tools/${pair%%=*}"; done

  sb_home="$sandbox/home"
  if [ "$sb_home" = "$real_home" ] || [ "$sb_home" = "$real_home_phys" ]; then
    echo "Bail out! sandbox HOME resolves to the real home" >&2; exit 1
  fi
  case "$sb_home/.local/share/luuna-install" in
    "$sandbox"/*) ;;
    *) echo "Bail out! LUUNA_INSTALL_HOME would escape the sandbox" >&2; exit 1 ;;
  esac

  echo "# $(basename "$file")"
  rc=0
  env -i \
    HOME="$sb_home" \
    XDG_DATA_HOME="$sb_home/.local/share" \
    LUUNA_INSTALL_HOME="$sb_home/.local/share/luuna-install" \
    LUUNA_INSTALL_BIN="$sb_home/.local/bin" \
    PATH="$sb_home/.local/bin:/usr/bin:/bin:/usr/sbin:/sbin:$sandbox/tools" \
    TMPDIR="$sandbox" \
    LC_ALL=C \
    COPYFILE_DISABLE=1 \
    SANDBOX="$sandbox" \
    REPO_ROOT="$repo_root" \
    TESTS_DIR="$tests_dir" \
    REAL_HOME_GUARD="$real_home_phys" \
    TEST_SHELL="$shell" \
    "$shell" "$file" || rc=$?
  if [ "$rc" -ne 0 ]; then
    failed=1
    echo "# FAILED: $(basename "$file") (sandbox kept at $sandbox)"
  else
    chmod -R u+rwx "$sandbox" 2>/dev/null || true
    rm -rf "$sandbox"
  fi
done

if [ "$failed" -ne 0 ]; then echo "# some tests failed"; exit 1; fi
echo "# all tests passed"
