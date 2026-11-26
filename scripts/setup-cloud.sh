#!/usr/bin/env bash
# Reproduce the cloud foundation without installing repository Git hooks.
set -euo pipefail
repository_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
luuna_tools_dir="${LUUNA_TOOLS_DIR:-/workspace/.tools}"
luuna_cache_dir="${LUUNA_CACHE_DIR:-/workspace/.cache}"
export npm_config_cache="$luuna_cache_dir/npm"
export CARGO_HOME="$luuna_tools_dir/cargo"
export RUSTUP_HOME="$luuna_tools_dir/rustup"
export PATH="$CARGO_HOME/bin:$luuna_tools_dir/luuna-pi/node_modules/.bin:$PATH"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
mkdir -p "$luuna_tools_dir" "$luuna_cache_dir"
node -e 'if (Number(process.versions.node.split(".")[0]) < 24) throw new Error("Use Node 24 or newer for the pinned Pi/runtime toolchain")'
if ! command -v rustup >/dev/null; then
  rustup_installer="$(mktemp)"
  trap 'rm -f "$rustup_installer"' EXIT
  curl --fail --silent --show-error https://sh.rustup.rs -o "$rustup_installer"
  sh "$rustup_installer" -y --no-modify-path --profile minimal --default-toolchain 1.90.0
fi
rustup toolchain install 1.90.0 --profile minimal --component rustfmt --component clippy --no-self-update
# Install the reviewed Pi dependency graph without changing its package identities.
mkdir -p "$luuna_tools_dir/luuna-pi"
cp "$repository_dir/tools/pi/package.json" "$luuna_tools_dir/luuna-pi/package.json"
cp "$repository_dir/tools/pi/package-lock.json" "$luuna_tools_dir/luuna-pi/package-lock.json"
npm ci --prefix "$luuna_tools_dir/luuna-pi" --ignore-scripts --no-audit --no-fund
cd "$repository_dir"
npm ci --ignore-scripts --no-audit --no-fund
cargo fetch --locked
cargo build --locked --workspace
printf 'Luuna cloud dependencies and runtime binary prepared. Run the checks documented in README.md.\n'
