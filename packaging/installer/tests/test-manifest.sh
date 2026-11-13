#!/bin/sh
# Manifest format validation, platform selection, index hashes and append-only immutability.
# The paths of lib.sh and of the manifest library are only known at run time (SC1091, SC1090).
# SC2016: the shell-metacharacter sample is literal text on purpose. SC2094: the line read is copied to the end of the same index file on purpose.
# shellcheck disable=SC1091,SC1090,SC2016,SC2094
. "$TESTS_DIR/lib.sh"
. "$MANIFEST_LIB"

RELDIR=packaging/installer/releases

valid_manifest() { make_fixture_release "$CASE_DIR/rel" 1.0.0; M="$CASE_DIR/rel/manifest.tsv"; }

# set_field N VALUE: copy the valid manifest with field N of the first asset row (line 3) replaced.
set_field() {
  awk -F '\t' -v n="$1" -v v="$2" 'BEGIN{OFS="\t"} NR==3{$n=v} {print}' "$M" > "$CASE_DIR/bad.tsv"
}
# sub_row3 SED_EXPR: copy the valid manifest with a sed expression applied to line 3 only.
sub_row3() { sed "3$1" "$M" > "$CASE_DIR/bad.tsv"; }
field() { awk -F '\t' -v n="$1" 'NR==3{print $n}' "$M"; }

expect_invalid() { # MESSAGE_FRAGMENT
  if cmp -s "$M" "$CASE_DIR/bad.tsv"; then fail "mutation did not change the manifest"; fi
  run_cmd manifest_validate "$CASE_DIR/bad.tsv"
  assert_rc_nonzero
  assert_out_contains "$1"
}

t_valid() { valid_manifest; run_cmd manifest_validate "$M"; assert_rc 0; assert_eq "$(manifest_release "$M")" 1.0.0; }
t_bad_header() { valid_manifest; awk 'NR==1{print "luuna-release-manifest" "\t" "2"; next} {print}' "$M" > "$CASE_DIR/bad.tsv"; expect_invalid "header"; }
t_bad_release() { valid_manifest; awk 'NR==2{print "release" "\t" "v1"; next} {print}' "$M" > "$CASE_DIR/bad.tsv"; expect_invalid "release"; }
t_sha_63() { valid_manifest; set_field 7 "$(field 7 | cut -c1-63)"; expect_invalid "sha256"; }
t_sha_upper() { valid_manifest; set_field 7 "A$(field 7 | cut -c2-)"; expect_invalid "sha256"; }
t_http_url() { valid_manifest; sub_row3 's#https://#http://#'; expect_invalid "url must"; }
t_other_repo() { valid_manifest; sub_row3 's#luuna-ai/luuna/#luuna-ai/luuna-atlas/#'; expect_invalid "url must"; }
t_other_owner() { valid_manifest; sub_row3 's#github.com/luuna-ai/#github.com/someone-else/#'; expect_invalid "url must"; }
t_other_tag() { valid_manifest; sub_row3 's#/v1\.0\.0/#/release-1.0.0/#'; expect_invalid "url must"; }
t_url_name_mismatch() { valid_manifest; set_field 6 "$(field 6 | sed 's/\.tar\.gz$/.tgz/')"; expect_invalid "url must"; }
t_unknown_component() { valid_manifest; set_field 2 compass; expect_invalid "unknown component"; }
t_bad_os() { valid_manifest; set_field 4 windows; expect_invalid "unsupported os"; }
t_exe_luuna() { valid_manifest; set_field 9 luuna-tui,luuna-runtime,luuna; expect_invalid "executable"; }
t_exe_lu() { valid_manifest; set_field 9 luuna-tui,luuna-runtime,lu; expect_invalid "executable"; }
t_exe_unknown() { valid_manifest; set_field 9 luuna-tui,luuna-runtime,lat; expect_invalid "executable"; }
t_exe_missing() { valid_manifest; set_field 9 luuna-tui,luuna-runtime; expect_invalid "must include"; }
t_exe_duplicate() { valid_manifest; set_field 9 luuna-tui,luuna-runtime,luuna-sbx,luuna-sbx; expect_invalid "duplicate executable"; }
t_row_version_differs() {
  valid_manifest
  set_field 3 1.0.1
  expect_invalid "version"
}
t_duplicate_row() { valid_manifest; { cat "$M"; sed -n 3p "$M"; } > "$CASE_DIR/bad.tsv"; expect_invalid "duplicate asset row"; }
t_crlf() { valid_manifest; sed 's/$/\r/' "$M" > "$CASE_DIR/bad.tsv"; expect_invalid "CRLF"; }
t_extra_field() { valid_manifest; awk 'NR==3{print $0 "\t" "extra"; next} {print}' "$M" > "$CASE_DIR/bad.tsv"; expect_invalid "9 tab-separated"; }
t_blank_line() { valid_manifest; { cat "$M"; echo; } > "$CASE_DIR/bad.tsv"; expect_invalid "9 tab-separated"; }
t_shell_meta() { valid_manifest; sub_row3 's#/v1\.0\.0/#/v1.0.0$(id)/#'; expect_invalid "url"; }

t_select() {
  valid_manifest
  run_cmd manifest_select "$M" linux amd64; assert_rc 0
  assert_eq "$(wc -l < "$OUT_FILE" | tr -d ' ')" 1
  assert_out_contains "luuna_1.0.0_linux_amd64.tar.gz"
  run_cmd manifest_select "$M" darwin arm64; assert_rc 3
}

t_version_ids() {
  manifest_is_version 1.2.3 || fail "1.2.3 rejected"
  manifest_is_version 1.2.3-rc.1 || fail "rc rejected"
  for bad in "" v1.2.3 1.2 "1.2.3 " "../1.2.3" "1.2.3/../x" "1.2.3-" "1.2.3
1.2.4"; do
    if manifest_is_version "$bad"; then fail "accepted bad version [$bad]"; fi
  done
}

t_repo_header_only() {
  run_cmd sh "$CHECK_MANIFESTS"; assert_rc 0; assert_out_contains "ok (0 release"
  assert_eq "$(wc -l < "$REPO_ROOT/$RELDIR/INDEX.tsv" | tr -d ' ')" 1
}

# make_releases_repo: throwaway repository layout with packaging/installer/releases/ for --root.
make_releases_repo() {
  R="$CASE_DIR/repo"
  mkdir -p "$R/$RELDIR"
  printf '#release-version\tmanifest-sha256\n' > "$R/$RELDIR/INDEX.tsv"
}
add_release() { # VERSION
  make_fixture_release "$CASE_DIR/fx-$1" "$1"
  mkdir -p "$R/$RELDIR/$1"
  cp "$CASE_DIR/fx-$1/manifest.tsv" "$R/$RELDIR/$1/manifest.tsv"
  printf '%s\t%s\n' "$1" "$(sha256_of "$R/$RELDIR/$1/manifest.tsv")" >> "$R/$RELDIR/INDEX.tsv"
}

t_index_ok() { make_releases_repo; add_release 1.0.0; run_cmd sh "$CHECK_MANIFESTS" --root "$R"; assert_rc 0; assert_out_contains "ok (1 release"; }
t_index_mismatch() {
  make_releases_repo; add_release 1.0.0
  printf '#release-version\tmanifest-sha256\n1.0.0\t%064d\n' 0 > "$R/$RELDIR/INDEX.tsv"
  run_cmd sh "$CHECK_MANIFESTS" --root "$R"; assert_rc 1; assert_out_contains "does not match its INDEX.tsv hash"
}
t_index_duplicate() {
  make_releases_repo; add_release 1.0.0
  sed -n 2p "$R/$RELDIR/INDEX.tsv" >> "$R/$RELDIR/INDEX.tsv"
  run_cmd sh "$CHECK_MANIFESTS" --root "$R"; assert_rc 1; assert_out_contains "duplicate version"
}
t_unindexed_dir() {
  make_releases_repo; add_release 1.0.0
  printf '#release-version\tmanifest-sha256\n' > "$R/$RELDIR/INDEX.tsv"
  run_cmd sh "$CHECK_MANIFESTS" --root "$R"; assert_rc 1; assert_out_contains "not listed in INDEX.tsv"
}
t_release_dir_mismatch() {
  make_releases_repo; add_release 1.0.0
  mv "$R/$RELDIR/1.0.0" "$R/$RELDIR/1.0.1"
  printf '#release-version\tmanifest-sha256\n1.0.1\t%s\n' "$(sha256_of "$R/$RELDIR/1.0.1/manifest.tsv")" > "$R/$RELDIR/INDEX.tsv"
  run_cmd sh "$CHECK_MANIFESTS" --root "$R"; assert_rc 1; assert_out_contains "release line does not match"
}

# git_repo: a temporary repository inside the case directory (assumption I20), identity on example.invalid.
git_repo() {
  make_releases_repo
  git -C "$R" init -q
  git -C "$R" config user.email test@example.invalid
  git -C "$R" config user.name test
  git -C "$R" config commit.gpgsign false
  add_release 1.0.0
  git -C "$R" add -A && git -C "$R" commit -q -m base
}
t_append_ok() {
  git_repo; add_release 1.1.0
  run_cmd sh "$CHECK_MANIFESTS" --root "$R" --base HEAD; assert_rc 0
}
t_modified_manifest_rejected() {
  git_repo
  # Reorder the executables of the published manifest (still valid) and fix its index line, so
  # only the append-only rule can catch it.
  m="$R/$RELDIR/1.0.0/manifest.tsv"
  awk -F '\t' 'BEGIN{OFS="\t"} NR==3{$9="luuna-sbx,luuna-runtime,luuna-tui"} {print}' "$m" > "$R/m" && mv "$R/m" "$m"
  printf '#release-version\tmanifest-sha256\n1.0.0\t%s\n' "$(sha256_of "$m")" > "$R/$RELDIR/INDEX.tsv"
  run_cmd sh "$CHECK_MANIFESTS" --root "$R"; assert_rc 0
  run_cmd sh "$CHECK_MANIFESTS" --root "$R" --base HEAD; assert_rc 1
  assert_out_contains "published release file changed (M): $RELDIR/1.0.0/manifest.tsv"
  assert_out_contains "INDEX.tsv may only gain appended lines"
}
t_deleted_release_rejected() {
  git_repo
  rm -rf "${R:?}/$RELDIR/1.0.0"
  printf '#release-version\tmanifest-sha256\n' > "$R/$RELDIR/INDEX.tsv"
  run_cmd sh "$CHECK_MANIFESTS" --root "$R" --base HEAD; assert_rc 1; assert_out_contains "(D)"
}
t_unknown_base() { git_repo; run_cmd sh "$CHECK_MANIFESTS" --root "$R" --base no-such-ref; assert_rc 2; }

run_case "valid fixture manifest" t_valid
run_case "bad header rejected" t_bad_header
run_case "bad release line rejected" t_bad_release
run_case "63-character sha rejected" t_sha_63
run_case "uppercase sha rejected" t_sha_upper
run_case "non-https url rejected" t_http_url
run_case "url for another repository of the organisation rejected" t_other_repo
run_case "url for another owner rejected" t_other_owner
run_case "tag other than v<version> rejected" t_other_tag
run_case "url not ending in the archive name rejected" t_url_name_mismatch
run_case "unknown component rejected" t_unknown_component
run_case "unsupported os rejected" t_bad_os
run_case "executable luuna rejected" t_exe_luuna
run_case "executable lu rejected" t_exe_lu
run_case "unknown executable lat rejected" t_exe_unknown
run_case "missing required executable rejected" t_exe_missing
run_case "duplicated executable rejected" t_exe_duplicate
run_case "row version different from the release line rejected" t_row_version_differs
run_case "duplicate row rejected" t_duplicate_row
run_case "CRLF line endings rejected" t_crlf
run_case "extra field rejected" t_extra_field
run_case "blank line rejected" t_blank_line
run_case "shell metacharacters in url rejected" t_shell_meta
run_case "platform selection: one row for linux/amd64, exit 3 for darwin/arm64" t_select
run_case "version identifiers are validated" t_version_ids
run_case "repository packaging/installer/releases/ is header-only and valid" t_repo_header_only
run_case "indexed manifest passes" t_index_ok
run_case "index hash mismatch rejected" t_index_mismatch
run_case "duplicate index version rejected" t_index_duplicate
run_case "unindexed release directory rejected" t_unindexed_dir
run_case "release line must match directory" t_release_dir_mismatch
if command -v git >/dev/null 2>&1; then
  run_case "appending a release passes --base" t_append_ok
  run_case "modified existing manifest rejected against --base" t_modified_manifest_rejected
  run_case "deleted release rejected against --base" t_deleted_release_rejected
  run_case "unknown base ref is a usage error" t_unknown_base
else
  skip_case "immutability against --base" "git not installed"
fi
finish_tests
