# bash completion for luuna-beacon
_luuna_beacon() {
  local cur="${COMP_WORDS[COMP_CWORD]}"
  COMPREPLY=( $(compgen -W '%s' -- "$cur") )
}
complete -F _luuna_beacon luuna-beacon lbc lb
