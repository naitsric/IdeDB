# IdeDB

A SQL client for macOS with DataGrip-grade UX. Built with Tauri 2, React and a
Rust core. Supports PostgreSQL, MySQL and SQLite.

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

Both databases start with a sample `shop` schema (`dev/seed/`). In the app,
create data sources by pasting these URLs into the URL field:

```
postgres://idedb:idedb@localhost:54329/idedb
mysql://idedb:idedb@localhost:33069/shop
```

## Test

```sh
pnpm test         # UI unit tests (statement splitting, editor, consoles)
pnpm test:rust    # unit tests + driver conformance tests against the Docker databases
pnpm typecheck
```

## CI

`.github/workflows/ci.yml` runs on every pull request and every push to
`main`:

- **Frontend:** `tsc --noEmit`, `pnpm test`, `pnpm build`.
- **Rust:** `cargo clippy --workspace --all-targets -- -D warnings` and
  `pnpm test:rust` against Postgres 17 and MySQL 8.4 service containers
  (same credentials and ports as `docker-compose.yml`). The job fails if the
  database tests skip themselves.

CI runs on Linux only: while the repository is private, macOS runner minutes
count 10x against the Actions quota. The Keychain integration is macOS-only
and compiled out there.

## Releasing

Releases are cut by hand from `main` with the **Release** workflow, from
Actions → Release → Run workflow, or:

```sh
gh workflow run release.yml -f bump=minor
gh workflow run release.yml -f bump=patch -f dry_run=true   # build only
```

| Input     | Default   | Meaning                                                                 |
| --------- | --------- | ----------------------------------------------------------------------- |
| `bump`    | `patch`   | `patch`, `minor` or `major`                                             |
| `target`  | `aarch64` | `aarch64` (Apple Silicon) or `universal` (adds Intel, builds longer)    |
| `draft`   | `true`    | publish the GitHub Release as a draft                                   |
| `dry_run` | `false`   | only build and attach the bundle to the workflow run: no commit, tag or release |

The workflow bumps the version with `scripts/release-version.mjs`
(`package.json` is the source of truth; `tauri.conf.json` reads it and the
Cargo workspace version follows), commits `Release vX.Y.Z` to `main`, tags
it, builds `IdeDB.app` and a `.dmg` on macOS, and publishes both in a GitHub
Release with generated notes. If `main` is protected against direct pushes,
allow `github-actions[bot]` to push or the prepare job will fail.

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

## macOS integration

- **Menu bar:** built from the command registry (`src/menu/`): every command
  appears under a menu picked by its category, unknown categories under Tools.
  A menu item shows its shortcut only when the key means the same thing
  everywhere; context keys (⌘⏎, ⌘B, ⌘N in the grid…) stay with the JS keymap,
  because AppKit gives menu shortcuts priority over the web view.
- **Translucent sidebar:** the window is transparent over the sidebar material,
  and only the explorer and title bar let it through. This uses Tauri's
  `macos-private-api`, which the Mac App Store does not accept; IdeDB ships
  through GitHub Releases. "Toggle Translucent Sidebar" makes everything opaque.
- **App icon:** `design/icon.svg` is the source. After editing it, run
  `pnpm icon` to render it and regenerate `src-tauri/icons`.

## Layout

```
.github/workflows/   CI (Linux) and the manual Release workflow (macOS)
scripts/             release-version.mjs (version bump), render-icon.mjs (app icon)
design/              app icon source (icon.svg) and its 1024px render
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
  db/                typed IPC with the Rust core, data source and console state
src-tauri/           Tauri app: commands exposed to the UI, driver dispatch
crates/
  idedb-core/           engine-agnostic types, the Session trait, conformance tests
  idedb-driver-pg/      PostgreSQL (tokio-postgres)
  idedb-driver-mysql/   MySQL (mysql_async)
  idedb-driver-sqlite/  SQLite (rusqlite, bundled)
  idedb-store/          data sources and query history (local SQLite), passwords (Keychain)
```

Every driver implements `idedb_core::Session` and runs the shared conformance
checks in `idedb_core::testing` (paging, cancellation, errors, data editor
changes, checking without running), so all engines behave the same behind the UI.

Live diagnostics come from the engine itself: shortly after typing stops, the
statements on screen are prepared (never executed) on the data source's
explorer session, so the editor flags exactly what the server would reject,
names and types included, without a SQL parser of our own.

Every user action is a registered command (`src/commands/registry.ts`) with
an id, a title and optionally a keybinding. Registering it is enough for it to
show up in Search Everywhere (⇧⇧ / ⌘⇧A) and the keymap.
