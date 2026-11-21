#!/usr/bin/env bash
# Set the version everywhere it is kept: the workspace (all crates and the desktop app,
# whose tauri.conf.json takes it from its Cargo.toml), package.json and the lock files.
# Usage: scripts/version.sh X.Y.Z   (then commit "Version X.Y.Z", tag vX.Y.Z, push the tag)
set -euo pipefail
v=${1:?usage: $0 X.Y.Z}
[[ $v =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "not a version: $v" >&2; exit 1; }
cd "$(dirname "$0")/.."
perl -0pi -e "s/(\[workspace\.package\]\nversion = )\"[^\"]*\"/\${1}\"$v\"/" Cargo.toml
npm version "$v" --no-git-tag-version --allow-same-version >/dev/null
cargo update -w -q
grep -A1 '^\[workspace.package\]' Cargo.toml | tail -1
