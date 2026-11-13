#!/usr/bin/env bash
set -euo pipefail

# The machine-local launcher owns the shared service, private connection and app bundle.
mode="${1:-run}"
case "$mode" in
  run|--verify)
    python3 "$HOME/.config/luuna/launch.py" gui-rebuild
    ;;
  --logs)
    python3 "$HOME/.config/luuna/launch.py" gui-rebuild
    exec /usr/bin/log stream --info --style compact --predicate 'process == "luuna-desktop"'
    ;;
  --debug)
    exec python3 "$HOME/.config/luuna/launch.py" gui-dev
    ;;
  *)
    echo "usage: $0 [run|--verify|--logs|--debug]" >&2
    exit 2
    ;;
esac
