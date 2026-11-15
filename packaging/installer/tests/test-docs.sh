#!/bin/sh
# Documentation and release-directory guards. Documents must not advertise installs that do not exist,
# and the installer's source must keep the allow-lists the tests rely on.
# The path of lib.sh is only known at run time (TESTS_DIR), so shellcheck cannot follow it.
# The eval guard test writes a literal sample line with single quotes on purpose (SC2016).
# shellcheck disable=SC1091,SC2016
. "$TESTS_DIR/lib.sh"

R=$REPO_ROOT
INST=$R/packaging/installer
DOCS="$R/README.md $R/CONTRIBUTING.md $R/docs/INSTALL.md $INST/releases/FORMAT.md $INST/tests/README.md $R/docs/decisions/0048-one-posix-installer.md"

need_file() { [ -f "$1" ] || fail "missing file: $1"; }

pipe_to_shell() { grep -nE '(curl|wget)[^|]*\|[[:space:]]*(sudo[[:space:]]+)?(ba|z|da)?sh([^A-Za-z0-9_-]|$)' "$1"; }
t_no_curl_pipe() {
  for f in $DOCS; do need_file "$f"; if pipe_to_shell "$f"; then fail "pipe-to-shell install line in $f"; fi; done
  printf 'curl -fsSL https://example.invalid/install.sh | sh\n' > "$CASE_DIR/bad.md"
  pipe_to_shell "$CASE_DIR/bad.md" >/dev/null || fail "pipe guard missed curl | sh"
}

t_no_checksums_in_releases() {
  if grep -rnE '[0-9a-fA-F]{64}' "$INST/releases"; then fail "64-hex string under packaging/installer/releases/ (real checksums arrive only with a release)"; fi
  assert_eq "$(wc -l < "$INST/releases/INDEX.tsv" | tr -d ' ')" 1
  for d in "$INST/releases"/*/; do [ -d "$d" ] && fail "release directory present: $d"; done
  true
}

t_install_md_sentences() {
  need_file "$R/docs/INSTALL.md"
  for s in 'No release of Luuna has been published yet.' \
           'Supported platforms: macOS and Linux (x86_64, arm64).' \
           'Windows: not supported by this installer.'; do
    grep -F -e "$s" "$R/docs/INSTALL.md" >/dev/null || fail "docs/INSTALL.md lacks: $s"
  done
}

t_no_windows_assets() {
  need_file "$INST/install-manifest.sh"
  if grep -rniE '_windows_|windows_(amd64|arm64)' "$INST/releases" "$INST/install-manifest.sh"; then fail "Windows asset naming present"; fi
  true
}

t_public_names() {
  need_file "$INST/install.sh"
  assert_eq "$(grep -c '^PUBLIC_NAMES=' "$INST/install.sh" | tr -d ' ')" 1 "(number of PUBLIC_NAMES lines)"
  assert_eq "$(grep '^PUBLIC_NAMES=' "$INST/install.sh")" 'PUBLIC_NAMES="luuna lu luuna-runtime"'
}

t_github_urls() {
  need_file "$INST/install-manifest.sh"
  want='https://github.com/luuna-ai/luuna/releases/download/'
  hits=$(grep -o 'https://github\.com/.*' "$INST/install-manifest.sh" || true)
  [ -n "$hits" ] || fail "install-manifest.sh has no github.com allow-list"
  printf '%s\n' "$hits" | while IFS= read -r hit; do
    case "$hit" in "$want"*) ;; *) fail "github.com url outside the one repository: $hit" ;; esac
  done
}

t_no_suite_name() {
  need_file "$INST/install.sh"; need_file "$INST/install-manifest.sh"
  if grep -n 'luuna-suite' "$INST/install.sh" "$INST/install-manifest.sh"; then fail "luuna-suite named in the installer"; fi
  true
}

t_no_eval() {
  need_file "$INST/install.sh"; need_file "$INST/install-manifest.sh"
  if grep -nw eval "$INST/install.sh" "$INST/install-manifest.sh"; then fail "eval used in the installer"; fi
  printf 'x=1; eval "$x"\n' > "$CASE_DIR/bad.sh"
  grep -nw eval "$CASE_DIR/bad.sh" >/dev/null || fail "eval guard missed a use"
}

run_case "no curl/wget pipe-to-shell lines" t_no_curl_pipe
run_case "no checksums or release directories under packaging/installer/releases/" t_no_checksums_in_releases
run_case "docs/INSTALL.md carries the three exact sentences" t_install_md_sentences
run_case "no Windows asset naming" t_no_windows_assets
run_case "PUBLIC_NAMES is exactly luuna, lu and luuna-runtime" t_public_names
run_case "every github.com url is the one repository's release download" t_github_urls
run_case "luuna-suite does not occur in the installer" t_no_suite_name
run_case "eval does not occur in the installer" t_no_eval
finish_tests
