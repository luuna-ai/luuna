#!/bin/sh
# Luuna installer for macOS and Linux (POSIX sh). See --help and packaging/installer/releases/FORMAT.md.
#
# It installs one release archive of luuna-tui, luuna-runtime and luuna-sbx for the current user and
# links the public names `luuna` and `lu` (both to the installed luuna-tui) and `luuna-runtime`.
# luuna-sbx sits beside luuna-tui and is not linked.
#
# Safety model:
#   - Writes only under LUUNA_INSTALL_HOME and LUUNA_INSTALL_BIN. Both are validated (absolute, not /,
#     no control characters, no '.', '..' or '//' segments, not HOME or an ancestor of it, not a
#     symlink, not inside the repository that holds this script, not nested in each other).
#   - No sudo, no shell-profile or PATH edit, never evaluates data as code, never runs a downloaded
#     file. Never writes .luuna/, ~/.luuna, ~/.pi or a repository.
#   - Fetches use https only (curl --proto =https --proto-redir =https --tlsv1.2, or wget
#     --https-only). Every archive is checked against the manifest's SHA-256 and its listing is
#     allow-listed (bin/ and the row's executables only; no links, devices, traversal or duplicates)
#     before extraction. Everything is staged before anything visible changes.
#   - The public names are a fixed table (PUBLIC_NAMES and link_exe below), never the files of an
#     archive. A public name is owned only if it is a symlink into LUUNA_INSTALL_HOME/versions/*/bin/*
#     without '.' or '..' segments. Anything else at `luuna`, `lu` or `luuna-runtime` (a file, a link
#     from Homebrew or from another installer) is an ownership conflict (exit 6) and is never
#     replaced or removed. An installation of the suite hub's installer is never adopted: it has
#     another home, another marker and other link names.
#   - current/previous are regular pointer files replaced by rename. Bin links are replaced by
#     rename only after an ownership check, so no swap follows a destination symlink.
#   - Recursive removal needs the home marker, a matching receipt, and only installer-created
#     entries. Mutations hold a mkdir lock.
#   - The lock is taken before the home marker is written, and only the lock holder may write or
#     remove the marker. A run that loses the lock (exit 8) removes only directories it created.
#   - Install, rollback and uninstall (including --dry-run) refuse when receipt.tsv names a
#     different LUUNA_INSTALL_HOME or LUUNA_INSTALL_BIN, or when current exists without a receipt.
#   - The commit phase (version dir, pointers, bin links, receipt) is journaled under the lock in
#     a .trash.* directory. A failure or HUP/INT/TERM before the receipt is written replays the
#     journal and restores the previous state byte for byte; foreign bin entries seen at undo
#     time are left untouched. The receipt write is the commit point; GC after it is non-fatal.
#     If the undo itself fails, the journal is retained for manual recovery (docs/INSTALL.md).
#   - Residual races: POSIX mv has no no-replace rename. Every check-then-rename or check-then-unlink
#     of a bin entry (link_bin, undo's link restore/removal, uninstall) has a narrowed but open
#     window: a foreign entry created in that instant can still be replaced or removed. link_bin
#     re-checks ownership immediately before `mv -f`. A stale .lock left by a crash blocks the next
#     run (exit 8) until it is removed by hand.
set -eu
umask 022

PROG="luuna install"
script_dir=$(cd "$(dirname "$0")" && pwd -P)
# The repository that holds this script: packaging/installer/ sits two levels below its root.
repo_root=$(cd "$script_dir/../.." && pwd -P)
# The manifest library sits beside this script; its path is only known at run time (SC1091).
# shellcheck disable=SC1091
. "$script_dir/install-manifest.sh"

TAB=$(printf '\t')
PUBLIC_NAMES="luuna lu luuna-runtime"
MARKER_NAME=.luuna-install-home
MARKER_TEXT="luuna-install-home 1"
VERSION_META=.luuna-install-version.tsv
RECEIPT_HEADER="luuna-install-receipt${TAB}1"
BIN_TMP_PREFIX=.luuna-install.tmp

# link_exe NAME: the installed executable a public name links to (the fixed link table).
link_exe() {
  case "$1" in
    luuna|lu) echo luuna-tui ;;
    luuna-runtime) echo luuna-runtime ;;
    *) return 1 ;;
  esac
}

usage() {
  cat <<'USAGE'
Usage: install.sh --manifest PATH [--asset-dir DIR] [--dry-run]
       install.sh --version V [--asset-dir DIR] [--dry-run]
       install.sh --rollback [--dry-run]
       install.sh --uninstall [--dry-run]
       install.sh --help

Installs Luuna for the current user on macOS or Linux, without sudo. A release archive carries
luuna-tui, luuna-runtime and luuna-sbx. The installer links `luuna` and `lu` to the installed
luuna-tui and `luuna-runtime` to the installed luuna-runtime; luuna-sbx stays beside luuna-tui.
It never replaces or removes a `luuna`, `lu` or `luuna-runtime` it did not install (exit 6), and it
never edits shell profiles or PATH.

  --manifest PATH  Install the release described by a manifest file (packaging/installer/releases/FORMAT.md)
  --version V      Install packaging/installer/releases/V/manifest.tsv from this checkout. It must match
                   packaging/installer/releases/INDEX.tsv. No releases are published yet, so this
                   currently always fails.
  --asset-dir DIR  Read archives from DIR (offline). Without it, archives are fetched from the
                   manifest's https URLs with curl or wget.
  --rollback       Swap the current and previous installed versions
  --uninstall      Remove the receipt-owned links and LUUNA_INSTALL_HOME. Project stores (.luuna/)
                   are never touched.
  --dry-run        Validate and print the plan without writing anything

Environment:
  LUUNA_INSTALL_HOME  Install root (default: ${XDG_DATA_HOME:-$HOME/.local/share}/luuna-install)
  LUUNA_INSTALL_BIN   Directory for the command links (default: $HOME/.local/bin)
  LUUNA_INSTALL_UNAME_S, LUUNA_INSTALL_UNAME_M
                      Override `uname -s` / `uname -m`. These exist for tests only.

Supported platforms: macOS and Linux on x86_64 and arm64. Windows is not supported by this installer.

Exit codes: 0 ok, 2 usage or invalid input, 3 unsupported platform, 4 checksum mismatch,
5 unsafe or malformed archive, 6 ownership conflict, 7 fetch failure, 8 another install holds the lock.
USAGE
}

die() { _code=$1; shift; echo "$PROG: $*" >&2; exit "$_code"; }
say() { echo "$PROG: $*"; }

# --- arguments ------------------------------------------------------------------
mode=""; manifest=""; version=""; asset_dir=""; dry_run=0
set_mode() { [ -z "$mode" ] || [ "$mode" = "$1" ] || die 2 "choose one of install, --rollback, --uninstall"; mode=$1; }
while [ $# -gt 0 ]; do
  case "$1" in
    --manifest) [ $# -ge 2 ] || die 2 "--manifest needs a path"; set_mode install; manifest=$2; shift 2 ;;
    --version) [ $# -ge 2 ] || die 2 "--version needs a value"; set_mode install; version=$2; shift 2 ;;
    --asset-dir) [ $# -ge 2 ] || die 2 "--asset-dir needs a directory"; asset_dir=$2; shift 2 ;;
    --rollback) set_mode rollback; shift ;;
    --uninstall) set_mode uninstall; shift ;;
    --dry-run) dry_run=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) die 2 "unknown argument: $1 (see --help)" ;;
  esac
done
[ -n "$mode" ] || { usage >&2; exit 2; }
if [ "$mode" = install ]; then
  [ -n "$manifest" ] && [ -n "$version" ] && die 2 "use --manifest or --version, not both"
else
  [ -z "$asset_dir" ] || die 2 "--asset-dir only applies to an install"
fi

# --- roots ----------------------------------------------------------------------
[ -n "${HOME:-}" ] || die 2 "HOME is not set"
case "$HOME" in /*) ;; *) die 2 "HOME must be an absolute path" ;; esac
[ -d "$HOME" ] || die 2 "HOME does not exist: $HOME"
home_phys=$(cd "$HOME" && pwd -P)

# resolve_root NAME PATH: validate an install root and print its physical absolute path.
resolve_root() {
  _n=$1 _p=$2
  case "$_p" in /*) ;; *) die 2 "$_n must be an absolute path: $_p" ;; esac
  if [ "$(printf '%s' "$_p" | LC_ALL=C tr -d '\001-\037\177' | wc -c)" -ne "$(printf '%s' "$_p" | wc -c)" ]; then
    die 2 "$_n must not contain control characters"
  fi
  while [ "$_p" != / ] && [ "${_p%/}" != "$_p" ]; do _p=${_p%/}; done
  [ "$_p" != / ] || die 2 "$_n must not be /"
  case "$_p/" in */./*|*/../*|*//*) die 2 "$_n must not contain '.', '..' or '//' segments: $_p" ;; esac
  [ ! -L "$_p" ] || die 2 "$_n must not be a symlink: $_p"
  if [ -e "$_p" ] && [ ! -d "$_p" ]; then die 2 "$_n exists and is not a directory: $_p"; fi
  _anc=$_p _rest=""
  while [ ! -d "$_anc" ]; do
    [ ! -L "$_anc" ] || die 2 "$_n has a dangling symlink component: $_anc"
    _rest="/${_anc##*/}$_rest"; _anc=${_anc%/*}; [ -n "$_anc" ] || _anc=/
  done
  _base=$(cd "$_anc" && pwd -P)
  [ "$_base" != / ] || _base=""
  _phys="$_base$_rest"
  [ -n "$_phys" ] || die 2 "$_n must not be /"
  [ "$_phys" != "$home_phys" ] || die 2 "$_n must not be HOME itself: $_phys"
  case "$home_phys/" in "$_phys"/*) die 2 "$_n must not be an ancestor of HOME: $_phys" ;; esac
  case "$_phys/" in "$repo_root"/*) die 2 "$_n must not be inside the repository that holds this installer: $_phys" ;; esac
  printf '%s\n' "$_phys"
}

xdg=${XDG_DATA_HOME:-}
case "$xdg" in /*) ;; *) xdg="$HOME/.local/share" ;; esac
H=$(resolve_root LUUNA_INSTALL_HOME "${LUUNA_INSTALL_HOME:-$xdg/luuna-install}") || exit $?
B=$(resolve_root LUUNA_INSTALL_BIN "${LUUNA_INSTALL_BIN:-$HOME/.local/bin}") || exit $?
[ "$H" != "$B" ] || die 2 "LUUNA_INSTALL_HOME and LUUNA_INSTALL_BIN must differ"
case "$B/" in "$H"/*) die 2 "LUUNA_INSTALL_BIN must not be inside LUUNA_INSTALL_HOME" ;; esac
case "$H/" in "$B"/*) die 2 "LUUNA_INSTALL_HOME must not be inside LUUNA_INSTALL_BIN" ;; esac

# --- cleanup and lock ---------------------------------------------------------------
STAGE=""; LOCKED=0; CREATED_DIRS=""; CREATED_MARKER=0
# Commit journal: TXN is none, active or done. See txn_begin/txn_undo.
TXN=none; TXN_DIR=""; NEW_DEST=""; HAD_OLD=0; VERSIONS_CREATED=0; UNDONE=0; UNDO_FAILED=0
cleanup() {
  _rc=$?
  trap - EXIT
  trap '' HUP INT TERM
  [ -z "$STAGE" ] || rm -rf "$STAGE" || :
  if [ "$_rc" -ne 0 ] && [ "$TXN" = active ] && [ "$LOCKED" = 1 ]; then txn_undo; fi
  if [ "$LOCKED" = 1 ]; then
    for _t in "$H"/.tmp.* "$H"/.trash.*; do
      # Keep the journal when undo failed, so the backups survive for inspection; the next
      # locked run sweeps it.
      if [ "$UNDO_FAILED" = 1 ] && [ "$_t" = "$TXN_DIR" ]; then continue; fi
      if [ -e "$_t" ]; then rm -rf "$_t" || :; fi
    done
  fi
  _undo_ok=0
  if [ "$_rc" -ne 0 ] && [ "$TXN" != "done" ] && { [ "$TXN" = none ] || [ "$UNDONE" = 1 ]; }; then _undo_ok=1; fi
  if [ "$_undo_ok" = 1 ] && [ "$LOCKED" = 1 ] && [ "$CREATED_MARKER" = 1 ]; then rm -f "$H/$MARKER_NAME" || :; fi
  [ "$LOCKED" = 0 ] || rm -rf "$H/.lock" || :
  if [ "$_undo_ok" = 1 ] && [ -n "$CREATED_DIRS" ]; then
    # Remove the directories this run created, deepest first; rmdir only removes empty ones.
    while IFS= read -r _d; do
      if [ -n "$_d" ]; then rmdir "$_d" 2>/dev/null || true; fi
    done <<EOF
$CREATED_DIRS
EOF
  fi
  exit "$_rc"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

# mkdir_tracked DIR: create DIR and missing ancestors, remembering them for failure cleanup.
mkdir_tracked() {
  _todo="" _d=$1
  while [ ! -d "$_d" ]; do
    [ ! -e "$_d" ] && [ ! -L "$_d" ] || die 6 "cannot create $_d: a non-directory is in the way"
    _todo="$_d
$_todo"
    _d=${_d%/*}; [ -n "$_d" ] || _d=/
  done
  # Shallowest first; CREATED_DIRS keeps deepest first for cleanup. Paths may contain spaces.
  # A concurrent run may create the same directory: tolerate that, but only remember the
  # directories this process actually created.
  while IFS= read -r _d; do
    [ -n "$_d" ] || continue
    if mkdir "$_d" 2>/dev/null; then
      CREATED_DIRS="$_d
$CREATED_DIRS"
    elif [ ! -d "$_d" ] || [ -L "$_d" ]; then
      die 6 "cannot create $_d"
    fi
  done <<EOF
$_todo
EOF
}

acquire_lock() {
  mkdir "$H/.lock" 2>/dev/null || die 8 "another install appears to be running ($H/.lock exists). If none is, remove that directory and retry."
  LOCKED=1
  printf '%s\n' "$$" > "$H/.lock/pid"
}

marker_ok() { [ -f "$H/$MARKER_NAME" ] && [ ! -L "$H/$MARKER_NAME" ] && [ "$(cat "$H/$MARKER_NAME")" = "$MARKER_TEXT" ]; }

# home_unclaimed: LUUNA_INSTALL_HOME holds nothing except, possibly, another run's .lock.
home_unclaimed() {
  for _e in "$H"/* "$H"/.[!.]* "$H"/..?*; do
    [ -e "$_e" ] || [ -L "$_e" ] || continue
    [ "${_e##*/}" = .lock ] || return 1
  done
  return 0
}

# check_receipt_roots: refuse (exit 6) unless receipt.tsv was written for these exact roots.
check_receipt_roots() {
  if [ ! -e "$H/receipt.tsv" ] && [ ! -L "$H/receipt.tsv" ]; then
    if [ -e "$H/current" ] || [ -L "$H/current" ]; then die 6 "refusing: $H has a current version but no receipt.tsv"; fi
    return 0
  fi
  [ -f "$H/receipt.tsv" ] && [ ! -L "$H/receipt.tsv" ] || die 6 "refusing: $H/receipt.tsv is not a regular file"
  [ "$(sed -n 1p "$H/receipt.tsv")" = "$RECEIPT_HEADER" ] || die 6 "refusing: unrecognized receipt"
  [ "$(receipt_field home)" = "$H" ] || die 6 "refusing: receipt was written for home $(receipt_field home)"
  [ "$(receipt_field bin)" = "$B" ] || die 6 "refusing: receipt was written for LUUNA_INSTALL_BIN=$(receipt_field bin); rerun with that value (or uninstall with it first)"
}

# check_home_entries: refuse (exit 6) if LUUNA_INSTALL_HOME holds anything this installer does not create.
check_home_entries() {
  for _e in "$H"/* "$H"/.[!.]* "$H"/..?*; do
    [ -e "$_e" ] || [ -L "$_e" ] || continue
    _n=${_e##*/}
    case "$_n" in
      "$MARKER_NAME"|receipt.tsv|current|previous)
        [ -f "$_e" ] && [ ! -L "$_e" ] || die 6 "refusing: $_e is not a regular file" ;;
      versions) [ -d "$_e" ] && [ ! -L "$_e" ] || die 6 "refusing: $_e is not a real directory" ;;
      .lock) ;;
      .stage.*|.trash.*|.tmp.*) [ ! -L "$_e" ] || die 6 "refusing: unexpected symlink $_e" ;;
      *) die 6 "refusing: $_e was not created by this installer (remove it or choose another LUUNA_INSTALL_HOME)" ;;
    esac
  done
  [ -d "$H/versions" ] || return 0
  for _e in "$H/versions"/* "$H/versions"/.[!.]* "$H/versions"/..?*; do
    [ -e "$_e" ] || [ -L "$_e" ] || continue
    manifest_is_version "${_e##*/}" && [ -d "$_e" ] && [ ! -L "$_e" ] || die 6 "refusing: unexpected entry $_e"
  done
}

# read_pointer NAME: print the version named by $H/NAME (empty if absent).
read_pointer() {
  _f="$H/$1"
  [ -e "$_f" ] || [ -L "$_f" ] || return 0
  [ -f "$_f" ] && [ ! -L "$_f" ] || die 6 "refusing: $_f is not a regular file"
  _v=$(cat "$_f")
  manifest_is_version "$_v" || die 6 "refusing: $_f does not name a version"
  [ -d "$H/versions/$_v/bin" ] && [ ! -L "$H/versions/$_v" ] || die 6 "refusing: $_f names a missing version: $_v"
  printf '%s\n' "$_v"
}

# check_version_bins VERSION: every executable of the installed version is a regular executable
# file, so relinking to it cannot leave a public name dangling.
check_version_bins() {
  for _c in $MANIFEST_COMPONENTS; do
    for _x in $(manifest_component_exes "$_c"); do
      _f="$H/versions/$1/bin/$_x"
      [ -f "$_f" ] && [ ! -L "$_f" ] && [ -x "$_f" ] || die 6 "refusing: $_f is not a regular executable file"
    done
  done
}

# replace_file SRC DEST: rename SRC over DEST. DEST must be absent or a regular non-symlink file.
replace_file() {
  if [ -L "$2" ] || { [ -e "$2" ] && [ ! -f "$2" ]; }; then die 6 "refusing to replace $2: not a regular file"; fi
  chmod 644 "$1"
  mv -f "$1" "$2"
}
write_pointer() { # NAME VERSION|""
  if [ -z "$2" ]; then
    if [ -L "$H/$1" ] || { [ -e "$H/$1" ] && [ ! -f "$H/$1" ]; }; then die 6 "refusing to remove $H/$1"; fi
    rm -f "$H/$1"; return 0
  fi
  _t=$(mktemp "$H/.tmp.XXXXXX")
  printf '%s\n' "$2" > "$_t"
  replace_file "$_t" "$H/$1"
}

# bin_state PATH: 0 if free, 1 if a link owned by this install, 2 if anything else (conflict).
bin_state() {
  if [ -L "$1" ]; then
    _t=$(readlink "$1")
    case "$_t" in *"/../"*|*"/./"*) return 2 ;; esac
    case "$_t" in "$H"/versions/*/bin/*) [ ! -d "$1" ] || return 2; return 1 ;; esac
    return 2
  fi
  [ ! -e "$1" ] || return 2
  return 0
}
preflight_bins() { # checks every public name
  for _n in $PUBLIC_NAMES; do
    _st=0; bin_state "$B/$_n" || _st=$?
    [ "$_st" -ne 2 ] || die 6 "refusing to replace $B/$_n: it exists and was not installed by this installer"
  done
}
link_bin() { # NAME TARGET
  _l="$B/$1"
  _st=0; bin_state "$_l" || _st=$?
  [ "$_st" -ne 2 ] || die 6 "refusing to replace $_l: not owned by this installer"
  if [ "$_st" = 1 ] && [ "$(readlink "$_l")" = "$2" ]; then return 0; fi
  _tmp="$B/$BIN_TMP_PREFIX.$$.$1"
  ln -s "$2" "$_tmp" || die 6 "cannot create a temporary link in $B"
  # Re-check right before the rename to narrow the window for a racing writer (see header).
  _st=0; bin_state "$_l" || _st=$?
  if [ "$_st" = 2 ]; then rm -f "$_tmp"; die 6 "refusing to replace $_l: not owned by this installer"; fi
  mv -f "$_tmp" "$_l"
}
# relink VERSION: point every public name at its executable in VERSION, from the fixed table.
relink() {
  [ -d "$B" ] || mkdir_tracked "$B"
  for _n in $PUBLIC_NAMES; do
    link_bin "$_n" "$H/versions/$1/bin/$(link_exe "$_n")"
  done
}

write_receipt() { # CURRENT PREVIOUS
  _t=$(mktemp "$H/.tmp.XXXXXX")
  {
    printf '%s\n' "$RECEIPT_HEADER"
    printf 'home\t%s\n' "$H"
    printf 'bin\t%s\n' "$B"
    printf 'current\t%s\n' "$1"
    printf 'previous\t%s\n' "${2:--}"
    for _n in $PUBLIC_NAMES; do
      printf 'link\t%s\t%s\n' "$_n" "$H/versions/$1/bin/$(link_exe "$_n")"
    done
    sed 1d "$H/versions/$1/$VERSION_META"
  } > "$_t"
  replace_file "$_t" "$H/receipt.tsv"
}
receipt_field() { awk -F '\t' -v k="$1" '$1 == k { print $2; exit }' "$H/receipt.tsv"; }

# --- commit journal -------------------------------------------------------------------
# txn_begin: under the lock and before the first visible mutation, back up current, previous and
# receipt.tsv and record the state of every public bin name. Install sets NEW_DEST, HAD_OLD and
# VERSIONS_CREATED first. A failure in here is still a pre-mutation failure.
txn_begin() {
  TXN_DIR=$(mktemp -d "$H/.trash.XXXXXX")
  for _f in current previous receipt.tsv; do
    if [ -e "$H/$_f" ] || [ -L "$H/$_f" ]; then
      [ -f "$H/$_f" ] && [ ! -L "$H/$_f" ] || die 6 "refusing: $H/$_f is not a regular file"
      cp -p "$H/$_f" "$TXN_DIR/$_f"
    else
      : > "$TXN_DIR/$_f.absent"
    fi
  done
  : > "$TXN_DIR/links.tsv"
  for _n in $PUBLIC_NAMES; do
    _st=0; bin_state "$B/$_n" || _st=$?
    case "$_st" in
      0) printf '%s\tabsent\n' "$_n" >> "$TXN_DIR/links.tsv" ;;
      1) printf '%s\towned\t%s\n' "$_n" "$(readlink "$B/$_n")" >> "$TXN_DIR/links.tsv" ;;
      *) die 6 "refusing to replace $B/$_n: it exists and was not installed by this installer" ;;
    esac
  done
  TXN=active
}
txn_done() { TXN="done"; }

# txn_undo: replay the journal. Runs from cleanup under set -e, so every step is non-fatal and
# nothing here calls die (or a helper that does).
txn_undo() {
  # Temporary links this process may have left in the bin directory.
  for _ul in "$B/$BIN_TMP_PREFIX.$$".*; do
    if [ -L "$_ul" ]; then rm -f "$_ul" || UNDO_FAILED=1; fi
  done
  if [ -f "$TXN_DIR/links.tsv" ]; then
    while IFS="$TAB" read -r _un _us _ut; do
      [ -n "$_un" ] || continue
      _ucur=0; bin_state "$B/$_un" || _ucur=$?
      if [ "$_ucur" = 2 ]; then say "left $B/$_un in place: not owned by this install"; continue; fi
      case "$_us" in
        absent)
          if [ "$_ucur" = 1 ]; then rm -f "$B/$_un" || UNDO_FAILED=1; fi ;;
        owned)
          if [ "$_ucur" = 0 ] || [ "$(readlink "$B/$_un")" != "$_ut" ]; then
            _uu="$B/$BIN_TMP_PREFIX.$$.undo.$_un"
            rm -f "$_uu" || :
            { ln -s "$_ut" "$_uu" && mv -f "$_uu" "$B/$_un"; } || UNDO_FAILED=1
          fi ;;
        *) UNDO_FAILED=1 ;;
      esac
    done < "$TXN_DIR/links.tsv"
  else
    UNDO_FAILED=1
  fi
  for _uf in current previous receipt.tsv; do
    _ud="$H/$_uf"
    if [ -f "$TXN_DIR/$_uf" ]; then
      if [ -L "$_ud" ] || { [ -e "$_ud" ] && [ ! -f "$_ud" ]; }; then UNDO_FAILED=1
      else mv -f "$TXN_DIR/$_uf" "$_ud" || UNDO_FAILED=1; fi
    elif [ -e "$TXN_DIR/$_uf.absent" ]; then
      if [ -L "$_ud" ]; then UNDO_FAILED=1
      elif [ -f "$_ud" ]; then rm -f "$_ud" || UNDO_FAILED=1
      elif [ -e "$_ud" ]; then UNDO_FAILED=1; fi
    else
      UNDO_FAILED=1
    fi
  done
  if [ -n "$NEW_DEST" ]; then
    # Decide from the filesystem, not from flags, so an interruption between a rename and a
    # flag update cannot make undo move the wrong directory.
    if [ "$HAD_OLD" = 1 ]; then
      if [ -d "$TXN_DIR/old-version" ]; then
        if [ -d "$NEW_DEST" ] && [ ! -L "$NEW_DEST" ]; then mv "$NEW_DEST" "$TXN_DIR/new-version" || UNDO_FAILED=1; fi
        if [ -e "$NEW_DEST" ] || [ -L "$NEW_DEST" ]; then UNDO_FAILED=1
        else mv "$TXN_DIR/old-version" "$NEW_DEST" || UNDO_FAILED=1; fi
      fi
    elif [ -d "$NEW_DEST" ] && [ ! -L "$NEW_DEST" ]; then
      mv "$NEW_DEST" "$TXN_DIR/new-version" || UNDO_FAILED=1
    fi
    if [ "$VERSIONS_CREATED" = 1 ] && [ -d "$H/versions" ]; then rmdir "$H/versions" 2>/dev/null || :; fi
  fi
  if [ "$UNDO_FAILED" = 0 ]; then
    UNDONE=1
    say "restored the previous installation"
  else
    say "could not fully restore the previous installation; rerun install or --rollback (backups kept in $TXN_DIR)"
  fi
}

gc_versions() { # KEEP...
  for _e in "$H/versions"/*; do
    [ -d "$_e" ] && [ ! -L "$_e" ] || continue
    _n=${_e##*/}
    case " $* " in *" $_n "*) continue ;; esac
    if manifest_is_version "$_n"; then rm -rf "$_e"; fi
  done
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{ print $1 }'
  elif command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1" | awk '{ print $1 }'
  else die 2 "need sha256sum or shasum"; fi
}

fetch() { # URL DEST
  if command -v curl >/dev/null 2>&1; then
    curl --proto '=https' --proto-redir '=https' --tlsv1.2 -fsSL -o "$2" "$1"
  elif command -v wget >/dev/null 2>&1; then
    wget --https-only -q -O "$2" "$1"
  else
    echo "$PROG: neither curl nor wget is available" >&2; return 1
  fi
}

# inspect_archive ARCHIVE EXE...: allow-list the listing before anything is extracted.
inspect_archive() {
  _a=$1; shift
  _names=$(tar -tzf "$_a" 2>/dev/null) || { echo "cannot list archive" >&2; return 1; }
  _verbose=$(tar -tzvf "$_a" 2>/dev/null) || { echo "cannot list archive" >&2; return 1; }
  [ -n "$_names" ] || { echo "empty archive" >&2; return 1; }
  if [ "$(printf '%s\n' "$_names" | wc -l)" -ne "$(printf '%s\n' "$_verbose" | wc -l)" ]; then
    echo "ambiguous archive listing" >&2; return 1
  fi
  if [ -n "$(printf '%s\n' "$_names" | sort | uniq -d)" ]; then echo "duplicate entries" >&2; return 1; fi
  while IFS= read -r _entry; do
    case "$_entry" in
      bin|bin/) continue ;;
    esac
    _hit=0
    for _x in "$@"; do [ "$_entry" != "bin/$_x" ] || _hit=1; done
    [ "$_hit" = 1 ] || { echo "unexpected entry: $_entry" >&2; return 1; }
  done <<EOF
$_names
EOF
  for _x in "$@"; do
    printf '%s\n' "$_names" | grep -Fx -e "bin/$_x" >/dev/null || { echo "missing executable: bin/$_x" >&2; return 1; }
  done
  # Only regular files and directories: no symlinks, hardlinks, devices, fifos.
  printf '%s\n' "$_verbose" | awk '
    { c = substr($0, 1, 1); if ((c != "-" && c != "d") || index($0, " -> ") || index($0, " link to ")) bad = 1 }
    END { exit bad }' || { echo "archive contains links or special files" >&2; return 1; }
}

detect_platform() {
  _s=${LUUNA_INSTALL_UNAME_S:-$(uname -s)}
  _m=${LUUNA_INSTALL_UNAME_M:-$(uname -m)}
  case "$_s" in
    Darwin) OS=darwin ;;
    Linux) OS=linux ;;
    *) die 3 "unsupported platform: $_s/$_m. This installer supports macOS and Linux on x86_64 and arm64. Windows is not supported by this installer." ;;
  esac
  case "$_m" in
    x86_64|amd64) ARCH=amd64 ;;
    arm64|aarch64) ARCH=arm64 ;;
    *) die 3 "unsupported platform: $_s/$_m (supported architectures: x86_64, arm64)" ;;
  esac
}

# note_path_shadowing: after an install, name any other `luuna` or `lu` that PATH finds first.
# Only prints; it never changes the other entry.
note_path_shadowing() {
  for _n in luuna lu; do
    _found=$(command -v "$_n" 2>/dev/null) || _found=""
    case "$_found" in /*) ;; *) continue ;; esac
    _fd=${_found%/*}; [ -n "$_fd" ] || _fd=/
    _fd=$(cd "$_fd" 2>/dev/null && pwd -P) || _fd=${_found%/*}
    [ "$_fd" != / ] || _fd=""
    if [ "$_fd/${_found##*/}" != "$B/$_n" ]; then
      say "note: \`$_n\` on PATH runs $_found, not $B/$_n; this installer left $_found unchanged"
    fi
  done
}

# --- uninstall --------------------------------------------------------------------
do_uninstall() {
  if [ ! -e "$H" ]; then say "nothing installed at $H"; exit 0; fi
  marker_ok || die 6 "refusing: $H has no luuna-install marker ($MARKER_NAME); not removing it"
  [ -f "$H/receipt.tsv" ] && [ ! -L "$H/receipt.tsv" ] || die 6 "refusing: $H has no receipt.tsv; remove it manually after checking its contents"
  check_receipt_roots
  check_home_entries
  _links=$(awk -F '\t' '$1 == "link" { print $2 }' "$H/receipt.tsv")
  if [ "$dry_run" = 1 ]; then
    for _n in $_links; do
      case " $PUBLIC_NAMES " in *" $_n "*) say "would remove link $B/$_n" ;; esac
    done
    say "would remove $H"; exit 0
  fi
  acquire_lock
  for _n in $_links; do
    case " $PUBLIC_NAMES " in *" $_n "*) ;; *) say "skipping non-public receipt entry: $_n"; continue ;; esac
    _st=0; bin_state "$B/$_n" || _st=$?
    if [ "$_st" = 1 ]; then rm -f "$B/$_n"; say "removed $B/$_n"
    elif [ "$_st" = 2 ]; then say "left $B/$_n in place: not owned by this install"; fi
  done
  if [ -d "$H/versions" ]; then
    for _e in "$H/versions"/*; do
      if [ -d "$_e" ] && [ ! -L "$_e" ] && manifest_is_version "${_e##*/}"; then rm -rf "$_e"; fi
    done
    rmdir "$H/versions"
  fi
  for _e in "$H"/.stage.* "$H"/.trash.* "$H"/.tmp.*; do if [ -e "$_e" ]; then rm -rf "$_e"; fi; done
  rm -f "$H/receipt.tsv" "$H/current" "$H/previous" "$H/$MARKER_NAME"
  rm -rf "$H/.lock"; LOCKED=0
  rmdir "$H" || die 6 "could not remove $H (not empty)"
  say "uninstalled; project stores (.luuna/) were not touched"
  exit 0
}

# --- rollback -----------------------------------------------------------------------
do_rollback() {
  if [ ! -d "$H" ] || ! marker_ok; then die 2 "nothing installed at $H"; fi
  check_home_entries
  check_receipt_roots
  _cur=$(read_pointer current) || exit $?
  _prev=$(read_pointer previous) || exit $?
  [ -n "$_cur" ] || die 2 "nothing installed at $H"
  [ -n "$_prev" ] || die 2 "no previous version to roll back to"
  check_version_bins "$_prev"
  preflight_bins
  if [ "$dry_run" = 1 ]; then say "would roll back from $_cur to $_prev"; exit 0; fi
  acquire_lock
  # Authoritative checks, under the lock.
  marker_ok || die 6 "refusing: $H has no luuna-install marker ($MARKER_NAME)"
  check_home_entries
  check_receipt_roots
  _cur=$(read_pointer current) || exit $?
  _prev=$(read_pointer previous) || exit $?
  [ -n "$_cur" ] || die 2 "nothing installed at $H"
  [ -n "$_prev" ] || die 2 "no previous version to roll back to"
  check_version_bins "$_prev"
  preflight_bins
  txn_begin
  write_pointer previous "$_cur"
  write_pointer current "$_prev"
  relink "$_prev"
  write_receipt "$_prev" "$_cur"
  txn_done
  say "rolled back: current $_prev, previous $_cur"
  exit 0
}

[ "$mode" != uninstall ] || do_uninstall
[ "$mode" != rollback ] || do_rollback

# --- install ------------------------------------------------------------------------
if [ -n "$version" ]; then
  manifest_is_version "$version" || die 2 "invalid release version: $version"
  index="$script_dir/releases/INDEX.tsv"
  [ -f "$index" ] && [ ! -L "$index" ] || die 2 "release index missing: packaging/installer/releases/INDEX.tsv"
  if [ -z "$(sed 1d "$index")" ]; then
    die 2 "No releases are published yet: packaging/installer/releases/INDEX.tsv lists none, so there is no release $version to install."
  fi
  _want=$(awk -F '\t' -v v="$version" 'NR > 1 && $1 == v { print $2; exit }' "$index")
  [ -n "$_want" ] || die 2 "release $version is not listed in packaging/installer/releases/INDEX.tsv"
  manifest="$script_dir/releases/$version/manifest.tsv"
  [ -f "$manifest" ] && [ ! -L "$manifest" ] || die 2 "no manifest for release $version (expected packaging/installer/releases/$version/manifest.tsv)"
  [ "$(sha256_of "$manifest")" = "$_want" ] || die 2 "packaging/installer/releases/$version/manifest.tsv does not match its INDEX.tsv hash"
fi
[ -f "$manifest" ] || die 2 "manifest not found: $manifest"
manifest_validate "$manifest" || die 2 "invalid manifest: $manifest"
release_v=$(manifest_release "$manifest")
[ -z "$version" ] || [ "$release_v" = "$version" ] || die 2 "manifest release $release_v does not match $version"
if [ -n "$asset_dir" ]; then
  [ -d "$asset_dir" ] || die 2 "--asset-dir is not a directory: $asset_dir"
  asset_dir=$(cd "$asset_dir" && pwd -P)
fi

detect_platform
rows=$(manifest_select "$manifest" "$OS" "$ARCH") || die 3 "unsupported platform: release $release_v has no complete set of assets for $OS/$ARCH"
link_plan=""
for _n in $PUBLIC_NAMES; do link_plan="${link_plan:+$link_plan, }$_n -> $(link_exe "$_n")"; done

old_cur=""
if [ -e "$H" ]; then
  # Read-only preflight, so --dry-run and early refusals write nothing. The checks repeated
  # under the lock below are the authoritative ones.
  if marker_ok; then
    check_home_entries
    check_receipt_roots
    old_cur=$(read_pointer current) || exit $?
    read_pointer previous >/dev/null
  elif ! home_unclaimed; then
    die 6 "refusing: $H exists, is not empty, and has no luuna-install marker ($MARKER_NAME)"
  fi
fi
preflight_bins

if [ "$old_cur" = "$release_v" ]; then say "release $release_v is already the current version"; exit 0; fi

if [ "$dry_run" = 1 ]; then
  say "dry run: release $release_v for $OS/$ARCH into $H (links in $B)"
  printf '%s\n' "$rows" | while IFS="$TAB" read -r _k _c _cv _o _a _u _s _f _x; do
    if [ -n "$asset_dir" ]; then _src="$asset_dir/$_f"; else _src=$_u; fi
    say "would install $_c $_cv ($_x) from $_src"
  done
  say "would link: $link_plan"
  exit 0
fi

# Writes begin here.
if [ ! -d "$H" ]; then mkdir_tracked "$H"; fi
# Lock first: only the lock holder may claim the home by writing the marker.
acquire_lock
if ! marker_ok; then
  home_unclaimed || die 6 "refusing: $H exists, is not empty, and has no luuna-install marker ($MARKER_NAME)"
  printf '%s\n' "$MARKER_TEXT" > "$H/$MARKER_NAME"
  CREATED_MARKER=1
fi
for _e in "$H"/.stage.* "$H"/.trash.* "$H"/.tmp.*; do if [ -e "$_e" ]; then rm -rf "$_e"; fi; done
check_home_entries
check_receipt_roots
old_cur=$(read_pointer current) || exit $?
read_pointer previous >/dev/null
if [ "$old_cur" = "$release_v" ]; then say "release $release_v is already the current version"; exit 0; fi
preflight_bins

STAGE=$(mktemp -d "$H/.stage.XXXXXX")
mkdir "$STAGE/bin" "$STAGE/dl"
printf 'luuna-install-version\t1\nrelease\t%s\n' "$release_v" > "$STAGE/$VERSION_META"
while IFS="$TAB" read -r _k comp cver os arch url sha file exes; do
  dl="$STAGE/dl/$file"
  if [ -n "$asset_dir" ]; then
    src="$asset_dir/$file"
    [ -f "$src" ] && [ ! -L "$src" ] || die 7 "asset not found in --asset-dir: $file"
    cp "$src" "$dl" || die 7 "cannot read asset: $src"
  else
    case "$url" in https://*) ;; *) die 2 "refusing non-https url: $url" ;; esac
    say "fetching $url"
    fetch "$url" "$dl" || die 7 "download failed: $url"
  fi
  got=$(sha256_of "$dl")
  [ "$got" = "$sha" ] || die 4 "checksum mismatch for $file: expected $sha, got $got"
  exe_list=$(printf '%s' "$exes" | tr ',' ' ')
  # shellcheck disable=SC2086 # exe_list is a validated, space-separated list of executable names
  why=$(inspect_archive "$dl" $exe_list 2>&1) || die 5 "unsafe archive $file: $why"
  x="$STAGE/x.$comp"
  mkdir "$x"
  tar -xozf "$dl" -C "$x" 2>/dev/null || die 5 "cannot extract $file"
  count=$(find "$x" -print | wc -l | tr -d ' ')
  # shellcheck disable=SC2086 # split the validated list to count its entries
  set -- $exe_list
  [ "$count" -eq $(($# + 2)) ] || die 5 "unsafe archive $file: extraction produced unexpected paths"
  for exe in $exe_list; do
    f="$x/bin/$exe"
    [ -f "$f" ] && [ ! -L "$f" ] && [ -x "$f" ] || die 5 "unsafe archive $file: bin/$exe is not a regular executable file"
    [ ! -e "$STAGE/bin/$exe" ] || die 5 "duplicate executable across components: $exe"
    mv "$f" "$STAGE/bin/$exe"
    chmod 755 "$STAGE/bin/$exe"
  done
  rm -rf "$x" "$dl"
  printf 'component\t%s\t%s\t%s\t%s\t%s\t%s\n' "$comp" "$cver" "$os" "$arch" "$sha" "$exes" >> "$STAGE/$VERSION_META"
  say "verified $comp $cver"
done <<EOF
$rows
EOF
rmdir "$STAGE/dl"
# Every link target of the table must have been staged (the manifest rules already require it).
for _n in $PUBLIC_NAMES; do
  _x=$(link_exe "$_n")
  [ -f "$STAGE/bin/$_x" ] && [ ! -L "$STAGE/bin/$_x" ] || die 5 "release $release_v lacks bin/$_x for $_n"
done

# Swap: everything staged and verified. Journal first, so any failure below is undone.
NEW_DEST="$H/versions/$release_v"
if [ -e "$NEW_DEST" ] || [ -L "$NEW_DEST" ]; then
  [ -d "$NEW_DEST" ] && [ ! -L "$NEW_DEST" ] || die 6 "refusing: $NEW_DEST is not a real directory"
  HAD_OLD=1
fi
[ -d "$H/versions" ] || VERSIONS_CREATED=1
txn_begin
[ "$VERSIONS_CREATED" = 0 ] || mkdir "$H/versions"
[ "$HAD_OLD" = 0 ] || mv "$NEW_DEST" "$TXN_DIR/old-version"
mv "$STAGE" "$NEW_DEST"
STAGE=""
new_prev=$old_cur
write_pointer previous "$new_prev"
write_pointer current "$release_v"
relink "$release_v"
write_receipt "$release_v" "$new_prev"
txn_done
# Committed. Garbage collection must not undo it or turn it into a failure.
# shellcheck disable=SC2086 # new_prev is empty or one validated version
gc_versions "$release_v" $new_prev || say "warning: could not remove old versions under $H/versions"
say "installed release $release_v (${new_prev:+previous $new_prev; }links in $B: $link_plan)"
case ":$PATH:" in *":$B:"*) ;; *) say "note: $B is not on PATH" ;; esac
note_path_shadowing
