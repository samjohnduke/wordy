# Wordy — Implementation Plan

A private, native, offline writing desk for fiction. Rust + gpui + gpui-component, with Loro as the
document model. Mac and Linux only. Single user. No website, no Windows, no Scrivener import.

Reference feature set: EmberWrite (spaces, act/chapter/scene tree, wiki with templates, entity
linking, scene versioning, tags, split panes, goals and stats, tasks, attachments, snippets,
docx/pdf/epub export, sync).

---

## 1. Goals and non-goals

**Goals**
- True rich-text prose editor with a stable document model (Loro rich text).
- Wiki-style entity linking: explicit links stored as marks, auto-detected mentions as decorations,
  backlinks indexed.
- Manuscript hierarchy (Act / Chapter / Scene), World space (characters, locations, cultures,
  systems, objects), Notes space.
- Lossless storage with full history (Loro snapshot), JSON mirror for inspection.
- Sync between machines through an account, conflict-free via CRDT merge (§13; the
  original LAN sync of §9 was removed on 2026-10-04).
- Mac (Apple Silicon) and Linux (Wayland, X11 fallback) builds.

**Non-goals**
- Windows, mobile, web, collaboration with other people, plugins, user-written themes,
  inline images, tables, nested lists, footnotes, Scrivener import, public roadmap.
  (A public website with downloads and a changelog was added in Phase 14. Phases 15–17
  add accounts and cloud sync, so "collaboration with other people" stops being a
  non-goal there; the web still gets no editor, only account pages, and the app keeps
  working fully offline.)

---

## 2. Stack (verified 2026-10-03)

| Concern | Crate | Version | Notes |
|---|---|---|---|
| UI framework | `gpui-pre` (alias as `gpui`) | =0.3.7 | pinned by gpui-component; Mac + Linux platform backends |
| Components | `gpui-component` | 0.7.0 | dock/splits/tabs, tree, inputs, markdown, theming |
| Document model / CRDT | `loro` | 1.16 | rich text, movable tree, undo, snapshots, JSON updates |
| Entity detection | `aho-corasick` | 1.1 | multi-pattern name/alias matching |
| Grapheme/word boundaries | `unicode-segmentation` | 1.13 | cursor movement, word counts |
| Spellcheck | `spellbook` | 0.4 | Hunspell-compatible, pure Rust |
| Index / stats / search | `rusqlite` (bundled) | 0.40 | derived data only, rebuildable |
| Word export | `docx-rs` | 0.4 | |
| ePub export | `epub-builder` | 0.8 | |
| PDF export | `typst` (+ `typst-pdf`) | 0.15 | manuscript template compiled in-process |
| Snippet images | `tiny-skia` + `cosmic-text` | 0.12 / 0.19 | render quote to PNG |
| Serialization | `serde`, `serde_json` | | settings, JSON mirror |
| Editor/PDF font | Libertinus Serif (OFL) | | bundled as embedded asset |

Pin `gpui-pre` and `gpui-component` together and bump them together. Expect API churn.

Cargo alias so code reads naturally:
```toml
gpui = { package = "gpui-pre", version = "=0.3.7" }
```

---

## 3. Architecture

Workspace with these crates:

```
wordy/
  crates/
    wordy-doc       # Loro schema, typed accessors, paragraph model, undo, save/load, migrations
    wordy-index     # SQLite index: words, backlinks, tags, search, stats; rebuilt from doc
    wordy-editor    # gpui prose editor element (layout, input, selection, decorations)
    wordy-export    # docx / epub / pdf / snippet png
    wordy-sync      # account link, project rooms over websockets, fetching copies
    wordy-app       # gpui-component shell: spaces, panels, views, commands, settings
```

Dependency direction: `app -> editor, index, export, sync -> doc`. Nothing depends on `app`.

Principle that drives everything: **stored data vs derived data**.
- Stored: the Loro document (text, marks, tree, metadata). This is the only source of truth.
- Derived: SQLite index, auto-link decorations, spell squiggles, search hits, stats, backlinks.
  All rebuildable from the doc at any time. Never written back into the doc.

---

## 4. Data model (Loro)

One `LoroDoc` per project. One project file on disk.

### 4.1 Containers

```
root map "project"
  name: String
  created: i64
  settings: Map { daily_goal, manuscript_goal, deadline, ... }

tree "nodes"                      # LoroTree — all hierarchy for all spaces
  node.meta (LoroMap):
    kind: "act" | "chapter" | "scene" | "folder" | "note" | "entity"
    title: String
    space: "manuscript" | "world" | "notes"
    status: "idea" | "draft" | "revised" | "final"    (scenes)
    word_goal: i64?                                   (scenes/chapters)
    tags: LoroList<String>
    body: LoroText                                    # rich text (scene, note, entity description)
    # entity-only fields:
    template: "character" | "location" | "culture" | "system" | "object" | "custom"
    aliases: LoroList<String>
    fields: LoroMap<String, LoroText | String>        # template-driven sheet fields
    relations: LoroList<Map { to: TreeID, kind: String, note: String }>
    attachments: LoroList<Map { name, path (relative to assets/), mime }>
    include_in_compile: bool                          (scenes)

map "tasks"                       # project-wide todo list
  <ulid>: Map { text, done: bool, created, node: TreeID? }

map "comments"                    # inline comments, referenced by comment marks
  <ulid>: Map { text: LoroText, created: i64, resolved: bool }

map "versions"                    # named scene versions (bookmarks into history)
  <ulid>: Map { node: TreeID, label, frontiers: bytes, created }

map "sessions"                    # daily writing sessions (for stats)
  <YYYY-MM-DD>: Map { words_start, words_end, seconds }
```

Entity ids are `TreeID`s. Links hold a TreeID, never a name. Renames never break links.

### 4.2 Rich text conventions (Quill-style)

- One `LoroText` per body. Paragraphs are separated by `\n`.
- Inline marks (configured via `config_text_style`):
  - `bold`, `italic`, `underline`, `strike`, `smallcaps` — expand **after**
  - `highlight: color-name` — expand **after**
  - `comment: ulid` — expand **none**
  - `link: TreeID` — expand **none**
- Paragraph marks live on the `\n` character, expand **none**:
  - `block: "p" | "h1" | "h2" | "h3" | "quote" | "break"`
  - `align: "center"` (headings / scene breaks only)
- `wordy-doc` exposes a `Paragraphs` view: splits `to_delta()` on `\n` into
  `Vec<Paragraph { block, runs: Vec<Run { text, marks }> }>` with byte-offset mapping tables
  (Loro indexes in Unicode code points; gpui wants bytes).
- Text normalized to NFC on input. Whitespace preserved exactly.

### 4.3 Storage

```
~/Wordy/<Project>/
  project.loro          # binary snapshot — the truth
  project.json          # derived JSON content mirror (get_deep_value), for grep/debugging
  assets/               # attachments, copied in by content hash name
  dictionary.txt        # custom words (app-level copy also kept)
  index.sqlite          # derived, safe to delete
  snapshots/            # rolling backups: project-YYYYMMDD-HHMM.loro (keep last 10 + daily 30)
```

- Autosave: debounce 1.5s after last edit, atomic write (tmp + rename).
- On open: import snapshot, rebuild index if `index.sqlite` missing or stale.
- History pruning: shallow snapshot at startup if history older than 365 days.
- Undo: Loro `UndoManager`, merge interval 500ms, exclude remote (sync) ops.

---

## 5. Editor design (`wordy-editor`)

The single biggest piece. Build it as a reusable gpui element that edits any `LoroText`.

### 5.1 Layers
1. **Model** — `LoroText` + `Paragraphs` view (from wordy-doc). Subscribes to Loro events and
   re-derives only the affected paragraphs.
2. **Decorations** — computed per paragraph, not stored:
   - auto-links (entity mentions), ambiguous mentions
   - spell errors
   - search matches
   - selection, cursor, composition range (IME)
3. **Layout** — each paragraph shaped with gpui's text system into wrapped lines with styled runs
   = marks ∪ decorations. Cache shaped lines per paragraph; invalidate on edit or width change.
4. **Input** — implements gpui's `EntityInputHandler` (IME, marked text, dead keys, dictation).
   Keybindings via gpui actions. Mouse: click/drag/double/triple select via line x→index mapping.

### 5.2 Behaviors (in order of implementation)
- Insert / delete / newline with paragraph mark carry-over (Enter in heading → next is `p`).
- Cursor movement by grapheme, word, line, paragraph; Home/End; Shift-extends.
- Selection rendering across wrapped lines and paragraphs.
- Mark toggles: Cmd/Ctrl-B/I/U, strike, highlight picker, clear formatting.
- Block type switch: paragraph, headings, quote, scene break (`***` auto-converts).
- Smart typography on input: curly quotes, em dash from `--`, ellipsis.
- Undo/redo via Loro UndoManager; restore selection from undo metadata.
- Copy/paste: plain text + custom `application/x-wordy-delta` for in-app rich paste.
- Link insertion: select text → Cmd-K → entity picker (fuzzy over names/aliases) → `link` mark.
  Typing `[[` opens the same picker inline.
- Click link → open entity in reference pane; Cmd-click → navigate.
- Find/replace within document; project-wide search lives in app using the index.
- Spellcheck: on-idle per paragraph, squiggle decorations, right-click suggestions, add to dictionary.
- Focus mode: hide chrome, typewriter scrolling (optional).

### 5.3 Testing
- `wordy-doc` unit tests for every edit op using a text-only fake renderer.
- Property tests: random edit sequences keep `Paragraphs` consistent with `to_delta()`.
- Golden tests: delta → paragraphs → runs byte offsets.

---

## 6. Entity linking and the index (`wordy-index`)

- Build an Aho-Corasick automaton from all entity names + aliases (case-insensitive, whole-word
  boundaries checked post-match). Rebuild when any entity name/alias changes.
- Scan a paragraph on edit (debounced ~150ms) → mention decorations. Unique match = auto-link
  style; multiple candidates = ambiguous style with click-to-pin (converts to explicit `link` mark).
- Backlinks table: `(entity_id, node_id, kind: explicit|auto, count)` rebuilt per node on save.
- Entity sheet shows "Appears in" from backlinks; scene list can filter by entity (this covers
  most of EmberWrite's tagging; explicit `tags` remain for free-form labels like POV or timeline).
- Rename entity → optional scoped replace over explicitly-linked text ranges only.

SQLite tables: `nodes(id, kind, space, title, status, words, updated)`, `backlinks`, `tags`,
`fts_body` (FTS5 over plain text), `daily_words(date, words)`.

---

## 7. App shell (`wordy-app`)

gpui-component dock layout, persisted per project.

- **Left rail**: space switcher (Home, Manuscript, World, Notes) + settings.
- **Sidebar**: tree for current space (gpui-component Tree), drag-reorder → `LoroTree::mov`.
  Context menu: new, rename, duplicate, status color, delete (to trash folder, not hard delete).
- **Center**: tabbed editor panes; split horizontally/vertically; drag tabs between panes.
- **Right dock**: two tabs, toggled with ⌘⇧R. *Reference*: pinned entity sheet / note /
  previous version, read-only; double-click opens the node in an editor tab. *Sheet*: the
  editable sheet of the entity in the active editor (Phase 19).
- **Home space**: Dashboard (today's words, streak, goal progress, deadline pace),
  Reports (words per day chart, per-chapter table), Tasks (checklist), Placeholders
  (list of `[TODO ...]` / `[[?]]` markers found in text), Export.
- **Entity sheet**: template-driven fields (character: role, physical, personality, backstory,
  pronunciation; location: demographics, description; etc.), image attachment shown at top,
  Relations tab with linked entities, Appears-in tab from backlinks.
- **Versions**: "Save version" bookmarks current frontiers with a label; "Compare" opens a
  read-only checkout in the reference pane; "Restore" copies old text into current body
  (as a normal edit, so it's undoable).
- **Status bar**: words in doc / chapter / manuscript, session words, spell language.
- **Command palette** (⌘K): all actions, fuzzy.

---

## 8. Export (`wordy-export`)

Compile = walk manuscript tree in order, include scenes where `include_in_compile`, map
paragraphs → target format.

- **docx**: standard manuscript format (12pt serif, double-spaced, chapter headings, `#` scene
  breaks). Marks → runs. Links → plain text.
- **epub**: one XHTML file per chapter, CSS for italics/bold/small caps, cover image optional.
- **pdf**: typst template embedded as a string; compile in-process with a virtual filesystem
  (`typst::World` impl); fonts bundled.
- **Snippet**: select text → render to 1080x1080 or 1200x630 PNG with cosmic-text on tiny-skia,
  dark/light variants, copied to clipboard and saved.
- **Whole project**: zip of the project folder (that's the backup format too).

---

## 9. LAN sync (`wordy-sync`) — removed 2026-10-04

Built in Phase 6 and taken out again in Phase 18 once account sync (§13) covered the
same ground without a second machine on the same network. Kept here as the record of
what it was.

Manual, two-machine, conflict-free.

1. Both machines advertise `_wordy._tcp` via mDNS with a project id and peer name.
2. User presses **Sync** → picks the peer → TCP connection, pre-shared pairing code typed once
   and stored (plain TCP on LAN is acceptable for a private tool; upgrade to TLS later if wanted).
3. Protocol: exchange version vectors → each side `export(Updates { from: other_vv })` → both import.
   Loro merges; no conflict copies ever.
4. Assets: exchange a manifest of `assets/` by hash; copy missing files both ways.
5. Also exchange `dictionary.txt` (union).
6. After sync: rebuild affected index rows, save snapshot, show summary (n changes in, n out).

Fallback with zero code: copy `project.loro` from the other machine and use **Import snapshot**
— Loro merges it the same way.

---

## 10. Phases and milestones

Each phase ends with something usable. Don't start the next until the acceptance line holds.

### Phase 0 — Skeleton (1 week)
- Cargo workspace, crates stubbed, CI builds on macOS + Ubuntu (Wayland + X11 runtime check).
- gpui-component window with dock layout, placeholder sidebar/center/right panels, light/dark.
- Bundle Libertinus Serif as an embedded asset; register with the text system.
- `wordy-doc`: create/open/save project; LoroTree with Manuscript/World/Notes roots.
- **Accept:** app opens, creates a project file, restarts and reloads it.

### Phase 1 — Plain editor on Loro (3–4 weeks)
- `Paragraphs` view with byte/codepoint mapping + tests.
- Editor element: text insert/delete, paragraphs, cursor, selection, mouse, scrolling, IME.
- Undo/redo via UndoManager.
- Scene tree: create/rename/reorder/delete scenes, open in tabs; status + include-in-compile
  toggles in context menu.
- Autosave + rolling snapshots. Word counts in status bar.
- **Accept:** draft a chapter for a week in it without reaching for another app.

### Phase 2 — Rich text + blocks (2 weeks)
- Marks: bold/italic/underline/strike/smallcaps/highlight. Paragraph marks: headings/quote/break.
- Smart typography. Copy/paste (plain + rich in-app).
- Inline comments: add on selection, margin display, resolve/delete.
- Find/replace in document.
- **Accept:** a formatted scene round-trips through save/load and undo with no loss.

### Phase 3 — World space + linking (3 weeks)
- Entity nodes with templates and fields; entity sheet view; attachments (open externally).
- Explicit links (⌘K picker, `[[`), link mark rendering, click → reference pane.
- Aho-Corasick auto-link decorations; ambiguity + pin.
- SQLite index: backlinks, FTS, per-node word counts. "Appears in" on sheets.
- Split panes + reference pane wired up.
- **Accept:** add a character, every past mention lights up; rename keeps explicit links intact.

### Phase 4 — Momentum tools (2 weeks)
- Goals (daily, manuscript, deadline → required pace), sessions, streak.
- Dashboard + Reports (simple bar chart via gpui-component charts).
- Tasks list; Placeholders scan; scene status colors in tree; tags + filter.
- Scene versions (bookmark / compare / restore).
- Spellcheck with spellbook + custom dictionary.
- **Accept:** dashboard matches a manual count; a saved version can be viewed and restored.

### Phase 5 — Export (1–2 weeks)
- docx, epub, typst PDF, snippet PNG, project zip.
- **Accept:** compiled manuscript opens cleanly in Word/Pages, Apple Books/Calibre, and a PDF viewer.

### Phase 6 — LAN sync (1–2 weeks) *(built, later removed in Phase 18)*
- mDNS discovery, pairing, update exchange, asset manifest, dictionary union.
- **Accept:** edit the same scene on both machines offline, sync, both converge with both edits.

### Phase 7 — Polish (done 2026-10-03)
- Focus mode, typewriter scroll, keyboard-only navigation, performance pass on long scenes,
  history pruning, crash-safe recovery from `snapshots/`.

---

## 10b. Gap audit (2026-10-03) and follow-on phases

Phases 0–7 shipped. An audit against §5–§9 and the EmberWrite feature list found the items
below either missing or shallow. The translucent window background (wallpaper visible behind the
sidebar and editor) is **intentional** and stays.

### Phase 8 — Structure and navigation (done 2026-10-03)
- Visible hierarchy in the sidebar tree: indent per depth, chevrons, collapse/expand state
  remembered per project. Acts/chapters visually distinct from scenes.
- Dashboard groups scenes under their chapter; root-level scenes go under "Unsorted".
- Drag-and-drop reorder in the tree: drop between siblings or onto a container → `move_before` /
  `move_after` / `move_node`. Keyboard Move Up/Down stays.
- Duplicate node (deep: body, marks, metadata, children) from the context menu.
- Layout persistence per project in `layout.json` (derived, safe to delete): current space, open
  tabs + active tab, reference pane open/pinned node, sidebar width, typewriter and focus mode.
- **Accept:** restart the app and land exactly where you left off; drag a scene into another
  chapter and the Dashboard reflects it.

### Phase 9 — Editor commands and discoverability (done 2026-10-03)
- Formatting toolbar above the editor: block-type dropdown (Paragraph / H1 / H2 / H3 / Quote /
  Scene break), bold / italic / underline / strike / small caps, highlight colour menu, comment,
  link, clear formatting. Reflects the style at the caret.
- Clear formatting action (removes all inline marks in the selection).
- Highlight colours: yellow, green, blue, pink, grey. Picker in the toolbar and via ⌘⇧H cycling.
- Command palette: Ctrl-P becomes a combined palette — nodes plus every action (`> ` prefix or
  auto-mixed), fuzzy, with keybinding hints.
- Status bar shows the spell language; clicking it toggles spellcheck off/on for the project.
- Typewriter and focus mode persisted (via Phase 8 layout.json).
- **Accept:** every editor command is reachable with the mouse and from the palette.

### Phase 10 — World space depth and versions (done 2026-10-03)
- Entity sheet tabs: Fields / Relations / Appears in. Image attachments render inline at the top
  of the sheet (first image = portrait).
- "Appears in" rows filter the Manuscript sidebar to scenes mentioning that entity (clear button
  in the search box).
- Rename entity → prompt "Also replace N linked mentions in the text?" → replaces text under
  explicit `link` marks only, as one undoable edit per scene.
- Version compare: reference pane shows the saved version with word-level diff highlighting
  (inserted = green, deleted = red strikethrough) against the live body.
- **Accept:** rename a character and every linked mention updates; compare shows what changed.

### Phase 11 — Visual design pass (done 2026-10-03)
- Left rail: real icons (home, book, globe, note) with tooltips and a clear active state;
  settings/theme icon at the bottom instead of the "Dark" text button in the title bar.
- Typographic hierarchy: panel headers, meta strip and body use distinct sizes/weights; consistent
  8px spacing scale; dashboard cards get titles, subtle borders and aligned numeric columns.
- Empty states with an action: new project → "Create your first chapter" button; empty World →
  "Add a character"; empty Notes → "New note".
- Reports: words per day (30 days), per week (26 weeks), and per session list; chapter table with
  goal progress bars.
- **Accept:** screenshots of every space look like one app, not a debug UI.

### Phase 12 — Infrastructure and robustness (done 2026-10-03)
- GitHub Actions: build + test on `macos-latest` (Apple Silicon) and `ubuntu-latest`; clippy
  and fmt gates. Mac binary verified to launch at least once (manual step documented).
- Startup pruning: if the oldest change is older than 365 days, take a backup and shallow-snapshot
  automatically; log it and show it once in the status bar.
- JSON mirror written at most once per 30 s and on quit, not on every autosave.
- IME tests: `replace_and_mark_text_in_range` / `unmark_text` round-trips for composition,
  dead keys, and replacing a marked range with a selection present.
- **Accept:** CI green on both OSes; a year-old project opens without manual compaction.

### Phase 13 — Loose ends after the gap audit (code done 2026-10-03; *(manual)* items open)
Found after Phase 12; items marked *(manual)* need another machine or a human and are tracked
here rather than done in code.
- Window close saves: the title-bar close button and the window manager's close request go
  through a `should_close` hook that flushes the snapshot and JSON mirror before the window
  goes away. The global `Quit` action, Cmd-Q and the Mac menu run the same flush from
  `on_app_quit`, so the two paths cannot drift.
- Chapter rows behave the same everywhere: a chapter has no body, so opening it from the
  dashboard or Reports reveals it in the Manuscript sidebar (space switched, ancestors
  expanded, row selected) instead of creating an editor tab for an empty container.
- Linux packaging: `packaging/linux/` holds `dev.sam.wordy.desktop` (the name must equal the
  window `app_id` so Wayland compositors match the icon), an SVG icon rendered to hicolor PNGs,
  and `install.sh` for a per-user install under `~/.local` (`--uninstall` reverses it).
- macOS bundle: `packaging/macos/bundle.sh` builds a release binary and wraps it in
  `Wordy.app` with `Info.plist` and an `.icns` made with `sips` + `iconutil`. *(manual: run
  and open on a Mac; this machine cannot.)*
- First real CI run (done 2026-10-03): the apt package list in `ci.yml` was written blind and
  held up; the only breakage was an unquoted colon in a step name. Both `ubuntu-latest` and
  `macos-latest` pass fmt, clippy, build and test (26 and 34 minutes cold). *(manual, still
  open)* the Mac binary has not been launched by a person.
- *(manual)* HiDPI and fractional scaling: check text crispness and hit targets at 1.5× and 2×
  on both OSes.
- No UI-level tests: gpui's `test-support` feature would rebuild gpui for the test profile, so
  UI logic stays in gpui-free modules (`wordy_editor::ime`, `wordy_doc`) that are unit-tested.
- Local disk sits at 97%; `target/` is 1.7 GB per profile. `cargo clean -p wordy-app` before
  release builds if space runs short. *(manual, this machine only)*
- **Accept:** closing the window with the mouse loses no text; clicking a chapter anywhere lands
  on the same sidebar row; `install.sh` puts Wordy in the launcher with its icon.

Rough total: 3–4 months of evenings/weekends; faster if full-time. The editor (Phases 1–2) is
roughly half the effort.

---

## 11. Risks and mitigations

| Risk | Mitigation |
|---|---|
| gpui / gpui-component API churn | Pin exact versions; upgrade only between phases; keep UI code thin over `wordy-editor`. |
| Editor correctness (IME, selection, wrapping) | Build on Loro deltas with exhaustive doc-level tests; test on a real Mac and Linux box from Phase 1. |
| Loro Unicode-codepoint indexing vs byte offsets | Single mapping table per paragraph in `wordy-doc`; never index Loro from UI code directly. |
| Linux HiDPI / Wayland quirks | Test early; keep X11 fallback; avoid fractional scaling assumptions. |
| History growth | Shallow snapshot pruning; rolling backups. |
| Single-file project corruption | Atomic writes + rolling snapshots + JSON mirror. |
| Scope creep | Phase gates with acceptance lines; non-goals list is binding. |

---

## 12. Decisions (resolved 2026-10-03)

1. **Project location:** fixed at `~/Wordy/`, one subfolder per project. No folder picker.
2. **Fonts:** bundle a single open serif (Libertinus Serif) used by both the editor and the typst
   PDF template, so output is identical on Mac and Linux. Embed via gpui asset source.
3. **Comments:** inline only. `comment: ulid` mark on a text range; comment text lives in a
   `comments` map (`<ulid>: Map { text: LoroText, created, resolved: bool }`). Shown in the margin
   beside the range; resolved comments hidden by default.
4. **Reference pane:** read-only. Renders the same `Paragraphs` view without an input handler.
   Double-click or "Open" button opens the node in an editor tab.
5. **Include in compile:** added in Phase 1 with the scene tree as a checkbox in the scene
   context menu and inspector. Export reads it in Phase 5.

### Phase 14 — Website and releases (2026-10-04)

- `site/`: Astro 7 + `@astrojs/cloudflare`, every page prerendered, served by a Cloudflare
  Worker from static assets (`wrangler.jsonc`, name `wordy-site`). Pages: landing (feature
  copy and an HTML illustration of the window, since no clean screenshot exists yet),
  `/download`, `/changelog`, `/changelog.xml` (RSS), 404. Libertinus Serif from
  `assets/fonts` for headings; palette from the icon.
- Changelog entries are `site/src/content/changelog/<version>.md` (front matter: version,
  date, title). `0.1.0.md` summarises Phases 0–13.
- `/download` fetches the latest GitHub Release at build time (`GITHUB_TOKEN` optional) and
  falls back to "not released yet" plus build-from-source instructions. Asset names are
  versionless so `releases/latest/download/<name>` stays valid.
- `.github/workflows/release.yml`: on a `v*` tag, checks the tag against the workspace
  version, builds `wordy-linux-x86_64.tar.gz` (binary + `install.sh` + desktop file + icons)
  on ubuntu-latest and `Wordy-macos-arm64.zip` (via `bundle.sh` and `ditto`) on
  macos-latest, with `.sha256` files, and publishes a GitHub Release whose body is the
  changelog entry. `install.sh` now also works from inside the tarball.
- *(manual)* Deploy: `cd site && pnpm wrangler login && pnpm deploy`. Then optionally
  uncomment `routes` in `site/wrangler.jsonc` for a custom domain and set `site` in
  `astro.config.mjs` to match. The first tag (`v0.1.0`) has not been pushed.
- *(manual)* Replace the HTML window illustration with a real screenshot
  (`site/public/screenshot.png`) from a demo project once one exists.

**Accept:** `pnpm build` and `pnpm check` clean; `wrangler dev` serves all routes (verified
2026-10-04 with headless Chromium at desktop, mobile and dark); release.yml parses and the
tarball layout matches what `install.sh` expects.

### Phase 15 — Accounts, passkeys and device linking (2026-10-04)

Cloud sync needs identities first. The site Worker grows server-rendered account pages
and an auth API; the app learns to link itself to an account.

- better-auth on D1 (`DB` binding, migrations in `site/migrations`), plugins: magic link
  (first sign-in only), `@better-auth/passkey`, bearer tokens, device authorization.
  `BETTER_AUTH_SECRET` is a Worker secret; `SITE_URL` and `EMAIL_FROM` are vars.
- Email goes through the Cloudflare `send_email` binding (`EMAIL`). Local `wrangler dev`
  writes outgoing mail to `.wrangler/` instead of sending, which the tests read.
- Sign-in policy: an email link is accepted only for an address with no passkey yet. Once
  a passkey exists the account is passkeys-only; the login page offers nothing else. A
  session without a passkey can only reach the passkey set-up page. Losing every passkey
  means deleting the account's passkey rows by hand (`wrangler d1 execute`); there is no
  email recovery on purpose.
- Pages (`prerender = false`): `/login`, `/account` (passkeys, devices, sign out),
  `/account/passkey` (first passkey), `/device` (approve a code), `/api/auth/*`,
  `/api/devices` (register and list devices for a bearer session). A `device` table
  records name, platform and last-seen per session so the account page can list and
  revoke machines.
- App: the Sync page gains an Account card. "Link this machine" runs the device-code flow
  in `wordy-sync::cloud` (ureq over rustls, blocking, on its own thread), opens the browser
  at `/device?user_code=…`, polls `/api/auth/device/token`, registers the device, and
  stores the bearer token in `sync.json` (mode 0600). The card lists the account's
  machines (remove the others), refreshes quietly at launch, drops the token when the
  server answers 401, and Unlink signs the session out. The server field is editable until
  linked (defaults to `https://wordy.samduke.dev`).
- *(manual)* Create the D1 database and apply migrations remotely, set the secret, verify a
  sender address for Email Sending, deploy.

**Accept:** a CDP script against `wrangler dev` with a virtual authenticator walks sign-up
by email, passkey registration, sign-out, passkey sign-in, device approval and device
registration; `pnpm check` clean; `cargo test` passes for the cloud client against the
local server. *(Done: `pnpm test:e2e` and `crates/wordy-sync/tests/cloud.rs`, the latter
skipping without a local server; its `full_link_flow` test has the browser script approve
the code with a virtual authenticator, so the app's client is covered end to end.)*

### Phase 16 — Project rooms and live sync (2026-10-04)

One room per project on the server; the app keeps a socket to it while the project is
open and edits flow both ways as you type.

- One Durable Object per project (`ProjectRoom`, partyserver with hibernation, in
  `site/src/server/rooms/`). Its SQLite holds the ordered update log (small updates
  inline, big ones as R2 keys), head and base sequence numbers, the dictionary, the asset
  manifest and per-device last-seen numbers. R2 (`PROJECTS`, bucket `wordy-projects`)
  holds updates over 512 KiB, snapshots and content-addressed assets. The server never
  runs Loro: it only orders and stores bytes. D1 gains `project` and `membership`
  (migration `0002`); the first device to open a project id claims it as owner.
- `site/src/worker.ts` wraps the Astro handler: `/parties/project-room/<id>` upgrades are
  authenticated with the device's bearer token, checked against membership, and handed
  to the room with an identity header. HTTP: `GET /api/projects`, `PUT|GET
  /api/projects/<id>/updates[/<seq>]` (large updates, with `x-wordy-snapshot-at` for
  compaction), `PUT|GET /api/projects/<id>/assets/<sha256>`.
- Protocol (`rooms/protocol.ts`, mirrored in `wordy-sync::cloud::protocol`): JSON text
  frames (`hello`, `welcome`, `synced`, `ack`, `blob`, `base_moved`, `dictionary`,
  `asset`, `presence`, `ping`/`pong`, `error`) and binary frames of an 8-byte sequence
  number plus Loro bytes. A device says which sequence it has; the room replays from
  there, or from the base when the device is behind a compaction, or tells it to reset
  when it claims more than the room has. Readers cannot push (Phase 17).
- App: `wordy-sync::cloud::room::RoomHandle` runs one thread per open project (connect
  with backoff, keepalive, replay, live fan-out, HTTP for big blobs, asset reconcile
  with the server winning on a clash). `SyncManager` pushes the ops since the last ack
  after every save (a full snapshot the first time), queues remote updates and applies
  them after a 1.2 s pause in typing; the workspace notes every caret as Loro cursors
  first and reloads each editor at the same text afterwards. `cloud.json` in the project
  folder remembers whether sync is on, the last sequence seen and the pushed version
  vector. Past 500 updates or 4 MiB of log the app uploads a snapshot that becomes the
  room's new base. The Account card has a "Sync this project" switch and shows live,
  offline, stopped and who else is in the room; the status bar shows "cloud live" and
  friends.
- Not done: the web never shows project content, there is no end-to-end encryption, and
  a device that was told to reset re-sends its whole copy rather than restoring anything
  from the server.
- *(manual)* Create the R2 bucket `wordy-projects` and apply migration `0002` remotely
  before deploying (the Durable Object migration `v1` runs with the deploy).

**Accept:** two copies of a project converge through the server, including a large edit,
a snapshot that compacts the log and a reconnect after it. *(Done:
`site/scripts/e2e-room.mjs` drives the room over raw websockets, and
`crates/wordy-sync/tests/cloud.rs::room_syncs_two_copies` runs two `RoomHandle`s on two
Loro docs against the local server: snapshot, live edits both ways, dictionary, an
attachment, a 700 KiB update over HTTP, compaction, replay from the base, a reset and a
stranger's refusal. The in-app behaviour was checked by building; the two-instance
walk-through is `docs/localhost-testing.md`.)*

### Phase 17 — Sharing (2026-10-04)

- Roles: the device that first syncs a project owns it; the owner can invite anyone by
  email as an *editor* or a *reader*. Ownership never moves, and the owner cannot leave
  their own project.
- Invitations (`site/migrations/0003_invitations.sql`): a random token, stored hashed,
  good for seven days; a new invitation to the same address replaces the old one. The
  email (`invitationEmail`) links to `/invite/<token>`, which does the right thing for
  whoever opens it: signed in as the invited address → "Join"; signed in as someone else
  → sign out first; a known address → passkey sign-in; a new address → one button sends
  the one-time account link, whose passkey step (`/account/passkey?next=…`) returns to
  the invitation. Used, withdrawn and expired links say so.
- Server (`site/src/server/sharing.ts` over `projects.ts`): invite, accept, change role,
  remove, leave and withdraw, each telling the project's room: an open connection of
  that account gets a `role` message and its new permissions at once, or is closed with
  4403 and refused on reconnect. The room already dropped a reader's pushes, words and
  asset announcements; the app now skips them too and shows "cloud read-only".
- API for the app: `GET /api/projects/:id/members` (members, plus pending invitations for
  the owner), `POST /api/projects/:id/invitations`, `DELETE …/invitations/:id`,
  `PATCH /api/projects/:id/members/:userId` (role) and `DELETE …/members/:userId`
  (owner removing, or oneself leaving). The web account page uses the same helpers: a
  card per project with the members, their roles, pending invitations, an invite form
  for owners and "Leave" for everyone else.
- Not done: the app has no sharing UI of its own (it shows role and read-only state; the
  website does the inviting), and there is no notification email on removal.
- *(manual)* Apply migration `0003` remotely before deploying.

**Accept:** an invited address with no account ends up with a passkey and the project on
its account; a reader sees edits live but its own stay local; an editor's go through;
removal cuts the connection. *(Done: `site/scripts/e2e-share.mjs` drives the whole thing
against the local server, with the invitee's sign-up in headless Chromium through
`e2e-auth.mjs --invite`.)*

### Phase 17b — Copies from the account, and testing on localhost (2026-10-04)

Done. Closes the gap left by Phase 17 (a machine could only sync a project it already
had a folder for) and makes the whole account/sync/sharing story runnable on one
computer against `wrangler dev`. `docs/localhost-testing.md` is the procedure.

- `Client::list_projects` (`GET /api/projects`) and `cloud::fetch_project`: join the room
  with an empty `LoroDoc`, import the replay up to `Synced`, wait for the asset
  reconcile (`RoomHandle::finish` joins the thread), write `project.loro`, then
  `cloud.json` with `enabled`, `last_seq = head` and the doc's version vector, and open
  the folder once to check it and write the mirror. An empty room (nothing pushed yet) is
  refused and the folder removed. Test: `fetch_project_copies_a_room` in
  `crates/wordy-sync/tests/cloud.rs`.
- Account card: "On the server" lists every project the account can reach ("yours" or
  "shared, editor/reader"), marks the ones with a local folder (ids read from
  `project.json` mirrors under the projects root, plus the open project) and offers "Get
  a copy" for the rest. The copy goes to a free folder name under the projects root and
  opens in a new window (`app::open_project_window`, split out of `open_main_window`).
- `WORDY_PROJECTS_DIR` overrides `~/Wordy` (`storage::projects_root`), so a second app
  instance (`WORDY_CONFIG_DIR` for its own account) keeps its projects apart.
- Site: `pnpm mail` prints the emails `wrangler dev` wrote (with their links), and
  `sendEmail` logs each message to the wrangler terminal when `SITE_URL` is localhost.
- Not done: a second window shares the process (one dictionary in the spell checker,
  Quit closes both); it is meant for testing and for getting a copy, not as a general
  multi-window mode.

### Phase 18 — LAN sync removed (2026-10-04)

Done. The Phase 6 peer sync (mDNS discovery, pairing code, TCP sessions pushing whole
snapshots, the "Peers" and "By address" controls) is gone; account sync does the job
without two machines having to be on one network. Deleted: `client.rs`, `server.rs`,
`discovery.rs`, `protocol.rs`, `session.rs` and `tests/converge.rs` in `wordy-sync`,
`docs/sync-test.md`, and the `mdns-sd` and `hmac` dependencies. The three helpers the
cloud code shared with it (`hex`, `sha256_hex`, `safe_relative`) live in
`wordy-sync/src/util.rs`. `SyncConfig` keeps only `device_name` (read from the old
`peer_name` key too), `cloud_server` and `cloud`; `SyncManager` owns only the account
and the room. The Home tab's "Sync" page is now "Account": the card alone, with the
machine's name next to the server field before linking.

### Phase 19 — Sheet moves to the right dock (2026-10-04)

Done. The entity sheet no longer sits above the description editor: `SheetPanel`
(`panels/sheet.rs`) is a second tab in the right dock next to Reference, and shows the
sheet of whichever entity is in the active editor (the hint otherwise). The workspace
points it at `active` from `layout_changed`, so every tab switch, open and close keeps
it in step; the sheet itself is built on the panel's next render, the first place a
window is at hand. Opening an entity brings the tab forward and opens the dock if it is
hidden (not during layout restore); pinning to Reference selects that tab instead. The
entity's type is a dropdown next to its title at the top of the sheet, replacing the row
of template buttons; the collapsible "Sheet" header is gone. `EditorPanel` lost its
`sheet` field and the `NamesChanged`/`FilterMentions` events, which the workspace now
takes straight from the panel.

### Phase 20 — No tabs at all (2026-10-04)

Done. Closing the last tab leaves the centre with no tab bar: the "Welcome" placeholder
panel is gone (`EditorPanel::placeholder` removed, `ensure_placeholder` with it) and the
centre dock layout is simply empty. `panels/empty.rs` holds `EmptyCenter`, a plain view
(not a panel) with one line of text and a search box over every node, matched with the
palette's `match_rank`, up/down/Enter or click opening the hit the way the palette does
(`Workspace::jump_to`). `WordyDockRenderer` wraps gpui-component's `DockSkin`, delegates
every hook, and in `center_frame` overlays the view while `EmptyCenter::shown` is set;
the workspace sets that flag from `layout_changed` (no editors and no Home) and focuses
the box whenever the last tab closes or there is nothing else to focus. The dock itself
never draws a close button on a lone tab (gpui-base refuses to empty a group through
its X), so `EditorPanel` and `HomePanel` carry a `sole_tab` flag the workspace sets from
`layout_changed`, and while it is set their `title_suffix` puts a close button at the end
of the tab bar that dispatches `CloseTab`.

### Phase 21 — Settings tab (2026-10-04)

Done. Home's row of seven sub-pages is gone. `panels/settings.rs` holds `SettingsPanel`,
a centre tab with a column of sections on the left (Account, Export, Appearance, Project,
Shortcuts) and the page on the right; Account, Export, the "Project file" box and the
shortcut list moved there from Home unchanged. It opens from the gear at the bottom of
the rail, `Ctrl-,` (`ShowSettings`) or "Go → Settings" in the palette, and the layout
file remembers whether it was open, on which section, and whether it or Home was in
front. Home (`panels/home.rs`) is now one scrolling page: today, goals, manuscript,
tasks, placeholders, charts, chapter goals and the last 30 sessions. The workspace's
tab identity is a three-way `Tab` enum (Home, Settings, Node) instead of `Option<TreeID>`.
Appearance is new: `prefs.rs` keeps `prefs.json` beside `sync.json` with the theme
(match the system, light, dark) and scrollbar mode, applied at startup and whenever the
window's appearance changes; the rail sun/moon button and `Ctrl-Shift-T` pin the result
there too.

### Phase 22 — Theme families (2026-10-04)

Done. Settings → Appearance gained a Theme row above Light or dark: Default (the stock
shadcn neutrals), Wordy (the website's cream page, ink and oxblood, both halves), Catppuccin
(Latte and Mocha) and High contrast (black on white / white on black, hard borders, radius 2,
no shadows). Each family is a gpui-component theme set in `crates/wordy-app/themes/*.json`,
embedded with `include_str!` and put in the `ThemeRegistry` by `prefs::register_themes` right
after `gpui_kit::init`. `Prefs.theme` holds the family; `Prefs::apply` swaps the global
theme's `light_theme` / `dark_theme` to the family's pair (resetting radius and shadow to the
stock values first, since a theme file only sets what it names) and then `Theme::change`
loads whichever half Appearance asks for. The JSON files are generated from palette tables so
all three sets name the same 105 colour keys; the Catppuccin colours are the project's own
(MIT). Frappé and Macchiato are a palette table away if wanted.

### Phase 23 — Text settings (2026-10-04)

Done. Settings → Text sets the editor's type per machine: font (a searchable Select of the
bundled Libertinus Serif plus every family the text system knows), size (12–36 px), line
spacing (1.2–2.2×), column width (440–1100 px) and paragraph style (first-line indent or a
half line of space), with a live preview and a reset. `Prefs.text` (`TextPrefs`) holds the
numbers and maps onto `EditorStyle`; `Prefs::apply` sets that style as a gpui global, and
`EditorStyle` is now `Global`, so every `ProseEditor` starts from the global and follows it
through `observe_global` (keeping the caret in view). Headings keep scaling off the body
size; exports keep their own paragraph setting. A `fonts/` folder beside `prefs.json` holds
the user's own `.ttf`/`.otf`/`.ttc` files: loaded at launch and on Reload, no install needed,
and they show up in the picker like any installed family.
