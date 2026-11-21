#!/usr/bin/env bash
# The website's screenshots (site/images/), taken on Linux in Docker (xvfb):
#   desktop.png          the desktop app, a view handed over by nqv -g from "albedo0"
#   terminal.png         the terminal viewer in an xterm, half-block characters
#   terminal-pixels.png  the same in an xterm with sixel graphics (full pixels)
#   png.png              nqv --png
# and, given Yelmo output with its CF grid mapping, the same in projections:
#   desktop-proj.png     the desktop app, as a globe
#   terminal-proj.png    the terminal viewer (sixel), in latitude-longitude
#   terminal-proj-blocks.png  the same in half-blocks
# Usage: scripts/site-shots.sh ICE.nc [YELMO.nc]   (CLIMBER-X ice_NH-16KM.nc, Yelmo
# yelmo.nc; under $HOME, which colima shares with Docker)
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
abs() { echo "$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"; }
ice=$(abs "${1:?usage: $0 ICE.nc [YELMO.nc]}")
yelmo=${2:+$(abs "$2")}
out=$root/linux-out/site
mkdir -p "$out"

docker volume create nqv-linux-cargo >/dev/null
docker volume create nqv-linux-target >/dev/null
docker volume create nqv-linux-rustup >/dev/null

docker run --rm -i \
  -v "$root":/repo:ro -v "$out":/out -v "$ice":/data/ice.nc:ro \
  ${yelmo:+-v "$yelmo":/data/yelmo.nc:ro} -e YELMO=${yelmo:+1} \
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

# nqv -g as from a cluster: its host name is albedo0
mkdir -p /tmp/bin /tmp/h/.config
printf '#!/bin/sh\necho albedo0\n' > /tmp/bin/hostname && chmod +x /tmp/bin/hostname

# desktop NAME ARGS...: the app, the view handed over by nqv -g
desktop() {
  local name=$1; shift
  cat > /tmp/desk.sh <<RUN
/work/target/debug/nqv-desktop & app=\$!
sleep 6
PATH=/tmp/bin:\$PATH $nqv -g ${*@Q} &
sleep 12; import -window root -crop 1280x860+0+0 /out/$name.png; kill \$app
RUN
  chmod +x /tmp/desk.sh
  env HOME=/tmp/h WEBKIT_DISABLE_DMABUF_RENDERER=1 dbus-run-session -- \
    xvfb-run -a -s "-screen 0 1400x950x24" /tmp/desk.sh > /out/$name.log 2>&1 || true
}

# term NAME GFX XTERM-OPTIONS -- NQV-ARGS...: the terminal viewer in an xterm
term() {
  local name=$1 gfx=$2; shift 2
  local xopts=()
  while [ "$1" != "--" ]; do xopts+=("$1"); shift; done
  shift
  cat > /tmp/term.sh <<RUN
LC_ALL=C.UTF-8 LANG=C.UTF-8 xterm -u8 -geometry 150x46+0+0 -fa "DejaVu Sans Mono" -fs 11 \
  -bg black -fg white ${xopts[@]@Q} -e env TERM=xterm-256color COLORTERM=truecolor NQV_GFX=$gfx \
  $nqv ${*@Q} & t=\$!
sleep 8; import -window root /tmp/full.png; kill \$t
convert /tmp/full.png -trim +repage /out/$name.png
RUN
  chmod +x /tmp/term.sh
  xvfb-run -a -s "-screen 0 1600x1000x24" /tmp/term.sh > /out/$name.log 2>&1 || true
}
sixel=(-ti vt340 -xrm "XTerm*numColorRegisters: 1024" -xrm "XTerm*maxGraphicSize: 2000x2000")

# CLIMBER-X's NH grid has no grid mapping in the file: --grid gives it (for coastlines)
ice=(/data/ice.nc --grid "stere north" -v uxy_s --mask "f_ice < 0.5" --contour "z_srf step=500" --range log,1,3000)
at=(--at xc=@300 --at yc=@330)
desktop desktop "${ice[@]}" "${at[@]}"
term terminal blocks -- "${ice[@]}" "${at[@]}"
term terminal-pixels sixel "${sixel[@]}" -- "${ice[@]}" "${at[@]}"
$nqv "${ice[@]}" --png /out/png.png --size 780x800 > /dev/null

if [ -n "$YELMO" ]; then
  gl=(/data/yelmo.nc -v uxy_s -t @-1 --range log,1,3000 --contour "z_srf step=500")
  desktop desktop-proj "${gl[@]}" --proj "ortho -40,72"
  term terminal-proj sixel "${sixel[@]}" -- "${gl[@]}" --proj latlon
  term terminal-proj-blocks blocks -- "${gl[@]}" --proj latlon
fi
ls -la /out/*.png
IN
echo "screenshots in $out; copy the good ones to site/images/"
