#!/bin/sh
# Validate packaging/installer/releases/: every manifest is valid, matches its INDEX.tsv hash, versions
# are unique, and (with --base REF) released files are append-only relative to REF.
#
# Usage: sh packaging/installer/tests/check-manifests.sh [--base REF] [--root DIR]
#   --root DIR  repository root to check (default: this checkout; used by the tests)
set -eu

script_dir=$(cd "$(dirname "$0")" && pwd -P)
# The repository root is three levels above packaging/installer/tests/.
root=$(cd "$script_dir/../../.." && pwd -P)
base=""
while [ $# -gt 0 ]; do
  case "$1" in
    --base) [ $# -ge 2 ] || { echo "check-manifests: --base needs a ref" >&2; exit 2; }; base=$2; shift 2 ;;
    --root)
      [ $# -ge 2 ] || { echo "check-manifests: --root needs a dir" >&2; exit 2; }
      root=$(cd "$2" && pwd -P) || { echo "check-manifests: --root is not a directory: $2" >&2; exit 2; }
      shift 2 ;;
    -h|--help) sed -n '2,7p' "$0"; exit 0 ;;
    *) echo "check-manifests: unknown argument: $1" >&2; exit 2 ;;
  esac
done

# The manifest library sits beside the installer, one level above this script; its path is only
# known at run time (SC1091).
# shellcheck disable=SC1091
. "$script_dir/../install-manifest.sh"

relpath=packaging/installer/releases
rel="$root/$relpath"
index="$rel/INDEX.tsv"
tab=$(printf '\t')
errors=0
err() { echo "check-manifests: $*" >&2; errors=$((errors + 1)); }

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}'
  else shasum -a 256 "$1" | awk '{print $1}'; fi
}

[ -f "$index" ] || { echo "check-manifests: missing $index" >&2; exit 1; }
cr=$(printf '\r')
if grep "$cr" "$index" >/dev/null 2>&1; then err "INDEX.tsv has CR line endings"; fi
if [ -n "$(tail -c 1 "$index")" ]; then err "INDEX.tsv must end with a newline"; fi
if [ "$(sed -n 1p "$index")" != "#release-version${tab}manifest-sha256" ]; then
  err "INDEX.tsv header must be: #release-version<TAB>manifest-sha256"
fi

seen=""
line_no=1
body=$(sed 1d "$index")
[ -n "$body" ] && while IFS= read -r line; do
  line_no=$((line_no + 1))
  v=${line%%"$tab"*}
  h=${line#*"$tab"}
  if [ "$v" = "$line" ] || [ -z "$v" ] || [ -z "$h" ] || [ "${h#*"$tab"}" != "$h" ]; then
    err "INDEX.tsv line $line_no: expected <version><TAB><sha256>"; continue
  fi
  manifest_is_version "$v" || { err "INDEX.tsv line $line_no: bad version: $v"; continue; }
  case " $seen " in *" $v "*) err "INDEX.tsv line $line_no: duplicate version $v"; continue ;; esac
  seen="$seen $v"
  case "$h" in *[!0-9a-f]*) err "INDEX.tsv line $line_no: bad sha256"; continue ;; esac
  [ ${#h} -eq 64 ] || { err "INDEX.tsv line $line_no: bad sha256 length"; continue; }
  mf="$rel/$v/manifest.tsv"
  if [ -L "$rel/$v" ] || [ -L "$mf" ] || [ ! -f "$mf" ]; then err "INDEX.tsv lists $v but $relpath/$v/manifest.tsv is missing or not a regular file"; continue; fi
  if ! manifest_validate "$mf"; then err "$v/manifest.tsv is invalid"; continue; fi
  [ "$(manifest_release "$mf")" = "$v" ] || err "$v/manifest.tsv release line does not match its directory"
  [ "$(sha256_of "$mf")" = "$h" ] || err "$v/manifest.tsv does not match its INDEX.tsv hash"
done <<EOF
$body
EOF

# Every release directory must be indexed and contain only manifest.tsv.
for d in "$rel"/* "$rel"/.[!.]* "$rel"/..?*; do
  [ -e "$d" ] || [ -L "$d" ] || continue
  name=${d##*/}
  case "$name" in FORMAT.md|INDEX.tsv) continue ;; esac
  if [ -L "$d" ] || [ ! -d "$d" ]; then err "unexpected entry in $relpath/: $name"; continue; fi
  case " $seen " in *" $name "*) ;; *) err "$relpath/$name is not listed in INDEX.tsv" ;; esac
  for f in "$d"/* "$d"/.[!.]* "$d"/..?*; do
    [ -e "$f" ] || [ -L "$f" ] || continue
    [ "${f##*/}" = manifest.tsv ] || err "unexpected file $relpath/$name/${f##*/}"
  done
done

if [ -n "$base" ]; then
  git -C "$root" rev-parse --verify --quiet "$base^{commit}" >/dev/null || { echo "check-manifests: unknown base ref: $base" >&2; exit 2; }
  changes=$(git -C "$root" diff --no-renames --name-status "$base" -- "$relpath/")
  while IFS="$tab" read -r status path; do
    [ -n "$status" ] || continue
    case "$path" in
      "$relpath/FORMAT.md") ;;
      "$relpath/INDEX.tsv")
        case "$status" in
          A) ;;
          M)
            old=$(git -C "$root" show "$base:$relpath/INDEX.tsv")
            n=$(printf '%s\n' "$old" | wc -l | tr -d ' ')
            if [ "$(head -n "$n" "$index")" != "$old" ]; then err "INDEX.tsv may only gain appended lines relative to $base"; fi ;;
          *) err "INDEX.tsv may not be removed ($status)" ;;
        esac ;;
      "$relpath"/*/*) [ "$status" = A ] || err "published release file changed ($status): $path" ;;
      *) err "unexpected change ($status): $path" ;;
    esac
  done <<EOF
$changes
EOF
fi

if [ "$errors" -ne 0 ]; then echo "check-manifests: $errors problem(s)" >&2; exit 1; fi
echo "check-manifests: ok ($(printf '%s' "$seen" | wc -w | tr -d ' ') release(s))"
