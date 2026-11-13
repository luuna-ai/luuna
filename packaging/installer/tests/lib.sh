# Shared helpers for the installer test suite. Source it from a test-*.sh file run by run.sh.
# shellcheck shell=sh
# Reasons: SC2034, the variables set here are read by the test files that source this one.
# SC2012, ls -ld is used for the mode text (find cannot print it portably) and ls -A for the entry names of directories the tests created.
# SC2016, the shim bodies and the stub scripts are written as literal text that runs later, on purpose.
# shellcheck disable=SC2034,SC2012,SC2016
set -eu

: "${SANDBOX:?run tests through packaging/installer/tests/run.sh}"
: "${REPO_ROOT:?run tests through packaging/installer/tests/run.sh}"
: "${TESTS_DIR:?run tests through packaging/installer/tests/run.sh}"

TAB=$(printf '\t')
CASES_RUN=0
CASES_FAILED=0

# --- sandbox guard --------------------------------------------------------------
guard_sandbox() {
  case "$HOME" in "$SANDBOX"/*) ;; *) echo "Bail out! HOME is outside the sandbox: $HOME" >&2; exit 1 ;; esac
  case "$XDG_DATA_HOME" in "$SANDBOX"/*) ;; *) echo "Bail out! XDG_DATA_HOME outside sandbox" >&2; exit 1 ;; esac
  case "$LUUNA_INSTALL_HOME" in "$SANDBOX"/*) ;; *) echo "Bail out! LUUNA_INSTALL_HOME outside sandbox" >&2; exit 1 ;; esac
  case "$LUUNA_INSTALL_BIN" in "$SANDBOX"/*) ;; *) echo "Bail out! LUUNA_INSTALL_BIN outside sandbox" >&2; exit 1 ;; esac
  if [ -n "${REAL_HOME_GUARD:-}" ] && [ "$HOME" = "$REAL_HOME_GUARD" ]; then
    echo "Bail out! HOME is the real home" >&2; exit 1
  fi
}
guard_sandbox

# --- cases ------------------------------------------------------------------------
# case_env NAME: point HOME and the install roots at a fresh per-case directory.
case_env() {
  CASE_DIR="$SANDBOX/cases/$1"
  rm -rf "$CASE_DIR"
  mkdir -p "$CASE_DIR/home" "$CASE_DIR/work"
  HOME="$CASE_DIR/home"
  XDG_DATA_HOME="$HOME/.local/share"
  LUUNA_INSTALL_HOME="$XDG_DATA_HOME/luuna-install"
  LUUNA_INSTALL_BIN="$HOME/.local/bin"
  PATH="$LUUNA_INSTALL_BIN:/usr/bin:/bin:/usr/sbin:/sbin:$SANDBOX/tools"
  export HOME XDG_DATA_HOME LUUNA_INSTALL_HOME LUUNA_INSTALL_BIN PATH CASE_DIR
  guard_sandbox
}

# run_case NAME FUNCTION: run FUNCTION in a subshell with a fresh case_env and report ok/not ok.
run_case() {
  CASES_RUN=$((CASES_RUN + 1))
  if ( case_env "$1"; "$2" ) > "$SANDBOX/case.log" 2>&1; then
    echo "ok $CASES_RUN - $1"
  else
    CASES_FAILED=$((CASES_FAILED + 1))
    echo "not ok $CASES_RUN - $1"
    sed 's/^/#   /' "$SANDBOX/case.log"
  fi
}

# skip_case NAME REASON
skip_case() { CASES_RUN=$((CASES_RUN + 1)); echo "ok $CASES_RUN - $1 # SKIP $2"; }

finish_tests() {
  echo "# $CASES_RUN cases, $CASES_FAILED failed"
  [ "$CASES_FAILED" -eq 0 ]
}

# --- assertions -----------------------------------------------------------------
fail() { echo "FAIL: $*" >&2; exit 1; }

# run_cmd CMD...: run a command, storing its exit code in $RC and its combined output in $OUT_FILE.
run_cmd() {
  OUT_FILE="$SANDBOX/out.$$"
  RC=0
  "$@" > "$OUT_FILE" 2>&1 || RC=$?
}

assert_rc() { # EXPECTED
  [ "$RC" -eq "$1" ] || { echo "--- output:" >&2; cat "$OUT_FILE" >&2; fail "expected exit $1, got $RC"; }
}
assert_rc_nonzero() {
  [ "$RC" -ne 0 ] || { cat "$OUT_FILE" >&2; fail "expected non-zero exit"; }
}
assert_out_contains() { # STRING
  grep -F -e "$1" "$OUT_FILE" >/dev/null 2>&1 || { cat "$OUT_FILE" >&2; fail "output does not contain: $1"; }
}
assert_out_not_contains() {
  if grep -F -e "$1" "$OUT_FILE" >/dev/null 2>&1; then cat "$OUT_FILE" >&2; fail "output contains: $1"; fi
}
assert_eq() { [ "$1" = "$2" ] || fail "expected [$2], got [$1] ${3:-}"; }
assert_exists() { [ -e "$1" ] || [ -L "$1" ] || fail "missing: $1"; }
assert_absent() { if [ -e "$1" ] || [ -L "$1" ]; then fail "should not exist: $1"; fi; }
assert_file_contains() { grep -F -e "$2" "$1" >/dev/null 2>&1 || fail "$1 does not contain: $2"; }

# snapshot DIR: stable description of every path below DIR (type, mode, link target, content checksum).
snapshot() {
  if [ ! -e "$1" ] && [ ! -L "$1" ]; then echo "(absent)"; return 0; fi
  find "$1" -print | LC_ALL=C sort | while IFS= read -r p; do
    mode=$(ls -ld "$p" | cut -c1-10)
    if [ -L "$p" ]; then echo "L $mode $p -> $(readlink "$p")"
    elif [ -f "$p" ]; then echo "F $mode $p $(cksum < "$p")"
    elif [ -d "$p" ]; then echo "D $mode $p"
    else echo "O $mode $p"; fi
  done
}
# take_snapshot NAME DIR / assert_unchanged NAME DIR (snapshots live outside every case directory)
take_snapshot() { mkdir -p "$SANDBOX/snaps"; snapshot "$2" > "$SANDBOX/snaps/$1"; }
assert_unchanged() {
  snapshot "$2" > "$SANDBOX/snaps/$1.after"
  if ! cmp -s "$SANDBOX/snaps/$1" "$SANDBOX/snaps/$1.after"; then
    diff "$SANDBOX/snaps/$1" "$SANDBOX/snaps/$1.after" >&2 || true
    fail "filesystem changed under $2"
  fi
}

# --- fault injection and barriers ---------------------------------------------------
# real_tool NAME: absolute path of NAME on the case PATH (call it before any shim is prepended).
real_tool() {
  _rt=$(command -v "$1" 2>/dev/null) || fail "tool not found: $1"
  case "$_rt" in /*) printf '%s\n' "$_rt" ;; *) fail "tool is not an absolute path: $1 ($_rt)" ;; esac
}

# make_shim DIR NAME LASTARG_GLOB BODY: write a one-shot PATH shim DIR/NAME. It fires when
# $CASE_DIR/fault.NAME.fired is absent and its last argument matches LASTARG_GLOB: it creates the
# fired file, then runs BODY. Otherwise, or when BODY falls through, it execs the real tool.
# The real tool's path and the glob are read from sidecar data files at run time and passed to
# BODY in the environment as REAL; they are never interpolated into the shim's code.
make_shim() {
  case "$1" in *:*) fail "shim directory must not contain ':' (it goes on PATH): $1" ;; esac
  mkdir -p "$1"
  real_tool "$2" > "$1/$2.real"
  printf '%s\n' "$3" > "$1/$2.glob"
  {
    printf '%s\n' '#!/bin/sh' \
      'REAL=$(cat "$0.real"); SHIM_GLOB=$(cat "$0.glob"); export REAL' \
      '_last=""; for _a in "$@"; do _last=$_a; done' \
      '_fired="$CASE_DIR/fault.${0##*/}.fired"' \
      '_hit=0' \
      '# shellcheck disable=SC2254' \
      'case "$_last" in $SHIM_GLOB) _hit=1 ;; esac' \
      'if [ ! -e "$_fired" ] && [ "$_hit" = 1 ]; then' \
      '  : > "$_fired"'
    printf '%s\n' "$4"
    printf '%s\n' 'fi' 'exec "$REAL" "$@"'
  } > "$1/$2"
  chmod 755 "$1/$2"
}

# pause_body TAG: shim BODY that blocks at a FIFO barrier until release_paused TAG.
pause_body() {
  printf ': > "$CASE_DIR/%s.signalled"; echo ready > "$CASE_DIR/%s.ready"; read -r _ < "$CASE_DIR/%s.go"\n' "$1" "$1" "$1"
}

# start_paused TAG CMD...: run CMD in the background and wait (no sleeps) until a pause_body
# shim in it reaches its barrier. Fails the case if CMD exits without pausing.
start_paused() {
  _tag=$1; shift
  rm -f "$CASE_DIR/$_tag.ready" "$CASE_DIR/$_tag.go" "$CASE_DIR/$_tag.signalled" "$CASE_DIR/$_tag.rc"
  mkfifo "$CASE_DIR/$_tag.ready" "$CASE_DIR/$_tag.go"
  (
    _prc=0
    "$@" > "$CASE_DIR/$_tag.out" 2>&1 || _prc=$?
    echo "$_prc" > "$CASE_DIR/$_tag.rc"
    if [ ! -e "$CASE_DIR/$_tag.signalled" ]; then echo exit > "$CASE_DIR/$_tag.ready"; fi
  ) &
  echo "$!" > "$CASE_DIR/$_tag.pid"
  _line=""
  read -r _line < "$CASE_DIR/$_tag.ready" || :
  [ "$_line" = ready ] || { cat "$CASE_DIR/$_tag.out" >&2; fail "$_tag exited without reaching its pause point"; }
}

# release_paused TAG: open the barrier, wait for the command, and set RC and OUT_FILE from it.
release_paused() {
  echo go > "$CASE_DIR/$1.go"
  wait "$(cat "$CASE_DIR/$1.pid")" || :
  RC=$(cat "$CASE_DIR/$1.rc")
  OUT_FILE="$CASE_DIR/$1.out"
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}'
  else shasum -a 256 "$1" | awk '{print $1}'; fi
}

# --- portable ustar writer ----------------------------------------------------------
# Tests craft archives byte-for-byte so malicious entries (traversal, absolute paths, links,
# devices, duplicates) are possible with any host tar. NUL bytes are written as \001 placeholders
# and translated by tr, because awk implementations differ on printing NUL.
# tar_add RAW NAME TYPE MODE [CONTENT_OR_LINKNAME]
#   TYPE: 0 regular file, 1 hardlink, 2 symlink, 3 char device, 5 directory
tar_add() {
  _raw=$1 _name=$2 _type=$3 _mode=$4 _data=${5:-}
  _size=0 _link=""
  case "$_type" in
    0) _size=$(printf '%s' "$_data" | wc -c | tr -d ' ') ;;
    1|2) _link=$_data ;;
  esac
  awk -v name="$_name" -v type="$_type" -v mode="$_mode" -v size="$_size" -v link="$_link" '
    function pad(s, n,   r) { r = s; while (length(r) < n) r = r "\001"; return r }
    BEGIN {
      for (i = 32; i < 127; i++) ord[sprintf("%c", i)] = i
      m = mode; while (length(m) < 7) m = "0" m   # mode is already an octal string such as 755
      h = pad(name, 100) m "\001" "0000000\001" "0000000\001"
      h = h sprintf("%011o", size) "\001" "00000000000\001"
      pre = h; post = type pad(link, 100) "ustar\001" "00" pad("root", 32) pad("root", 32)
      post = post "0000000\001" "0000000\001" pad("", 155) pad("", 12)
      sum = 0; all = pre "        " post
      for (i = 1; i <= length(all); i++) { c = substr(all, i, 1); if (c in ord) sum += ord[c] }
      printf "%s%s%s", pre, sprintf("%06o", sum) "\001 ", post
    }' | tr '\001' '\000' >> "$_raw"
  if [ "$_type" = 0 ] && [ "$_size" -gt 0 ]; then
    printf '%s' "$_data" >> "$_raw"
    _pad=$(( (512 - _size % 512) % 512 ))
    if [ "$_pad" -gt 0 ]; then dd if=/dev/zero bs=1 count="$_pad" 2>/dev/null >> "$_raw"; fi
  fi
}
# tar_close RAW OUT.tar.gz
tar_close() {
  dd if=/dev/zero bs=512 count=2 2>/dev/null >> "$1"
  gzip -n -c "$1" > "$2"
  rm -f "$1"
}

# --- release fixtures ------------------------------------------------------------
FIXTURE_OS=${FIXTURE_OS:-linux}
FIXTURE_ARCH=${FIXTURE_ARCH:-amd64}
# Matching platform overrides for packaging/installer/install.sh.
LUUNA_INSTALL_UNAME_S=Linux
LUUNA_INSTALL_UNAME_M=x86_64
export LUUNA_INSTALL_UNAME_S LUUNA_INSTALL_UNAME_M

# The one component of the consolidated release and its three executables (assumption I6).
FIXTURE_COMPONENT=luuna

stub_script() { # EXE VERSION
  printf '#!/bin/sh\necho "stub:%s:%s"\nfor a in "$@"; do printf "arg:[%%s]\\n" "$a"; done\n' "$1" "$2"
}

# make_fixture_release DIR VERSION [KIND...]
# Writes DIR/assets/luuna_<V>_<os>_<arch>.tar.gz and DIR/manifest.tsv for FIXTURE_OS/FIXTURE_ARCH, with
# the SHA256 computed from the generated stub archive. KIND is one of
#   corrupt badsha traversal absolute symlink hardlink device extra missing noexec dup
# The mutation is applied to the archive's last executable (luuna-sbx) or adds an entry beside it.
make_fixture_release() {
  _dir=$1 _ver=$2; shift 2
  mkdir -p "$_dir/assets"
  {
    printf 'luuna-release-manifest\t1\n'
    printf 'release\t%s\n' "$_ver"
  } > "$_dir/manifest.tsv"
  _kind=""
  for _m in "$@"; do _kind=$_m; done
  _file="${FIXTURE_COMPONENT}_${_ver}_${FIXTURE_OS}_${FIXTURE_ARCH}.tar.gz"
  _raw="$_dir/assets/$_file.raw"
  : > "$_raw"
  tar_add "$_raw" "bin/" 5 755
  tar_add "$_raw" "bin/luuna-tui" 0 755 "$(stub_script luuna-tui "$_ver")"
  tar_add "$_raw" "bin/luuna-runtime" 0 755 "$(stub_script luuna-runtime "$_ver")"
  case "$_kind" in
    missing) ;;
    noexec) tar_add "$_raw" "bin/luuna-sbx" 0 644 "$(stub_script luuna-sbx "$_ver")" ;;
    symlink) tar_add "$_raw" "bin/luuna-sbx" 2 777 "/bin/sh" ;;
    hardlink) tar_add "$_raw" "bin/luuna-sbx" 1 755 "bin/luuna-tui" ;;
    dup)
      tar_add "$_raw" "bin/luuna-sbx" 0 755 "$(stub_script luuna-sbx "$_ver")"
      tar_add "$_raw" "bin/luuna-sbx" 2 777 "../../../../escape" ;;
    *) tar_add "$_raw" "bin/luuna-sbx" 0 755 "$(stub_script luuna-sbx "$_ver")" ;;
  esac
  case "$_kind" in
    traversal) tar_add "$_raw" "bin/../../evil" 0 755 "evil" ;;
    absolute) tar_add "$_raw" "$SANDBOX/absolute-evil" 0 755 "evil" ;;
    device) tar_add "$_raw" "bin/dev" 3 644 ;;
    extra) tar_add "$_raw" "README" 0 644 "extra" ;;
  esac
  tar_close "$_raw" "$_dir/assets/$_file"
  _sha=$(sha256_of "$_dir/assets/$_file")
  case "$_kind" in
    corrupt) printf 'X' | dd of="$_dir/assets/$_file" bs=1 seek=40 conv=notrunc 2>/dev/null ;;
    badsha) _sha=$(printf '%s' "$_sha" | tr '0-9a-f' '1-9a-f0') ;;
  esac
  printf 'asset\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$FIXTURE_COMPONENT" "$_ver" "$FIXTURE_OS" "$FIXTURE_ARCH" \
    "https://github.com/luuna-ai/luuna/releases/download/v$_ver/$_file" \
    "$_sha" "$_file" "luuna-tui,luuna-runtime,luuna-sbx" >> "$_dir/manifest.tsv"
}

INSTALL="$REPO_ROOT/packaging/installer/install.sh"
MANIFEST_LIB="$REPO_ROOT/packaging/installer/install-manifest.sh"
CHECK_MANIFESTS="$REPO_ROOT/packaging/installer/tests/check-manifests.sh"
SH=${TEST_SHELL:-sh}
