#!/usr/bin/env bash
# The website's screenshots (site/images/), taken on Linux in Docker (xvfb), of one ice
# sheet view in each way of running nqv:
#   desktop.png          the desktop app, the view handed over by nqv -g from "albedo0"
#   terminal.png         the terminal viewer in an xterm, half-block characters
#   terminal-pixels.png  the same in an xterm with sixel graphics (full pixels)
#   png.png              nqv --png
# Usage: scripts/site-shots.sh DATA.nc   (an ice_NH-16KM.nc of CLIMBER-X; under $HOME,
# which colima shares with Docker)
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
data=$(cd "$(dirname "${1:?usage: $0 ice_NH-16KM.nc}")" && pwd)/$(basename "$1")
out=$root/linux-out/site
mkdir -p "$out"

docker volume create nqv-linux-cargo >/dev/null
docker volume create nqv-linux-target >/dev/null
docker volume create nqv-linux-rustup >/dev/null

docker run --rm -i \
  -v "$root":/repo:ro -v "$out":/out -v "$data":/data/ice.nc:ro \
  -v nqv-linux-cargo:/root/.cargo -v nqv-linux-rustup:/root/.rustup -v nqv-linux-target:/work/target \
  ubuntu:22.04 bash -s <<'IN'
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive CARGO_TARGET_DIR=/work/target PATH=/root/.cargo/bin:$PATH
apt-get update -qq
apt-get install -y -qq libwebkit2gtk-4.1-dev build-essential curl file libxdo-dev libssl-dev \
  librsvg2-dev xvfb dbus-x11 imagemagick rsync libgl1-mesa-dri xterm fonts-dejavu-core >/dev/null
command -v node >/dev/null || { curl -fsSL https://deb.nodesource.com/setup_22.x | bash - >/dev/null; apt-get install -y -qq nodejs >/dev/null; }
cargo --version >/dev/null 2>&1 || curl -sSf https://sh.rustup.rs | sh -s -- -y -q --profile minimal
rsync -a --delete --exclude target --exclude node_modules --exclude build --exclude linux-out \
  --exclude .svelte-kit /repo/ /work/src/
cd /work/src
npm ci --silent
npm run build >/dev/null
cargo build -q -p nqv-desktop --features tauri/custom-protocol
cargo build -q -p nqv
nqv=/work/target/debug/nqv
view=(-v uxy_s --mask "f_ice < 0.5" --contour "z_srf step=500" --range log,1,3000)
at=(--at xc=@300 --at yc=@330)

# nqv -g as from a cluster: its host name is albedo0
mkdir -p /tmp/bin /tmp/h/.config
printf '#!/bin/sh\necho albedo0\n' > /tmp/bin/hostname && chmod +x /tmp/bin/hostname
cat > /tmp/desk.sh <<RUN
/work/target/debug/nqv-desktop & app=\$!
sleep 6
PATH=/tmp/bin:\$PATH $nqv -g /data/ice.nc ${view[@]@Q} ${at[@]@Q} &
sleep 12; import -window root -crop 1280x860+0+0 /out/desktop.png; kill \$app
RUN
chmod +x /tmp/desk.sh
env HOME=/tmp/h WEBKIT_DISABLE_DMABUF_RENDERER=1 dbus-run-session -- \
  xvfb-run -a -s "-screen 0 1400x950x24" /tmp/desk.sh > /out/desktop.log 2>&1 || true

# xterm: half-blocks, then sixel (as a VT340, with enough colours)
term() { # NAME GFX XTERM-OPTIONS...
  local name=$1 gfx=$2; shift 2
  cat > /tmp/term.sh <<RUN
LC_ALL=C.UTF-8 LANG=C.UTF-8 xterm -u8 -geometry 150x46+0+0 -fa "DejaVu Sans Mono" -fs 11 \
  -bg black -fg white $* -e env TERM=xterm-256color COLORTERM=truecolor NQV_GFX=$gfx \
  $nqv /data/ice.nc ${view[@]@Q} ${at[@]@Q} & t=\$!
sleep 8; import -window root /tmp/full.png; kill \$t
convert /tmp/full.png -trim +repage /out/$name.png
RUN
  chmod +x /tmp/term.sh
  xvfb-run -a -s "-screen 0 1600x1000x24" /tmp/term.sh > /out/$name.log 2>&1 || true
}
term terminal blocks
term terminal-pixels sixel -ti vt340 -xrm "'XTerm*numColorRegisters: 1024'" \
  -xrm "'XTerm*maxGraphicSize: 2000x2000'"

$nqv /data/ice.nc "${view[@]}" --png /out/png.png --size 780x800 > /dev/null
ls -la /out/*.png
IN
echo "screenshots in $out; copy the good ones to site/images/"
