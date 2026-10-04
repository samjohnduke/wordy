# Testing accounts, sync and sharing on localhost

Everything the website does for the app (sign-in, linked machines, project
rooms, sharing) can run on this machine against `wrangler dev`, with real
app instances. Nothing reaches the internet: the database, the rooms, the
bucket and the outgoing mail all live under `site/.wrangler/`.

## 1. Start the server

```sh
cd site
pnpm install
cp .dev.vars.example .dev.vars        # set BETTER_AUTH_SECRET to anything long
pnpm wrangler d1 migrations apply DB  # local database (repeat after new migrations)
pnpm preview                          # astro build && wrangler dev → http://localhost:8787
```

Use `pnpm preview` (or `pnpm build` then `pnpm wrangler dev --port 8787`), not
`pnpm dev`: the app talks websockets to the project rooms, which only the
built Worker serves. Rebuild and restart after changing the site.

Mail: with `SITE_URL=http://localhost:8787` the Worker prints every message
it sends in its own terminal (`[mail] to …`), and `pnpm mail` prints the
latest ones with their links pulled out. Nothing is actually delivered, so
any address works, real or made up.

## 2. Passkeys in the browser

Sign-in is passkeys only, so the browser needs an authenticator. Two ways:

- **Chrome DevTools virtual authenticator.** Open DevTools → More tools →
  WebAuthn, tick "Enable virtual authenticator environment", add an
  authenticator (ctap2, internal, "supports resident keys", "supports user
  verification"). It lasts while DevTools stays open on that tab; the
  passkeys it holds are gone when it closes, so keep the tab open for the
  session or add a new passkey next time.
- **A password manager extension** that offers passkeys (Bitwarden,
  1Password) works for localhost too and keeps them.

The headless walk-throughs (`pnpm test:e2e`, `test:e2e:room`,
`test:e2e:share`) use a virtual authenticator of their own and are a good
way to seed a database: `pnpm test:e2e:share` leaves an owner, an editor,
a shared project and a few linked devices behind.

## 3. The first account

1. Open <http://localhost:8787/login>, enter an address, "Send me a link".
2. `pnpm mail` (or the wrangler terminal) shows the magic link; open it.
3. Add a passkey on the page that follows. You land on `/account`.

Each further account needs a different browser profile, an incognito
window, or signing out first (an address that already has a passkey gets no
more email links).

## 4. The app, pointed at localhost

In Wordy, Settings → Account → "Server" (shown while the machine is not linked):
enter `http://localhost:8787`, then "Link this machine". The browser opens
`/device` with the code prefilled; approve it with the passkey. From then on
the app keeps that server in `sync.json` until you unlink.

Then, on a project: "Sync this project". The account page lists it under
Projects, and other linked machines see it under "On the server" in their
Account card.

## 5. A second machine, on the same computer

A second app instance stands in for another machine, or for another
person. It needs its own config (the account token) and, to avoid opening
the same folders, its own projects root:

```sh
WORDY_CONFIG_DIR=/tmp/wordy-b WORDY_PROJECTS_DIR=/tmp/wordy-b/projects cargo run
```

Link it (same account for a second machine, or sign in as a different
account in a second browser profile for a collaborator) and use "Get a
copy" under "On the server" to download a project it has no folder for.
The copy opens in a new window with cloud sync already on; edits in either
window reach the other after a short pause in typing.

## 6. Sharing

1. As the owner, on <http://localhost:8787/account>, invite an address as
   an editor or a reader.
2. `pnpm mail` shows the invitation link. Open it in the invitee's browser
   profile. A new address creates its account (email link, passkey) on the
   way; an existing one signs in with its passkey. "Join" adds the project.
3. In the invitee's app instance, "Refresh" under Account shows the project
   under "On the server" ("shared, editor"); "Get a copy" opens it.
4. Change the role or remove the member on the owner's account page: the
   invitee's open window follows at once (read-only, or "Stopped: you no
   longer have access").

## Starting over

Stop wrangler and delete `site/.wrangler/state` (accounts, rooms, bucket)
and `site/.wrangler/tmp/email` (mail); run the migrations again. On the app
side, unlink each instance (or delete its `sync.json`) and remove
`cloud.json` from any project that should stop syncing.
