# Releasing

How nqv is built, released and updated, as the lab's Tauri apps (papers-write, Papers
Review, Papers Desktop) are.

## The cycle

1. `scripts/version.sh X.Y.Z`: the version in the workspace `Cargo.toml` (all crates and
   the desktop app; `tauri.conf.json` takes it from there), `package.json` and the lock
   files. Committed as "Version X.Y.Z".
2. An annotated tag `vX.Y.Z`, pushed. The tag starts `.github/workflows/release.yml`: a
   draft release, then the builds, each uploading its files to the draft:
   - desktop, Linux (x86-64, `ubuntu-22.04`): `nqv-desktop-linux-amd64.deb` and
     `nqv-desktop-linux-amd64.AppImage`, with their update signatures;
   - desktop, macOS (Apple silicon, `macos-15`): `nqv-desktop-macos-aarch64.dmg`, the app
     as `.app.tar.gz` and its signature;
   - `latest.json`, which the desktop apps read for updates;
   - the terminal viewer: `nqv-linux-x86_64` and `nqv-linux-aarch64` (static, musl, for
     HPC systems) and `nqv-macos-aarch64`.
   File names carry no version, so `releases/latest/download/<name>` always gives the
   latest one (the website links there).
3. A person reads the draft, writes its notes and publishes it. Only then do the apps see
   the update.

The builds are not code-signed or notarized: on macOS the first open is right-click ▸
Open (or `xattr -dr com.apple.quarantine nqv.app`).

HPC hosts still get the terminal viewer with `scripts/deploy.sh HOST` (built locally), or
from the release:

```bash
curl -fsSL https://mathesislab.github.io/nqv/install.sh | sh
```

## Updates of the desktop app

The updater plugin reads
`https://github.com/mathesislab/nqv/releases/latest/download/latest.json` 10 s after the
start and twice a day (not in debug builds, nor with `NQV_NO_UPDATES=1`). An update
found shows "nqv X available · Install" in the toolbar; it installs only when clicked,
then restarts the app.

The updates are signed with a minisign key: its public key is in `tauri.conf.json`
(`plugins.updater.pubkey`), its private key is the repository secret
`TAURI_SIGNING_PRIVATE_KEY` (no password), and a copy is kept outside the repository
(`~/.tauri/nqv.key`). A lost private key means the installed apps cannot be updated: they
need the new version installed by hand once.

On Linux the AppImage updates itself; a `.deb` is updated by installing the new one.

## Checks

`.github/workflows/ci.yml` on each push to main and each pull request, on macOS and
Linux: `npm run check`, the frontend built, `cargo fmt --check`, `cargo clippy -D
warnings`, `cargo test`, and on Linux a smoke test (`scripts/ci/smoke-linux.sh`): the
desktop app opens the test file under xvfb and must stay up without a panic; `nqv -g`,
as from a cluster, hands it a view, which must open, and closing its window must end
`nqv -g`. The screenshots are kept as the run's artifact `smoke-linux`.

`scripts/linux-docker.sh` does the same from a Mac in Ubuntu 22.04 (Docker, e.g. colima),
then builds the `.deb` and the AppImage (without update signatures) into `linux-out/`.

## Linux

The same app. What differs, and why:

- The binary is `nqv-desktop` (`mainBinaryName`): the product name would make it
  `/usr/bin/nqv`, the terminal viewer's name.
- Files to open are the command's arguments (a desktop file's `%F`), made absolute. On
  macOS they also come from the Finder (`RunEvent::Opened`).
- The window opens hidden, in the page's background colour, and is shown once the page
  has drawn (`ready`), or after 3 s: WebKitGTK otherwise shows a half-drawn page.
- Maps are drawn with WebGL2, else with a 2D canvas (`painter.ts`): some drivers and
  virtual machines have no WebGL2. Both were checked under xvfb, with contours over them.
- Copying uses the Clipboard API, else `execCommand("copy")`.
- ssh runs without a terminal (`Prompt::Batch`): it cannot ask for a password or to trust
  a new host, so it fails at once and its message is shown. Hosts need a key in
  ssh-agent, and one login from a terminal.
- A blank window on NVIDIA's driver: start with `WEBKIT_DISABLE_DMABUF_RENDERER=1`.

GitHub's macOS arm64 runners are sometimes short: a job "not acquired by Runner" is
re-run (`gh run rerun <id> --failed`), not a failure of the code.

## The website

`site/` (one page, `index.html` and `style.css`, the screenshots in `site/images/`) is
published to <https://mathesislab.github.io/nqv/> by `.github/workflows/pages.yml` when
it changes on main.
