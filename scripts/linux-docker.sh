#!/usr/bin/env bash
# Build and try the desktop app on Linux from a Mac (or any Docker host), in Ubuntu 22.04
# as the release does: the smoke test with a screenshot, then the .deb and the AppImage.
# Output in linux-out/ (screenshot.png, bundles). Usage: scripts/linux-docker.sh [--no-bundle]
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
out=$root/linux-out
mkdir -p "$out"
bundle=1
[[ ${1:-} == --no-bundle ]] && bundle=0

# Caches between runs: cargo, the target folder, npm.
docker volume create nqv-linux-cargo >/dev/null
docker volume create nqv-linux-target >/dev/null

docker run --rm -i \
  -v "$root":/repo:ro -v "$out":/out \
  -v nqv-linux-cargo:/root/.cargo -v nqv-linux-target:/work/target \
  -e BUNDLE=$bundle -e APPIMAGE_EXTRACT_AND_RUN=1 -e NO_STRIP=true \
  ubuntu:22.04 bash -s <<'IN'
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive CARGO_TARGET_DIR=/work/target PATH=/root/.cargo/bin:$PATH
apt-get update -qq
apt-get install -y -qq libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev \
  libssl-dev libayatana-appindicator3-dev librsvg2-dev patchelf xvfb dbus-x11 \
  imagemagick rsync openssh-client libgl1-mesa-dri >/dev/null
command -v node >/dev/null || { curl -fsSL https://deb.nodesource.com/setup_22.x | bash - >/dev/null; apt-get install -y -qq nodejs >/dev/null; }
command -v cargo >/dev/null || curl -sSf https://sh.rustup.rs | sh -s -- -y -q --profile minimal
rsync -a --delete --exclude target --exclude node_modules --exclude build --exclude linux-out \
  --exclude .svelte-kit /repo/ /work/src/
cd /work/src
npm ci --silent
NQV_SMOKE_SHOT=/out/screenshot.png scripts/ci/smoke-linux.sh
if [ "$BUNDLE" = 1 ]; then
  npm run tauri build -- --bundles deb,appimage
  cp -v $CARGO_TARGET_DIR/release/bundle/deb/*.deb $CARGO_TARGET_DIR/release/bundle/appimage/*.AppImage /out/
  dpkg-deb -c /out/*.deb | grep -E 'bin/|\.desktop'
fi
IN
echo "results in $out"
