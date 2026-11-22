#!/bin/sh
# Install the nqv terminal viewer (NetCDF Quick View): one static file, nothing else.
#
#   curl -fsSL https://mathesislab.github.io/nqv/install.sh | sh
#
# Puts the binary for this machine (Linux x86-64 or ARM, macOS on Apple silicon) in
# ~/bin, or in $NQV_DEST. NQV_VERSION=v0.3.0 installs that release instead of the
# latest. Run it again to update.
set -eu

repo=https://github.com/mathesislab/nqv
dest=${NQV_DEST:-$HOME/bin}
version=${NQV_VERSION:-latest}

say() { printf 'nqv install: %s\n' "$*"; }
fail() { say "$*" >&2; exit 1; }

case "$(uname -s)/$(uname -m)" in
  Linux/x86_64 | Linux/amd64) asset=nqv-linux-x86_64 ;;
  Linux/aarch64 | Linux/arm64) asset=nqv-linux-aarch64 ;;
  Darwin/arm64) asset=nqv-macos-aarch64 ;;
  *) fail "no build for $(uname -s) $(uname -m); see $repo#build to build it" ;;
esac

case "$version" in
  latest) url=$repo/releases/latest/download/$asset ;;
  v*) url=$repo/releases/download/$version/$asset ;;
  *) url=$repo/releases/download/v$version/$asset ;;
esac

mkdir -p "$dest"
tmp=$(mktemp "$dest/.nqv.XXXXXX")
trap 'rm -f "$tmp"' EXIT
say "downloading $asset ($version)"
if command -v curl >/dev/null 2>&1; then
  curl -fsSL -o "$tmp" "$url" || fail "could not download $url"
elif command -v wget >/dev/null 2>&1; then
  wget -q -O "$tmp" "$url" || fail "could not download $url"
else
  fail "needs curl or wget"
fi
chmod 755 "$tmp"
# the new binary must run here before it replaces the old one
"$tmp" --version >/dev/null 2>&1 || fail "the downloaded nqv does not run on this machine"
mv -f "$tmp" "$dest/nqv"
trap - EXIT
say "installed $("$dest/nqv" --version) in $dest/nqv"

case ":$PATH:" in
  *":$dest:"*) ;;
  *) say "$dest is not on your PATH: add  export PATH=\"$dest:\$PATH\"  to ~/.bashrc (or ~/.zshrc)" ;;
esac
