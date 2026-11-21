#!/usr/bin/env bash
# Smoke test of the desktop app on Linux (as papers-write's): the app, built with its
# frontend, opens the test file under xvfb and a private D-Bus; then `nqv -g`, as from a
# cluster (a fake `hostname`), hands it a view to serve. The app must stay up without a
# panic, and `nqv -g` must end when it does. NQV_SMOKE_SHOT=FILE.png also saves
# screenshots: FILE.png the main window, FILE-handoff.png the handed view (needs
# ImageMagick).
set -euo pipefail

seconds=20
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$root"
target=${CARGO_TARGET_DIR:-$root/target}

# the frontend embedded, not the dev server's
npm run build
cargo build -p nqv-desktop --features tauri/custom-protocol
cargo build -p nqv

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
log=$work/app.log
home=$work/home
mkdir -p "$home/.config" "$work/bin"
printf '#!/bin/sh\necho cluster\n' > "$work/bin/hostname"
chmod +x "$work/bin/hostname"

shot=${NQV_SMOKE_SHOT:-}
shot2=${shot:+${shot%.png}-handoff.png}
data=$root/crates/nqv-proto/tests/data/small.nc
cat > "$work/run.sh" <<RUN
#!/usr/bin/env bash
"$target/debug/nqv-desktop" "$data" &
app=\$!
sleep 8
if [ -n "$shot" ]; then import -window root "$shot" || echo "no screenshot"; fi
PATH="$work/bin:\$PATH" "$target/debug/nqv" -g "$data" -v temp -t @2 --cmap magma > "$work/gui.log" 2>&1 &
gui=\$!
sleep 8
if [ -n "$shot2" ]; then import -window root "$shot2" || echo "no screenshot"; fi
# closing its window (:q, typed with xdotool where there is one) ends nqv -g
if command -v xdotool >/dev/null; then
  win=\$(xdotool search --name '^cluster:' | head -1)
  xdotool windowfocus --sync "\$win" && sleep 1 && xdotool type ':q' && xdotool key Return
  sleep 3
  if kill -0 \$gui 2>/dev/null; then echo "nqv -g still serving after its window closed"
  else wait \$gui && echo "nqv -g ended with its window" || echo "nqv -g failed: \$?"; fi
fi
sleep 2
if kill -0 \$app 2>/dev/null; then
  kill \$app; wait \$app
  if kill -0 \$gui 2>/dev/null || ! command -v xdotool >/dev/null; then
    wait \$gui && echo "nqv -g ended with the app" || echo "nqv -g failed: \$?"
  fi
  exit 124
fi
wait \$app
RUN
chmod +x "$work/run.sh"

# the app runs until stopped (124): a crash ends it earlier
status=0
env HOME="$home" XDG_CONFIG_HOME="$home/.config" WEBKIT_DISABLE_DMABUF_RENDERER=1 \
  dbus-run-session -- xvfb-run -a -s "-screen 0 1400x950x24" "$work/run.sh" \
  > "$log" 2>&1 || status=$?

failures=()
[[ $status -eq 124 ]] || failures+=("exit status $status, expected 124 (still running after ${seconds}s)")
! grep -q panicked "$log" || failures+=("the app panicked")
grep -q "serving its data" "$work/gui.log" || failures+=("nqv -g did not hand its view over: $(cat "$work/gui.log")")
if command -v xdotool >/dev/null; then
  grep -q "nqv -g ended with its window" "$log" || failures+=("nqv -g did not end with its window")
else
  grep -q "nqv -g ended with the app" "$log" || failures+=("nqv -g did not end with the app")
fi

if ((${#failures[@]})); then
  printf 'smoke test failed: %s\n' "${failures[@]}" >&2
  echo "--- output of the app ---" >&2
  cat "$log" >&2
  exit 1
fi
cat "$log"
echo "smoke test passed: the app ran for ${seconds}s, and served a view handed over by nqv -g"
