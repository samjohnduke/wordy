# Wordy

A private, personal fiction-writing app: manuscript, world and notes in one
window, rich text on a Loro CRDT document, and manual file-level sync between
two machines on the same network. Native (Rust, gpui) on macOS and Linux.
`PLAN.md` is the design and the build log.

## Build

```sh
cargo build            # debug build: target/debug/wordy
cargo run              # opens the most recent project under ~/Wordy
cargo run -- ~/Wordy/My\ Novel   # or a specific project folder
```

Linux needs the usual gpui development packages (Wayland, X11, fontconfig,
freetype, ALSA, Vulkan, OpenSSL). The exact apt list is in
`.github/workflows/ci.yml`. macOS needs only Xcode's command line tools.

## Checks

CI runs on `macos-latest` and `ubuntu-latest` and gates on all four:

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo build --all-targets
cargo test --all-targets
```

## Release checklist

1. CI green on both operating systems.
2. **Manual: the Mac binary launches.** CI builds and tests on macOS but
   cannot open a window. On an Apple Silicon Mac, from a clean checkout:

   ```sh
   cargo build --release
   ./target/release/wordy /tmp/launch-check
   ```

   Expect a window titled "Wordy — launch-check" with the empty-manuscript
   hint, type a sentence into a new scene, quit with Cmd-Q, relaunch with
   the same path and see the sentence again. Delete `/tmp/launch-check`.
3. Run the two-host sync checklist in [`docs/sync-test.md`](docs/sync-test.md).
4. Bump `version` in the root `Cargo.toml`, add `site/src/content/changelog/<version>.md`,
   then push a `v<version>` tag. `.github/workflows/release.yml` builds the Linux
   tarball and the macOS app zip and publishes a GitHub Release with the changelog
   entry as its notes. Redeploy the website afterwards so the download page picks
   the release up.

## Install

Linux, per user (no root): builds a release binary if needed and puts the
command, the launcher entry and the icon under `~/.local`.

```sh
packaging/linux/install.sh             # add --uninstall to remove it again
```

macOS: builds `target/release/Wordy.app` (unsigned) from the same sources.

```sh
packaging/macos/bundle.sh
```

The icon is `packaging/wordy.svg`; the PNGs next to the scripts are rendered
from it with `rsvg-convert` and checked in so neither script needs it.

## Website

`site/` is the public site (landing page, downloads, changelog): Astro on Cloudflare
Workers, deployed with Wrangler. The download page reads the latest GitHub Release at
build time, so it needs no changes per release.

```sh
cd site
pnpm install
pnpm dev              # http://localhost:4321
pnpm preview          # production build served by wrangler dev
pnpm deploy           # astro build && wrangler deploy (needs `pnpm wrangler login` once)
```

## Data on disk

Each project is a folder (default under `~/Wordy/`):

| File | What |
| --- | --- |
| `project.loro` | the project: a Loro snapshot, the only file that matters |
| `project.json` | readable mirror of the current state, rewritten at most every 30 s and on quit |
| `snapshots/` | rolling backups, taken on explicit save, every 30 minutes of autosave, and before any history compaction |
| `assets/` | attachments by content hash |
| `index.sqlite` | derived search index; safe to delete |

On launch, a project whose edit history reaches back more than a year is
backed up to `snapshots/` and compacted automatically; the status bar says
so once.
