#!/bin/sh
# Offline tests for package.sh argument validation and check-archive.sh safety
# checks. Uses generated stub archives only: no cargo, network, or extraction
# into the repository.
#
#   sh scripts/release/test-release-scripts.sh
set -eu

here=$(cd "$(dirname "$0")" && pwd -P)
package="$here/package.sh"
check="$here/check-archive.sh"

sandbox=$(mktemp -d "${TMPDIR:-/tmp}/luuna-roots-release-test.XXXXXX")
trap 'rm -rf "$sandbox"' EXIT INT TERM HUP

failures=0
pass() { printf 'ok   %s\n' "$1"; }
flunk() {
	printf 'not ok %s\n' "$1"
	failures=$((failures + 1))
}

# expect_exit CODE NAME CMD... : run CMD, require exit CODE.
expect_exit() {
	want=$1 label=$2
	shift 2
	set +e
	"$@" >"$sandbox/out.log" 2>&1
	got=$?
	set -e
	if [ "$got" -eq "$want" ]; then
		pass "$label"
	else
		flunk "$label (exit $got, want $want)"
		sed 's/^/    /' "$sandbox/out.log"
	fi
}

# --- package.sh argument validation (all fail before cargo runs) ---------
expect_exit 2 "package: missing --version" sh "$package" --out "$sandbox/o1"
expect_exit 2 "package: missing --out" sh "$package" --version 1.0.0
expect_exit 2 "package: leading v rejected" sh "$package" --version v1.0.0 --out "$sandbox/o1"
expect_exit 2 "package: path in version rejected" sh "$package" --version '1.0.0/../x' --out "$sandbox/o1"
expect_exit 2 "package: build metadata rejected" sh "$package" --version 1.0.0+abc --out "$sandbox/o1"
expect_exit 2 "package: empty prerelease rejected" sh "$package" --version 1.0.0- --out "$sandbox/o1"
expect_exit 2 "package: unknown target rejected" sh "$package" --version 1.0.0 --out "$sandbox/o1" --target x86_64-pc-windows-msvc
expect_exit 2 "package: unknown flag rejected" sh "$package" --version 1.0.0 --out "$sandbox/o1" --publish
expect_exit 2 "package: dangling --out" sh "$package" --version 1.0.0 --out
mkdir "$sandbox/nonempty" && : >"$sandbox/nonempty/keep"
expect_exit 2 "package: non-empty --out rejected" sh "$package" --version 1.0.0 --out "$sandbox/nonempty"
: >"$sandbox/afile"
expect_exit 2 "package: file --out rejected" sh "$package" --version 1.0.0 --out "$sandbox/afile"
ln -s "$sandbox/nonempty" "$sandbox/linkdir"
expect_exit 2 "package: symlink --out rejected" sh "$package" --version 1.0.0 --out "$sandbox/linkdir"
[ -f "$sandbox/nonempty/keep" ] && [ "$(ls -A "$sandbox/nonempty")" = keep ] && pass "package: rejected --out left untouched" || flunk "package: rejected --out left untouched"
expect_exit 0 "package: --help" sh "$package" --help

# --- check-archive.sh fixtures ---------------------------------------------
if command -v sha256sum >/dev/null 2>&1; then
	sha256() { sha256sum "$1" | awk '{print $1}'; }
else
	sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
fi

# expect_fail_with PATTERN NAME CMD... : run CMD, require exit 1 and PATTERN in output,
# so a fixture is rejected for the intended reason and not an earlier check.
expect_fail_with() {
	pattern=$1 label=$2
	shift 2
	set +e
	"$@" >"$sandbox/out.log" 2>&1
	got=$?
	set -e
	if [ "$got" -eq 1 ] && grep -q -- "$pattern" "$sandbox/out.log"; then
		pass "$label"
	else
		flunk "$label (exit $got, want 1 with '$pattern')"
		sed 's/^/    /' "$sandbox/out.log"
	fi
}

stub_tree() {
	rm -rf "$sandbox/tree" && mkdir -p "$sandbox/tree/bin"
	printf '#!/bin/sh\necho stub "$@"\n' >"$sandbox/tree/bin/luuna-roots"
	cp "$sandbox/tree/bin/luuna-roots" "$sandbox/tree/bin/lrt"
	chmod 0755 "$sandbox/tree/bin/luuna-roots" "$sandbox/tree/bin/lrt"
}

# write_sums ARCHIVE : write a matching sibling SHA256SUMS.
write_sums() {
	printf '%s  %s\n' "$(sha256 "$1")" "$(basename "$1")" >"$(dirname "$1")/SHA256SUMS"
}

# make_archive NAME ENTRY... : tar ENTRY paths (relative to tree) into NAME,
# with a matching SHA256SUMS so later checks are what reject bad fixtures.
make_archive() {
	dest="$sandbox/$1"
	shift
	rm -rf "$dest" && mkdir -p "$dest"
	COPYFILE_DISABLE=1 tar -C "$sandbox/tree" -P -czf "$dest/luuna-roots_1.2.3_linux_amd64.tar.gz" "$@"
	write_sums "$dest/luuna-roots_1.2.3_linux_amd64.tar.gz"
	printf '%s\n' "$dest/luuna-roots_1.2.3_linux_amd64.tar.gz"
}

stub_tree
good=$(make_archive good bin/luuna-roots bin/lrt)
gooddir=$(dirname "$good")
goodname=$(basename "$good")
goodsum=$(sha256 "$good")
expect_exit 0 "check: well-formed archive with matching SHA256SUMS passes" sh "$check" --no-exec "$good"

printf '%s  %s\n' "0000000000000000000000000000000000000000000000000000000000000000" "$goodname" >"$gooddir/SHA256SUMS"
expect_fail_with "checksum mismatch" "check: SHA256SUMS mismatch rejected" sh "$check" --no-exec "$good"

rm "$gooddir/SHA256SUMS"
expect_fail_with "missing required SHA256SUMS" "check: missing SHA256SUMS rejected" sh "$check" --no-exec "$good"
expect_fail_with "missing required SHA256SUMS" "check: missing SHA256SUMS rejected with exec enabled" sh "$check" "$good"

printf '%s  %s\n' "$goodsum" "$goodname" >"$sandbox/realsums"
ln -s "$sandbox/realsums" "$gooddir/SHA256SUMS"
expect_fail_with "must not be a symlink" "check: symlinked SHA256SUMS rejected" sh "$check" --no-exec "$good"
rm "$gooddir/SHA256SUMS"

mkdir "$gooddir/SHA256SUMS"
expect_fail_with "not a regular file" "check: directory SHA256SUMS rejected" sh "$check" --no-exec "$good"
rmdir "$gooddir/SHA256SUMS"

printf '%s  %s\n' "$goodsum" "other.tar.gz" >"$gooddir/SHA256SUMS"
expect_fail_with "listed exactly once" "check: archive absent from SHA256SUMS rejected" sh "$check" --no-exec "$good"

printf '%s  %s\n%s  %s\n' "$goodsum" "$goodname" "$goodsum" "$goodname" >"$gooddir/SHA256SUMS"
expect_fail_with "listed exactly once" "check: duplicate SHA256SUMS entry rejected" sh "$check" --no-exec "$good"

printf '%s  %s\n%s *%s\n' "$goodsum" "$goodname" "$goodsum" "$goodname" >"$gooddir/SHA256SUMS"
expect_fail_with "listed exactly once" "check: duplicate binary-mode SHA256SUMS entry rejected" sh "$check" --no-exec "$good"

printf '%s  %s\n' "not-a-digest" "$goodname" >"$gooddir/SHA256SUMS"
expect_fail_with "malformed SHA256SUMS" "check: non-hex SHA256SUMS entry rejected" sh "$check" --no-exec "$good"

printf '%s  %s\n' "${goodsum}00" "$goodname" >"$gooddir/SHA256SUMS"
expect_fail_with "malformed SHA256SUMS" "check: wrong-length SHA256SUMS entry rejected" sh "$check" --no-exec "$good"

printf '%s  %s\n' "$(printf '%s' "$goodsum" | tr 'a-f' 'A-F')" "$goodname" >"$gooddir/SHA256SUMS"
case "$goodsum" in
*[a-f]*) expect_fail_with "malformed SHA256SUMS" "check: uppercase SHA256SUMS entry rejected" sh "$check" --no-exec "$good" ;;
esac
write_sums "$good"

missing=$(make_archive missing bin/luuna-roots)
expect_fail_with "archive entries must be exactly" "check: missing alias rejected" sh "$check" --no-exec "$missing"

: >"$sandbox/tree/bin/extra" && chmod 0755 "$sandbox/tree/bin/extra"
extra=$(make_archive extra bin/luuna-roots bin/lrt bin/extra)
expect_fail_with "archive entries must be exactly" "check: extra entry rejected" sh "$check" --no-exec "$extra"

stub_tree
withdir=$(make_archive withdir bin)
expect_exit 1 "check: directory entry rejected" sh "$check" --no-exec "$withdir"

stub_tree
chmod 0644 "$sandbox/tree/bin/lrt"
noexec=$(make_archive noexec bin/luuna-roots bin/lrt)
expect_fail_with "not a regular 0755 file" "check: non-executable entry rejected" sh "$check" --no-exec "$noexec"

stub_tree
chmod 0777 "$sandbox/tree/bin/lrt"
worldw=$(make_archive worldw bin/luuna-roots bin/lrt)
expect_fail_with "not a regular 0755 file" "check: world-writable entry rejected" sh "$check" --no-exec "$worldw"

stub_tree
rm "$sandbox/tree/bin/lrt" && ln -s luuna-roots "$sandbox/tree/bin/lrt"
symlink=$(make_archive symlink bin/luuna-roots bin/lrt)
expect_fail_with "not a regular 0755 file" "check: symlink entry rejected" sh "$check" --no-exec "$symlink"

stub_tree
rm "$sandbox/tree/bin/lrt" && ln "$sandbox/tree/bin/luuna-roots" "$sandbox/tree/bin/lrt"
hardlink=$(make_archive hardlink bin/luuna-roots bin/lrt)
expect_exit 1 "check: hardlink entry rejected" sh "$check" --no-exec "$hardlink"

stub_tree
mkdir -p "$sandbox/tree/sub"
traversal=$(make_archive traversal bin/luuna-roots bin/lrt sub/../bin/lrt)
expect_fail_with "archive entries must be exactly" "check: traversal entry rejected" sh "$check" --no-exec "$traversal"

absolute=$(make_archive absolute bin/luuna-roots "$sandbox/tree/bin/lrt")
expect_fail_with "archive entries must be exactly" "check: absolute entry rejected" sh "$check" --no-exec "$absolute"

stub_tree
badname="$sandbox/badname/luuna-roots_1.2.3_windows_amd64.tar.gz"
mkdir -p "$sandbox/badname"
tar -C "$sandbox/tree" -czf "$badname" bin/luuna-roots bin/lrt
write_sums "$badname"
expect_fail_with "unexpected archive name" "check: unsupported platform name rejected" sh "$check" --no-exec "$badname"

mkdir -p "$sandbox/corrupt"
printf 'not a tarball' >"$sandbox/corrupt/luuna-roots_9.9.9_linux_amd64.tar.gz"
write_sums "$sandbox/corrupt/luuna-roots_9.9.9_linux_amd64.tar.gz"
expect_fail_with "cannot list archive" "check: corrupt archive rejected" sh "$check" --no-exec "$sandbox/corrupt/luuna-roots_9.9.9_linux_amd64.tar.gz"

expect_exit 2 "check: missing archive argument" sh "$check"
expect_exit 2 "check: nonexistent archive" sh "$check" "$sandbox/nope.tar.gz"
expect_exit 2 "check: unknown option" sh "$check" --publish "$good"

# --- provenance git state (git-state.sh, no cargo) ---------------------------
# git_state_of PATH ROOT : print "commit|dirty|untracked" computed with PATH.
git_state_of() {
	(
		PATH=$1
		. "$here/git-state.sh"
		git_state "$2"
		printf '%s|%s|%s' "$git_commit" "$git_dirty" "$git_untracked"
	)
}
expect_state() {
	label=$1 want=$2 got=$3
	if [ "$got" = "$want" ]; then
		pass "$label"
	else
		flunk "$label (got '$got', want '$want')"
	fi
}

mkdir -p "$sandbox/nogit" "$sandbox/notrepo"
expect_state "provenance: git missing from PATH is unknown (null)" "|null|null" "$(git_state_of "$sandbox/nogit" "$sandbox/notrepo")"

# Stub git: rev-parse prints a fixed commit; status prints $STUB_STATUS or fails with $STUB_FAIL.
mkdir -p "$sandbox/stubgit"
cat >"$sandbox/stubgit/git" <<'STUB'
#!/bin/sh
for a in "$@"; do
	case "$a" in
	rev-parse) [ -n "${STUB_FAIL:-}" ] && exit 128; echo 0123456789abcdef0123456789abcdef01234567; exit 0 ;;
	status) [ -n "${STUB_FAIL:-}" ] && { echo "fatal: detected dubious ownership" >&2; exit 128; }; printf '%s' "${STUB_STATUS:-}"; exit 0 ;;
	esac
done
exit 2
STUB
chmod 0755 "$sandbox/stubgit/git"
stubpath="$sandbox/stubgit"
c=0123456789abcdef0123456789abcdef01234567
expect_state "provenance: clean checkout is false" "$c|false|false" "$(STUB_STATUS='' git_state_of "$stubpath" "$sandbox/notrepo")"
expect_state "provenance: untracked build input is dirty" "$c|true|true" "$(STUB_STATUS='?? build.rs' git_state_of "$stubpath" "$sandbox/notrepo")"
expect_state "provenance: untracked after tracked change is dirty" "$c|true|true" "$(STUB_STATUS=' M src/lib.rs
?? .cargo/config.toml' git_state_of "$stubpath" "$sandbox/notrepo")"
expect_state "provenance: tracked change only" "$c|true|false" "$(STUB_STATUS=' M Cargo.toml' git_state_of "$stubpath" "$sandbox/notrepo")"
expect_state "provenance: git failure is unknown (null)" "|null|null" "$(STUB_FAIL=1 git_state_of "$stubpath" "$sandbox/notrepo")"

if command -v git >/dev/null 2>&1; then
	expect_state "provenance: real git outside a repository is unknown (null)" "|null|null" \
		"$(GIT_DIR="$sandbox/notrepo" GIT_CEILING_DIRECTORIES="$sandbox" git_state_of "$PATH" "$sandbox/notrepo")"
else
	printf 'skip provenance: real git outside a repository (git not installed)\n'
fi

grep -q '"worktree_dirty": \$git_dirty,' "$package" && grep -q '"untracked_present": \$git_untracked,' "$package" &&
	grep -q -- '--untracked-files=normal' "$here/git-state.sh" &&
	pass "provenance: package.sh emits git_state values" || flunk "provenance: package.sh emits git_state values"

# --- reproducible mtime across time zones --------------------------------------
mtime() { stat -f %m "$1" 2>/dev/null || stat -c %Y "$1"; }
grep -q '^	TZ=UTC touch -t 198001010000 ' "$package" && pass "mtime: package.sh pins TZ=UTC for touch" || flunk "mtime: package.sh pins TZ=UTC for touch"
: >"$sandbox/tz-west" && : >"$sandbox/tz-east"
TZ=America/Los_Angeles sh -c 'TZ=UTC touch -t 198001010000 "$1"' sh "$sandbox/tz-west"
TZ=Asia/Tokyo sh -c 'TZ=UTC touch -t 198001010000 "$1"' sh "$sandbox/tz-east"
west=$(mtime "$sandbox/tz-west") east=$(mtime "$sandbox/tz-east")
[ "$west" = 315532800 ] && [ "$east" = 315532800 ] && pass "mtime: same epoch (315532800) under different host TZ" ||
	flunk "mtime: same epoch under different host TZ (west=$west east=$east)"

# Nothing escaped the private extraction dirs.
[ ! -e "$sandbox/bin" ] && [ ! -e "$here/bin" ] && pass "check: no files written outside temp dirs" || flunk "check: no files written outside temp dirs"

if [ "$failures" -ne 0 ]; then
	printf '%s release-script test(s) failed\n' "$failures"
	exit 1
fi
printf 'all release-script tests passed\n'
