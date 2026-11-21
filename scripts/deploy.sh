#!/usr/bin/env bash
# Build a static Linux nqv binary and copy it to one or more ssh hosts.
# Usage: scripts/deploy.sh HOST [HOST...]   (installs to ~/bin/nqv; override with NQV_DEST)
set -euo pipefail
[ $# -ge 1 ] || { echo "usage: $0 HOST [HOST...]" >&2; exit 1; }
target=x86_64-unknown-linux-musl
dest=${NQV_DEST:-bin}
cd "$(dirname "$0")/.."
rustup target list --installed | grep -qx $target || rustup target add $target
cargo build --release -p nqv --target $target
for host in "$@"; do
  ssh "$host" "mkdir -p $dest"
  scp -q target/$target/release/nqv "$host:$dest/nqv"
  echo "$host: $(ssh "$host" "$dest/nqv --version")"
done
