#!/bin/sh
# Re-verify a Luuna Roots release archive before anyone extracts or ships it.
#
#   sh scripts/release/check-archive.sh [--no-exec] ARCHIVE
#
# Checks, in order, without extracting first:
#   1. the file name is luuna-roots_<version>_<os>_<arch>.tar.gz
#   2. a sibling SHA256SUMS exists as a regular, non-symlink file, lists the
#      archive exactly once with a 64-hex digest, and that digest matches
#      (required; --no-exec does not relax it)
#   3. the listing holds exactly bin/luuna-roots and bin/lrt, each a regular
#      file with mode 0755 (no directories, links, devices, or extra entries)
# Only then it extracts into a private temp dir, re-checks the files, and, when
# the archive matches the host platform (and --no-exec is not given), runs
# --version and --help through both names and requires identical output so the
# alias is known to preserve arguments.
set -eu

usage() {
	printf 'Usage: sh scripts/release/check-archive.sh [--no-exec] ARCHIVE\n'
}

fail() {
	printf 'check-archive.sh: %s\n' "$1" >&2
	exit "${2:-1}"
}

no_exec=false
archive=""
while [ "$#" -gt 0 ]; do
	case "$1" in
	--no-exec)
		no_exec=true
		shift
		;;
	-h | --help)
		usage
		exit 0
		;;
	--)
		shift
		break
		;;
	-*)
		usage >&2
		fail "unknown option: $1" 2
		;;
	*)
		[ -z "$archive" ] || fail "only one ARCHIVE may be given" 2
		archive=$1
		shift
		;;
	esac
done
if [ "$#" -gt 0 ]; then
	[ -z "$archive" ] && [ "$#" -eq 1 ] || fail "only one ARCHIVE may be given" 2
	archive=$1
fi
[ -n "$archive" ] || {
	usage >&2
	exit 2
}
[ -f "$archive" ] && [ ! -L "$archive" ] || fail "not a regular file: $archive" 2

name=$(basename "$archive")
if ! printf '%s\n' "$name" | grep -Eq '^luuna-roots_[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?_(darwin|linux)_(arm64|amd64)\.tar\.gz$'; then
	fail "unexpected archive name: $name"
fi
platform=${name%.tar.gz}
arch=${platform##*_}
platform=${platform%_*}
os=${platform##*_}

if command -v sha256sum >/dev/null 2>&1; then
	sha256() { sha256sum "$1" | awk '{print $1}'; }
else
	sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
fi

# SHA256SUMS is mandatory, independent of --no-exec.
sums="$(dirname "$archive")/SHA256SUMS"
[ ! -L "$sums" ] || fail "SHA256SUMS must not be a symlink: $sums"
[ -e "$sums" ] || fail "missing required SHA256SUMS next to the archive: $sums"
[ -f "$sums" ] || fail "SHA256SUMS is not a regular file: $sums"
listed=$(awk -v n="$name" '$2 == n || $2 == "*" n {c++} END {print c+0}' "$sums") || fail "cannot read $sums"
[ "$listed" -eq 1 ] || fail "$name must be listed exactly once in $sums (found $listed)"
expected=$(awk -v n="$name" '$2 == n || $2 == "*" n {print $1}' "$sums") || fail "cannot read $sums"
case "$expected" in
"" | *[!0-9a-f]*) fail "malformed SHA256SUMS entry for $name" ;;
esac
[ "${#expected}" -eq 64 ] || fail "malformed SHA256SUMS entry for $name"
actual=$(sha256 "$archive")
[ "$expected" = "$actual" ] || fail "checksum mismatch for $name"
printf 'ok   sha256 %s\n' "$actual"

names=$(tar -tzf "$archive") || fail "cannot list archive"
expected_names=$(printf 'bin/lrt\nbin/luuna-roots')
sorted_names=$(printf '%s\n' "$names" | LC_ALL=C sort)
[ "$sorted_names" = "$expected_names" ] || fail "archive entries must be exactly bin/luuna-roots and bin/lrt; got: $(printf '%s' "$names" | tr '\n' ' ')"

listing=$(tar -tzvf "$archive") || fail "cannot list archive"
count=0
while IFS= read -r line; do
	count=$((count + 1))
	case "$line" in
	-rwxr-xr-x\ *) ;;
	*) fail "entry is not a regular 0755 file: $line" ;;
	esac
	case "$line" in
	*" -> "* | *" link to "*) fail "link entry rejected: $line" ;;
	esac
done <<EOF
$listing
EOF
[ "$count" -eq 2 ] || fail "expected 2 archive entries, found $count"
printf 'ok   entries bin/luuna-roots bin/lrt (regular, 0755)\n'

work=$(mktemp -d "${TMPDIR:-/tmp}/luuna-roots-check.XXXXXX")
trap 'rm -rf "$work"' EXIT INT TERM HUP
tar -xzf "$archive" -C "$work" bin/luuna-roots bin/lrt
for exe in luuna-roots lrt; do
	path="$work/bin/$exe"
	[ -f "$path" ] && [ ! -L "$path" ] && [ -x "$path" ] || fail "extracted $exe is not an executable regular file"
done

case "$(uname -s)" in
Darwin) host_os=darwin ;;
Linux) host_os=linux ;;
*) host_os=unknown ;;
esac
case "$(uname -m)" in
arm64 | aarch64) host_arch=arm64 ;;
x86_64 | amd64) host_arch=amd64 ;;
*) host_arch=unknown ;;
esac

if [ "$no_exec" = true ]; then
	printf 'skip exec (--no-exec)\n'
elif [ "$os" != "$host_os" ] || [ "$arch" != "$host_arch" ]; then
	printf 'skip exec (archive %s/%s, host %s/%s)\n' "$os" "$arch" "$host_os" "$host_arch"
else
	for args in "--version" "--help" "--version --json"; do
		# shellcheck disable=SC2086 # args are fixed literals split on purpose
		primary=$("$work/bin/luuna-roots" $args 2>&1) || fail "luuna-roots $args exited non-zero"
		# shellcheck disable=SC2086
		alias_out=$("$work/bin/lrt" $args 2>&1) || fail "lrt $args exited non-zero"
		[ "$primary" = "$alias_out" ] || fail "lrt $args output differs from luuna-roots $args"
	done
	printf 'ok   exec --version/--help through luuna-roots and lrt: %s\n' "$("$work/bin/luuna-roots" --version)"
fi
printf 'ok   %s\n' "$name"
