#!/usr/bin/env bash
# Smoke test of the desktop app on Linux (as papers-write's): the app, built with its
# frontend, opens the test file under xvfb and a private D-Bus; it must stay up without a
# panic. NQV_SMOKE_SHOT=FILE also saves a screenshot of the screen (needs ImageMagick).
set -euo pipefail

seconds=20
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$root"
target=${CARGO_TARGET_DIR:-$root/target}

# the frontend embedded, not the dev server's
npm run build
cargo build -p nqv-desktop --features tauri/custom-protocol

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
log=$work/app.log
home=$work/home
mkdir -p "$home/.config"

shot=${NQV_SMOKE_SHOT:-}
cat > "$work/run.sh" <<RUN
#!/usr/bin/env bash
"$target/debug/nqv-desktop" "$root/crates/nqv-proto/tests/data/small.nc" &
app=\$!
sleep $((seconds - 4))
if [ -n "$shot" ]; then import -window root "$shot" || echo "no screenshot"; fi
sleep 4
if kill -0 \$app 2>/dev/null; then kill \$app; wait \$app; exit 124; fi
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

if ((${#failures[@]})); then
  printf 'smoke test failed: %s\n' "${failures[@]}" >&2
  echo "--- output of the app ---" >&2
  cat "$log" >&2
  exit 1
fi
cat "$log"
echo "smoke test passed: the app ran for ${seconds}s"
