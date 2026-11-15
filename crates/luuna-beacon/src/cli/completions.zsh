#compdef luuna-beacon lbc lb
_luuna_beacon() {
  local -a commands
  commands=(%s)
  _describe 'command' commands
}
_luuna_beacon "$@"
