#!/bin/sh
# Build and package the public Luuna Roots executables for one platform.
#
#   sh scripts/release/package.sh --version VERSION --out DIR [--target TRIPLE]
#
# Writes into DIR (which must be absent or empty):
#   luuna-roots_<VERSION>_<os>_<arch>.tar.gz  entries: bin/luuna-roots, bin/lrt (0755, regular files only)
#   SHA256SUMS                                checksum of the archive
#   provenance.json                           self-reported build metadata (not an attestation);
#                                             worktree_dirty/untracked_present are null when unknown
#   LICENSE, UPSTREAM.md                      attribution that accompanies the archive
#
# The archive VERSION is a release label only. The binaries report the
# workspace version from Cargo.toml (`luuna-roots --version`); bumping it is an
# owner release action. Nothing here publishes anything.
set -eu

usage() {
	cat <<'EOF'
Usage: sh scripts/release/package.sh --version VERSION --out DIR [--target TRIPLE]

  --version VERSION  Archive version label (semver, e.g. 0.1.0 or 0.1.0-rc.1)
  --out DIR          Output directory; must not exist or must be empty
  --target TRIPLE    One of aarch64-apple-darwin, x86_64-apple-darwin,
                     x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu
                     (default: host triple from rustc -vV)
  -h, --help         Show this help
EOF
}

die() {
	printf 'package.sh: %s\n' "$1" >&2
	exit "${2:-1}"
}

version=""
out=""
target=""
while [ "$#" -gt 0 ]; do
	case "$1" in
	--version)
		[ "$#" -ge 2 ] || die "--version needs a value" 2
		version=$2
		shift 2
		;;
	--out)
		[ "$#" -ge 2 ] || die "--out needs a value" 2
		out=$2
		shift 2
		;;
	--target)
		[ "$#" -ge 2 ] || die "--target needs a value" 2
		target=$2
		shift 2
		;;
	-h | --help)
		usage
		exit 0
		;;
	*)
		usage >&2
		die "unknown argument: $1" 2
		;;
	esac
done

[ -n "$version" ] || die "--version is required" 2
[ -n "$out" ] || die "--out is required" 2

# Semver core with optional prerelease; no build metadata, no leading 'v'.
if ! printf '%s\n' "$version" | grep -Eq '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-[0-9A-Za-z]+(\.[0-9A-Za-z]+)*)?$'; then
	die "invalid --version '$version' (expected semver like 1.2.3 or 1.2.3-rc.1)" 2
fi
[ "${#version}" -le 64 ] || die "--version is too long" 2

case "$out" in
-*) die "--out must not start with '-'" 2 ;;
esac
if [ -L "$out" ]; then
	die "--out must not be a symlink: $out" 2
fi
if [ -e "$out" ]; then
	[ -d "$out" ] || die "--out exists and is not a directory: $out" 2
	if [ -n "$(ls -A "$out")" ]; then
		die "--out directory is not empty: $out" 2
	fi
fi

case "$target" in
"") ;;
aarch64-apple-darwin | x86_64-apple-darwin | x86_64-unknown-linux-gnu | aarch64-unknown-linux-gnu) ;;
*) die "unsupported --target '$target'" 2 ;;
esac

script_dir=$(cd "$(dirname "$0")" && pwd -P)
root=$(cd "$script_dir/../.." && pwd -P)
[ -f "$root/Cargo.toml" ] || die "cannot locate workspace root from $script_dir"

command -v cargo >/dev/null 2>&1 || die "cargo is not on PATH"
command -v rustc >/dev/null 2>&1 || die "rustc is not on PATH"

host=$(cd "$root" && rustc -vV | sed -n 's/^host: //p')
[ -n "$target" ] || target=$host
case "$target" in
aarch64-apple-darwin) os=darwin arch=arm64 ;;
x86_64-apple-darwin) os=darwin arch=amd64 ;;
x86_64-unknown-linux-gnu) os=linux arch=amd64 ;;
aarch64-unknown-linux-gnu) os=linux arch=arm64 ;;
*) die "host triple '$target' is not a supported release target" 2 ;;
esac

if command -v sha256sum >/dev/null 2>&1; then
	sha256() { sha256sum "$1" | awk '{print $1}'; }
elif command -v shasum >/dev/null 2>&1; then
	sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
else
	die "need sha256sum or shasum"
fi

mkdir -p "$out"
out=$(cd "$out" && pwd -P)

stage=$(mktemp -d "${TMPDIR:-/tmp}/luuna-roots-package.XXXXXX")
trap 'rm -rf "$stage"' EXIT INT TERM HUP

printf 'package.sh: cargo build --release --locked -p roots-cli --target %s\n' "$target" >&2
(cd "$root" && cargo build --release --locked -p roots-cli --target "$target")

target_dir=${CARGO_TARGET_DIR:-target}
case "$target_dir" in
/*) ;;
*) target_dir="$root/$target_dir" ;;
esac
built="$target_dir/$target/release"
mkdir "$stage/bin"
for exe in luuna-roots lrt; do
	[ -f "$built/$exe" ] || die "build did not produce $built/$exe"
	cp "$built/$exe" "$stage/bin/$exe"
	chmod 0755 "$stage/bin/$exe"
	# Fixed mtime keeps archives reproducible across runs of the same build.
	# touch -t reads local time, so pin TZ to get the same epoch on every host.
	TZ=UTC touch -t 198001010000 "$stage/bin/$exe"
done

archive="luuna-roots_${version}_${os}_${arch}.tar.gz"
tarball="$stage/archive.tar"
# Only the two file entries are archived: no directory, link, or metadata entries.
if tar --version 2>/dev/null | grep -q 'GNU tar'; then
	tar -C "$stage" --format=ustar --owner=0 --group=0 --numeric-owner \
		-cf "$tarball" bin/luuna-roots bin/lrt
else
	COPYFILE_DISABLE=1 tar -C "$stage" --format=ustar --uid 0 --gid 0 --uname '' --gname '' \
		-cf "$tarball" bin/luuna-roots bin/lrt
fi
gzip -n -9 -c "$tarball" >"$out/$archive"

archive_sha=$(sha256 "$out/$archive")
printf '%s  %s\n' "$archive_sha" "$archive" >"$out/SHA256SUMS"

cp "$root/LICENSE" "$out/LICENSE"
cp "$root/UPSTREAM.md" "$out/UPSTREAM.md"

json_str() {
	# Escape backslash, double quote, and control characters for a JSON string value.
	printf '%s' "$1" | tr -d '\000-\010\013-\037' | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' | awk 'BEGIN{ORS=""} NR>1{print "\\n"} {print}'
}
json_or_null() {
	if [ -n "$1" ]; then
		printf '"%s"' "$(json_str "$1")"
	else
		printf 'null'
	fi
}

internal_version=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"$/\1/p' "$root/Cargo.toml" | head -n 1)
# shellcheck source=scripts/release/git-state.sh
. "$script_dir/git-state.sh"
git_state "$root"
ref=${GITHUB_REF:-$(git -C "$root" symbolic-ref -q HEAD 2>/dev/null || true)}
rustc_version=$(rustc --version)
cargo_version=$(cargo --version)

cat >"$out/provenance.json" <<EOF
{
  "schema": "luuna-component-provenance/1",
  "self_reported": true,
  "component": "roots",
  "repository": $(json_or_null "${GITHUB_REPOSITORY:-brodynelly/luuna-roots}"),
  "archive_version": "$(json_str "$version")",
  "internal_version": $(json_or_null "$internal_version"),
  "commit": $(json_or_null "$git_commit"),
  "worktree_dirty": $git_dirty,
  "untracked_present": $git_untracked,
  "ref": $(json_or_null "$ref"),
  "workflow_run_id": $(json_or_null "${GITHUB_RUN_ID:-}"),
  "workflow_run_attempt": $(json_or_null "${GITHUB_RUN_ATTEMPT:-}"),
  "runner_os": $(json_or_null "${RUNNER_OS:-$(uname -s)}"),
  "runner_arch": $(json_or_null "${RUNNER_ARCH:-$(uname -m)}"),
  "os": "$os",
  "arch": "$arch",
  "target": "$(json_str "$target")",
  "toolchain": {
    "rustc": "$(json_str "$rustc_version")",
    "cargo": "$(json_str "$cargo_version")"
  },
  "build_command": "cargo build --release --locked -p roots-cli --target $(json_str "$target")",
  "executables": ["luuna-roots", "lrt"],
  "archives": [
    { "file": "$(json_str "$archive")", "sha256": "$archive_sha" }
  ]
}
EOF

printf '%s\n' "$out/$archive"
