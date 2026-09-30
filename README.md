<p align="center">
  <img src="design/icon.png" width="128" alt="IdeDB">
</p>

<h1 align="center">IdeDB</h1>

<p align="center">
  <strong>A fast, native SQL IDE for macOS, built for the keyboard.</strong><br>
  PostgreSQL · MySQL · SQLite
</p>

<p align="center">
  <a href="https://github.com/naitsric/IdeDB/actions/workflows/ci.yml"><img src="https://github.com/naitsric/IdeDB/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
</p>

<!-- screenshot: docs/screenshot.png -->

## Why IdeDB

IdeDB brings DataGrip-style workflows to a small, native Mac app. It is a
5 MB download, every action is a keystroke away, and it is careful
with real data: it checks your SQL against the server without running it,
never commits a transaction you opened, and asks before throwing away
unsaved work.

## Features

### Connect
- **PostgreSQL, MySQL and SQLite**, with the same experience on all three.
- Paste a `postgres://` or `mysql://` URL and the connection form fills itself.
- **Passwords live in the macOS Keychain**, never in IdeDB's own files; or
  leave them unsaved and IdeDB asks once per session.
- TLS modes Disable, Prefer, Require and Verify full, with libpq semantics.
- Color-code data sources so production never looks like staging.

### Explore
- **Database Explorer** with schemas, tables, views and columns, primary and
  foreign keys marked, loaded on demand.
- **Speed search**: start typing in the tree to filter it.
- **Go to Table** (⌘O) finds any table in any connected database; schemas load
  in the background as soon as you connect.

### Write SQL
- A real code editor with dialect-aware highlighting, multiple cursors, search
  and IntelliJ-style editing keys.
- **Run the statement under the caret** with ⌘⏎, or a selection of several:
  each gets its own result tab, and the run stops at the first error.
- **Completion that knows your schema**: schemas, tables and columns, through
  aliases, ranked above keywords where a table belongs.
- **JOIN completion from foreign keys**: type `join` and get
  `customers c on c.id = o.customer_id`.
- **Live diagnostics**: the server checks your statements as you type without
  executing them, so errors, unknown tables and columns are underlined before
  you run anything.
- **Go to Declaration** (⌘B) jumps from a name in the editor to the table or
  column in the explorer. **Reformat** (⌥⌘L) tidies a statement or selection.
- A schema selector per console, and a searchable **query history** with the
  duration, row count or error of every run.

### Work with results
- A canvas-rendered grid, virtualized in both directions, that scrolls
  smoothly through large results.
- **Results load a page at a time** and fetch more as you scroll, so a huge
  table never floods memory.
- Value viewer for JSON and long text, and live count, sum, average, min and
  max of the selected cells.
- Copy as TSV, CSV, JSON or SQL INSERT; export results to CSV, JSON or SQL.
- Pin result tabs to keep them while you run other queries.

### Edit data
- Open any table (F4) and filter it with WHERE and ORDER BY fields.
- Edit cells, set NULL, add, duplicate and delete rows. Changes are marked until
  you **Submit (⌘⏎), in a single transaction**: if one change fails, none are
  applied and the failing row is highlighted.
- **Go to Referenced Row** (⌘B) follows a foreign key to the row it points to.

### Transactions you control
- **Auto or Manual** transaction mode per console, with Commit (⌥⌘⏎) and
  Rollback (⌥⇧⌘Z) and an indicator of how long a transaction has been open.
- IdeDB never commits or rolls back a transaction you opened, and it asks
  before closing a tab, disconnecting or quitting with one still open.
- If a lost connection or an implicit commit ends a transaction, the console
  tells you.

### Feels like a Mac app
- Native menu bar, translucent sidebar, light and dark themes following the
  system, and a window that reopens where you left it.
- A 9 MB app on the system's WebKit and a Rust core.

## Keyboard

| Action                          | Shortcut |
| ------------------------------- | -------- |
| Search Everywhere               | ⇧⇧       |
| Find Action                     | ⇧⌘A      |
| Go to Table                     | ⌘O       |
| New Data Source                 | ⌘N       |
| New Query Console               | ⇧⌘L      |
| Execute statement / selection   | ⌘⏎       |
| Cancel running statement        | ⌘F2      |
| Query History                   | ⌥⌘E      |
| Reformat Code                   | ⌥⌘L      |
| Go to Declaration / Referenced Row | ⌘B    |
| Open Table Data                 | F4       |
| Submit data changes (in the grid) | ⌘⏎     |
| Commit / Rollback               | ⌥⌘⏎ / ⌥⇧⌘Z |
| Database Explorer               | ⌘1       |

Every action is also in Find Action (⇧⌘A) and in the menu bar.

## Install

Download the latest `.dmg` from
[Releases](https://github.com/naitsric/IdeDB/releases/latest) and drag IdeDB
to Applications. Requires macOS 13 or later on Apple Silicon.

Builds are not notarized yet, so macOS blocks the first launch. Run this once
after installing:

```sh
xattr -dr com.apple.quarantine /Applications/IdeDB.app
```

## Roadmap

- SSH tunnels
- Transposed grid view
- Editable keymap
- Prompts for `:named` query parameters
- Notarized builds and automatic updates

## Contributing

IdeDB is built with Tauri 2, React and Rust. See
[CONTRIBUTING.md](CONTRIBUTING.md) to run it locally, run the tests and learn
how the code is organized.

## License

To be decided.

---

<sub>DataGrip is a trademark of JetBrains s.r.o. IdeDB is an independent project and is not affiliated with or endorsed by JetBrains.</sub>
