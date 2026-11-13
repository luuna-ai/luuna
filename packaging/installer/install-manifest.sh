# Luuna release manifest: parse and validate (POSIX sh + awk; no jq, no Python, no Node).
# shellcheck shell=sh
#
# Sourced by packaging/installer/install.sh and packaging/installer/tests/check-manifests.sh.
# Nothing runs when this file is sourced: it only defines MANIFEST_COMPONENTS and functions.
#
# Format (packaging/installer/releases/FORMAT.md), tab-separated, LF line endings, printable ASCII
# only, final newline:
#   luuna-release-manifest<TAB>1
#   release<TAB><version>
#   asset<TAB><component><TAB><version><TAB><os><TAB><arch><TAB><url><TAB><sha256><TAB><file><TAB><exe,exe,...>
# Rules for each asset row:
#   component  one of MANIFEST_COMPONENTS
#   version    equal to the release line
#   os, arch   darwin or linux; amd64 or arm64
#   sha256     64 lowercase hex characters
#   file       exactly <component>_<version>_<os>_<arch>.tar.gz
#   url        exactly https://github.com/luuna-ai/luuna/releases/download/v<version>/<file>
#   exes       each executable of the component exactly once, nothing else, any order
# One row per component/os/arch. Link names (luuna, lu) never appear in a manifest.
#
# Functions:
#   manifest_validate FILE        exit 0 if valid; diagnostics on stderr
#   manifest_release FILE         print the release version (FILE must be valid)
#   manifest_select FILE OS ARCH  print the asset rows for OS/ARCH in manifest order; return 3 unless
#                                 every component in MANIFEST_COMPONENTS has a row for that platform
#   manifest_component_exes COMP  print the executables a component's archive carries
#   manifest_is_version STRING    exit 0 if STRING is a valid version identifier

MANIFEST_COMPONENTS="luuna"

manifest_component_exes() {
  case "$1" in
    luuna) echo "luuna-tui luuna-runtime luuna-sbx" ;;
    *) return 1 ;;
  esac
}

manifest_is_version() {
  printf '%s\n' "$1" | awk '
    NR > 1 { bad = 1 }
    END { exit (bad || NR != 1) }
    length($0) > 64 || $0 !~ /^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z][0-9A-Za-z.-]*)?$/ { bad = 1 }'
}

# _manifest_allowed: "comp=exe,exe;comp=exe,..." built from the component table above, so the
# validator and the installer read the same list.
_manifest_allowed() {
  for _mc in $MANIFEST_COMPONENTS; do
    printf '%s=%s;' "$_mc" "$(manifest_component_exes "$_mc" | tr ' ' ',')"
  done
}

manifest_validate() {
  _mf=$1
  if [ ! -f "$_mf" ]; then echo "manifest: not a regular file: $_mf" >&2; return 1; fi
  if [ ! -s "$_mf" ]; then echo "manifest: empty file: $_mf" >&2; return 1; fi
  _cr=$(printf '\r')
  if grep "$_cr" "$_mf" >/dev/null 2>&1; then
    echo "manifest: CRLF or CR line endings are not allowed: $_mf" >&2; return 1
  fi
  # Only printable ASCII, tab and newline.
  _bad=$(LC_ALL=C tr -d '\011\012\040-\176' < "$_mf" | wc -c | tr -d ' ')
  if [ "$_bad" != 0 ]; then echo "manifest: non-printable or non-ASCII bytes: $_mf" >&2; return 1; fi
  if [ -n "$(tail -c 1 "$_mf")" ]; then echo "manifest: must end with a newline: $_mf" >&2; return 1; fi
  LC_ALL=C awk -F '\t' -v allowed_list="$(_manifest_allowed)" '
    function fail(msg) { printf "manifest: line %d: %s\n", NR, msg > "/dev/stderr"; err = 1 }
    function isver(v) { return length(v) <= 64 && v ~ /^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z][0-9A-Za-z.-]*)?$/ }
    BEGIN {
      nc = split(allowed_list, comps, ";")
      for (i = 1; i <= nc; i++) {
        if (comps[i] == "") continue
        eq = index(comps[i], "=")
        allowed[substr(comps[i], 1, eq - 1)] = substr(comps[i], eq + 1)
      }
      prefix = "https://github.com/luuna-ai/luuna/releases/download/"
    }
    NR == 1 { if ($0 != "luuna-release-manifest\t1") fail("header must be: luuna-release-manifest<TAB>1"); next }
    NR == 2 {
      if (NF != 2 || $1 != "release" || !isver($2)) fail("second line must be: release<TAB><version>")
      else release = $2
      next
    }
    {
      if (NF != 9) { fail("asset row must have exactly 9 tab-separated fields"); next }
      for (i = 1; i <= 9; i++) if ($i == "") { fail("empty field " i); next }
      if ($1 != "asset") { fail("unknown row type: " $1); next }
      comp = $2; ver = $3; os = $4; arch = $5; url = $6; sha = $7; file = $8; exes = $9
      if (!(comp in allowed)) { fail("unknown component: " comp); next }
      if (!isver(ver)) fail("bad asset version: " ver)
      else if (release != "" && ver != release) fail("asset version " ver " does not match the release line " release)
      if (os != "darwin" && os != "linux") fail("unsupported os: " os)
      if (arch != "arm64" && arch != "amd64") fail("unsupported arch: " arch)
      if (length(sha) != 64 || sha ~ /[^0-9a-f]/) fail("sha256 must be 64 lowercase hex characters")
      want_file = comp "_" ver "_" os "_" arch ".tar.gz"
      if (file != want_file) fail("archive file name must be " want_file)
      want_url = prefix "v" ver "/" want_file
      if (url != want_url) fail("url must be " want_url)
      n = split(exes, e, ",")
      m = split(allowed[comp], ok, ",")
      for (i = 1; i <= m; i++) okset[NR, ok[i]] = 1
      for (i = 1; i <= n; i++) {
        if ((NR, e[i]) in seen) fail("duplicate executable: " e[i])
        seen[NR, e[i]] = 1
        if (!((NR, e[i]) in okset)) fail("not an executable of " comp ": " e[i])
      }
      for (i = 1; i <= m; i++) if (!((NR, ok[i]) in seen)) fail("executables must include " ok[i])
      key = comp "/" os "/" arch
      if (key in rows) fail("duplicate asset row for " key)
      rows[key] = 1
      assets++
    }
    END {
      if (NR < 2) { printf "manifest: missing header or release line\n" > "/dev/stderr"; err = 1 }
      else if (assets == 0) { printf "manifest: no asset rows\n" > "/dev/stderr"; err = 1 }
      exit err
    }' "$_mf"
}

manifest_release() {
  awk -F '\t' 'NR == 2 && $1 == "release" { print $2; exit }' "$1"
}

manifest_select() {
  _mf=$1 _os=$2 _arch=$3
  _rows=$(awk -F '\t' -v os="$_os" -v arch="$_arch" 'NR > 2 && $1 == "asset" && $4 == os && $5 == arch' "$_mf")
  for _c in $MANIFEST_COMPONENTS; do
    if ! printf '%s\n' "$_rows" | awk -F '\t' -v c="$_c" '$2 == c { found = 1 } END { exit !found }'; then
      echo "manifest: no $_c asset for $_os/$_arch" >&2
      return 3
    fi
  done
  printf '%s\n' "$_rows"
}
