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

## Website and accounts

`site/` is the public site (landing page, downloads, changelog), the account server
(sign-in, passkeys, linked machines) and the sync server (one Durable Object per project,
updates and attachments in R2): Astro on Cloudflare Workers, deployed with Wrangler. The
download page reads the latest GitHub Release at build time, so it needs no changes per
release. Accounts live in D1 (better-auth), sign-in is passkeys only after the first email
link, and email goes out through Cloudflare's `send_email` binding.

```sh
cd site
pnpm install
cp .dev.vars.example .dev.vars          # then set BETTER_AUTH_SECRET
pnpm wrangler d1 migrations apply DB    # local database for dev and tests
pnpm dev                                # http://localhost:4321
pnpm preview                            # production build served by wrangler dev
pnpm test:e2e                           # headless Chromium walk-through against :8787
pnpm test:e2e:room                      # project rooms over websockets against :8787
pnpm test:e2e:share                     # invitations, roles and removal against :8787
pnpm deploy                             # astro build && wrangler deploy
```

First deployment, once: `pnpm wrangler login`, `pnpm wrangler d1 create wordy` (put the id
in `wrangler.jsonc`), `pnpm wrangler d1 migrations apply DB --remote`,
`pnpm wrangler r2 bucket create wordy-projects`, `pnpm wrangler secret put
BETTER_AUTH_SECRET`, and verify the sending domain under Email Sending in the Cloudflare
dashboard. Locally, `wrangler dev` writes outgoing mail under `.wrangler/tmp/email/`
instead of sending it, and keeps the rooms and the bucket under `.wrangler/state/`.

In the app, Sync → Account → "Link this machine" opens the browser at `/device` with a
code; approving it there with a passkey gives the app a bearer token, stored in
`sync.json` (owner-readable only). "Sync this project" then keeps the open project in its
room on the server: every save sends the new edits, edits from other machines land after
a short pause in typing without moving the caret, custom words and attachments follow,
and the project keeps working offline (changes go up on reconnect). `cloud.json` in the
project folder holds the switch and the sync position. The Rust client is
`wordy-sync::cloud`; its tests run against a local `wrangler dev --port 8787` and skip
when there is none.

Sharing happens on the website: under Account, each synced project lists its members, and
the owner can invite an address as an editor or a reader. The emailed link signs the
invitee in, or creates their account (with a passkey) if they are new, and adds the
project to their account; a linked machine then opens it like any of its own. Readers
see every change live but their own edits stay on their machine; the owner can change a
role or remove someone at any time, and open connections follow suit immediately.

## Data on disk

Each project is a folder (default under `~/Wordy/`):

| File | What |
| --- | --- |
| `project.loro` | the project: a Loro snapshot, the only file that matters |
| `project.json` | readable mirror of the current state, rewritten at most every 30 s and on quit |
| `snapshots/` | rolling backups, taken on explicit save, every 30 minutes of autosave, and before any history compaction |
| `assets/` | attachments by content hash |
| `cloud.json` | whether the project syncs to the account, and how far it got |
| `index.sqlite` | derived search index; safe to delete |

On launch, a project whose edit history reaches back more than a year is
backed up to `snapshots/` and compacted automatically; the status bar says
so once.
