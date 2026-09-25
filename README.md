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

## Layout

```
src/                 React UI
  commands/          command registry, keymap, app commands, Search Everywhere
  actions.ts         user-level actions shared by commands, menus and search
  workbench/         window chrome, dockable panels (explorer, consoles, result tabs)
  editor/            SQL editor (CodeMirror 6), statement splitting, completion
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
changes), so all engines behave the same behind the UI.

Every user action is a registered command (`src/commands/registry.ts`) with
an id, a title and optionally a keybinding. Registering it is enough for it to
show up in Search Everywhere (⇧⇧ / ⌘⇧A) and the keymap.
