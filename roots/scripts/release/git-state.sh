# shellcheck shell=sh
# Sourced by package.sh and test-release-scripts.sh; not run directly.
#
# git_state ROOT sets three variables describing the checkout at ROOT:
#   git_commit     HEAD commit id, or empty when unknown
#   git_dirty      true | false | null  tracked changes or untracked files exist
#   git_untracked  true | false | null  untracked, non-ignored files exist
# null means unknown (no git on PATH, not a repository, unsafe-directory
# refusal, or any other git failure) and must never be read as clean. Ignored
# files such as target/ do not count. Apart from git itself this uses only shell
# builtins, so it still runs when git is absent from PATH.
git_state() {
	git_commit=""
	git_dirty=null
	git_untracked=null
	command -v git >/dev/null 2>&1 || return 0
	git_commit=$(git --no-optional-locks -C "$1" rev-parse --verify -q HEAD 2>/dev/null) || git_commit=""
	if git_status=$(git --no-optional-locks -C "$1" status --porcelain --untracked-files=normal 2>/dev/null); then
		if [ -n "$git_status" ]; then
			git_dirty=true
		else
			git_dirty=false
		fi
		case "$git_status" in
		'??'* | *'
??'*) git_untracked=true ;;
		*) git_untracked=false ;;
		esac
	fi
	return 0
}
