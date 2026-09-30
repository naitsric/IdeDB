# Contributing to IdeDB

IdeDB is a Tauri 2 app: a React + TypeScript UI over a Rust core, one driver
crate per database engine. This guide covers running it, testing it, how the
code is organized, and how releases are cut.

## Requirements

- macOS 13+, Xcode Command Line Tools
- Rust (`rustup`), Node 22+ and pnpm
- Docker, for the local databases used in development and tests

## Develop

```sh
pnpm install
pnpm db:up        # Postgres 17 on :54329 and MySQL 8.4 on :33069, user/password idedb/idedb
pnpm app          # runs the app with hot reload
```

Both databases start with a sample `shop` schema (`dev/seed/`): customers,
products, orders and 150,000 order items, with foreign keys, a composite
primary key and a view. In the app, press ⌘N and paste one of these into the
URL field:

```
postgres://idedb:idedb@localhost:54329/idedb
mysql://idedb:idedb@localhost:33069/shop
```

For SQLite, pick any file path; the database is created if it does not exist.
`pnpm db:down` stops the containers.

## Test

```sh
pnpm typecheck    # tsc --noEmit
pnpm test         # UI unit tests (vitest): editor, completion, consoles, grid, transactions, menu
pnpm test:rust    # Rust unit tests + driver conformance tests against the Docker databases
```

The driver integration tests read `IDEDB_PG_URL` and `IDEDB_MYSQL_URL` (set by
`pnpm test:rust`) and skip themselves when those are missing. SQLite tests
always run.

## Project layout

```
.github/workflows/   CI (Linux) and the manual Release workflow (macOS)
scripts/             release-version.mjs (version bump), render-icon.mjs (app icon)
design/              app icon source (icon.svg) and its 1024px render
dev/seed/            sample data loaded into the Docker databases and CI
src/                 React UI
  commands/          command registry, keymap, app commands, Search Everywhere
  menu/              native menu bar model, built from the command registry
  actions.ts         user-level actions shared by commands, menus and search
  workbench/         window chrome, dockable panels (explorer, consoles, result tabs)
  editor/            SQL editor (CodeMirror 6): statement splitting, completion (with
                     FK joins), live diagnostics, go to declaration, formatting
  explorer/          Database Explorer tree
  dialogs/           data source and password dialogs
  grid/              result grid (Glide Data Grid behind our own props) and the data
                     editor: filters, pending changes, copy/export, value viewer
  db/                typed IPC with the Rust core; data source, console, transaction
                     and result-paging state
src-tauri/           Tauri app: commands exposed to the UI, menu
crates/
  idedb-core/           engine-agnostic types, the Session trait, conformance tests
  idedb-driver-pg/      PostgreSQL (tokio-postgres)
  idedb-driver-mysql/   MySQL (mysql_async)
  idedb-driver-sqlite/  SQLite (rusqlite, bundled)
  idedb-drivers/        dispatch over the drivers (AnySession), opening a saved data source
  idedb-mcp/            MCP server core: tools, grants, approvals, pooled read-only sessions, audit
  idedb-sql/            SQL statement classifier for the MCP server: read, write or forbidden
  idedb-store/          data sources and query history (local SQLite), passwords (Keychain)
```

## Architecture notes

**Commands.** Every user action is a registered command
(`src/commands/registry.ts`) with an id, a title, a category and optionally a
keybinding and a focus context. Registering it is enough for it to appear in
Search Everywhere (⇧⇧ / ⌘⇧A), the keymap and the native menu bar. Several
commands may share a key when they declare different contexts (for example
⌘⏎ runs a statement in the editor and submits edits in the grid).

**Drivers.** Every driver implements `idedb_core::Session` and runs the shared
conformance checks in `idedb_core::testing`: paging, cancellation, errors,
data editor changes, checking without running, respecting transactions the
user opened, and refusing writes in read-only sessions (`connect_with` and
`ConnectOptions`). All engines behave the same behind the UI; a new engine
starts by passing those checks.

**Results.** A statement reads its first page (500 rows by default; *Result
Page Size…* changes it) and the rest stays open on the console's session.
Scrolling near the end fetches the next page, *Fetch All Rows* the rest, and
*Close Result Set* releases it. A session holds one open result; anything else
it runs (another statement, a data editor submit, a schema switch, a commit)
closes it first, and so do two minutes without reading, because an open result
holds server resources (a snapshot and table locks in Postgres, a statement
paused mid-send in MySQL, a shared lock on a SQLite file). The driver's own
read transaction never shows as the user's.

**Transactions.** Drivers never commit or roll back a transaction the user
opened: reads and data editor submits inside one use savepoints and leave it
open. Each console has a Tx Auto/Manual mode; in Manual the first statement
opens a transaction (`BEGIN`, or `START TRANSACTION` on MySQL). Closing a
console, disconnecting or deleting its data source, and quitting or closing
the window with a transaction open ask first. The Dock's Quit and system
logout end the app without asking (macOS gives apps no way to stop them); the
server then rolls the transaction back.

**Live diagnostics** come from the engine itself: shortly after typing stops,
the statements on screen are prepared (never executed) on the data source's
explorer session, so the editor flags exactly what the server would reject,
names and types included, without a SQL parser of our own.

**Menu bar.** Built from the command registry (`src/menu/`): every command
appears under a menu picked by its category, unknown categories under Tools.
A menu item shows its shortcut only when the key means the same thing
everywhere; context keys (⌘⏎, ⌘B, ⌘N in the grid…) stay with the JS keymap,
because AppKit gives menu shortcuts priority over the web view.

**Translucent sidebar.** The window is transparent over the sidebar material,
and only the explorer and title bar let it through. This uses Tauri's
`macos-private-api`, which the Mac App Store does not accept; IdeDB ships
through GitHub Releases. "Toggle Translucent Sidebar" makes everything opaque.

**App icon.** `design/icon.svg` is the source. After editing it, run
`pnpm icon` to render it and regenerate `src-tauri/icons`.

## CI

`.github/workflows/ci.yml` runs on every pull request and every push to
`main`:

- **Frontend:** `tsc --noEmit`, `pnpm test`, `pnpm build`.
- **Rust:** `cargo clippy --workspace --all-targets --locked -- -D warnings`
  and `pnpm test:rust` against Postgres 17 and MySQL 8.4 service containers
  seeded from `dev/seed/` (same credentials and ports as
  `docker-compose.yml`). The job fails if the database tests skip themselves.

CI runs on Linux only: while the repository is private, macOS runner minutes
count 10x against the Actions quota. The Keychain integration is macOS-only
and compiled out there, so code behind `cfg(target_os = "macos")` must also be
warning-free when it is compiled out.

## Releasing

Releases are cut by hand from `main` with the **Release** workflow, from
Actions → Release → Run workflow, or:

```sh
gh workflow run release.yml -f bump=minor
gh workflow run release.yml -f bump=patch -f dry_run=true   # build only
```

| Input     | Default   | Meaning                                                                         |
| --------- | --------- | ------------------------------------------------------------------------------- |
| `bump`    | `patch`   | `patch`, `minor` or `major`                                                     |
| `target`  | `aarch64` | `aarch64` (Apple Silicon) or `universal` (adds Intel, builds longer)            |
| `draft`   | `true`    | publish the GitHub Release as a draft                                           |
| `dry_run` | `false`   | only build and attach the bundle to the workflow run: no commit, tag or release |

The workflow bumps the version with `scripts/release-version.mjs`
(`package.json` is the source of truth; `tauri.conf.json` reads it and the
Cargo workspace version follows), commits `Release vX.Y.Z` to `main`, tags
it, builds `IdeDB.app` and a `.dmg` on macOS, and publishes both in a GitHub
Release with generated notes. If `main` is protected against direct pushes,
allow `github-actions[bot]` to push or the version job will fail.

**Code signing.** Until the Apple secrets exist, builds are ad-hoc signed and
not notarized, so macOS blocks them after download; testers run
`xattr -dr com.apple.quarantine /Applications/IdeDB.app` once. Signing turns
on by itself when these repository secrets are set:

- `APPLE_CERTIFICATE` (base64 of the Developer ID Application `.p12`),
  `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`: signing.
- `APPLE_ID`, `APPLE_PASSWORD` (app-specific password), `APPLE_TEAM_ID`:
  notarization.

A release build uses the macOS runner for roughly 10–20 minutes (`universal`
takes longer), which counts 10x while the repository is private. Auto-updates
are not wired yet: release assets of a private repository cannot be
downloaded without authentication.

## Pull requests

- Keep each PR focused; describe what changed, why, and how it was tested.
- `pnpm typecheck`, `pnpm test`, `pnpm test:rust` and clippy must pass; CI
  checks the same.
- New driver behavior gets a conformance check in `idedb_core::testing`, run by
  every driver; UI logic gets vitest tests (extract pure functions where
  needed).
