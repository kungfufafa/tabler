# Tabler

A native-feeling desktop database client for **PostgreSQL**, **MySQL / MariaDB** and **SQLite**,
built to be as tidy and comfortable as TablePlus. Tauri v2 + Rust backend, React + TypeScript
renderer.

## Features

- **TablePlus layout** — a toolbar with Discard / Preview / Commit, a status pill tinted with the
  connection colour (`LOCAL │ PostgreSQL 17.6 : name : database : schema.table`) and panel toggles;
  a rail of open connections, each its own workspace of tabs; a sidebar with Items | History; the
  table's bottom bar (`Data | Structure`, `+ Row`, Filters, Columns, paging); an editable row-details
  sidebar; a welcome screen listing connections with their environment tag (Local, Testing,
  Development, Staging, Production). ⌘S commits straight away (⇧⌘P previews first).
- **Connections** — saved profiles with colours and groups, SSL modes, read-only sessions, SSH
  tunnels (password, private key with passphrase, agent) with host-key verification. Passwords live
  in the macOS Keychain, never in plain files. Opening a profile that is already open switches to
  it instead of starting a second session. SQLite: **Choose…** an existing file or **New…** to
  create one; Test / Connect never create a missing file (a moved or mistyped path is an error),
  and foreign-key enforcement (`PRAGMA foreign_keys`) is a switch under Advanced.
- **Browse** — schema/database pickers, searchable table list with row estimates and **pins**
  ("Pin to Top" in a table's context menu; kept per connection and database), virtualized data
  grid (100k+ rows) with server-side sort, filters (`=`, `≠`, `<`…, `LIKE`, `IN`, `BETWEEN`,
  `IS NULL`, **Raw SQL** conditions, AND/OR), paging, column header menu, resize, hide and
  drag-to-reorder, an editable row-details sidebar (␣ toggles it; full blobs in its Cell view), foreign-key
  jump, quick filters from a cell, copy as TSV/CSV/JSON/INSERT/Markdown (⌥⌘C picker). Like
  TablePlus, a single click opens a table in a replaceable _preview_ tab (italic) and the sidebar
  keeps the last five tables under "Recent"; Data and Structure are two views of the same tab
  (⌃⌘[ / ⌃⌘]).
- **Edit** — inline, type-aware cell editing with staged changes (edited, new and deleted rows are
  highlighted), ⌘V paste of cells and rows (from Numbers, Excel or TSV), ⌥↩ multi-line text and a
  JSON editor with validation, `Preview SQL` before commit, single-transaction apply with per-row
  errors, undo / redo, Set Value ▸ NULL / EMPTY / DEFAULT (also ⌥-click), duplicate/delete rows,
  double-click below the last row for a new one, tables without primary keys handled safely.
- **SQL editor** — CodeMirror with schema-aware autocomplete per dialect, run the current
  statement (⌘↩) / all (⇧⌘↩) / the selection, multiple result sets, streaming results, cancel,
  beautify / uglify the current statement, a collapsible result pane, EXPLAIN (EXPLAIN
  ANALYZE of `INSERT` / `UPDATE` / `DELETE` runs inside a transaction that is rolled back, so the
  rows stay), formatting, error positions highlighted in the editor, query history, open/save
  `.sql` files.
- **Structure** — columns (with a primary-key toggle), indexes, foreign keys and triggers; staged
  DDL edits with preview (SQLite table rebuilds when `ALTER TABLE` cannot do it),
  create/rename/truncate/drop tables, **Open in SQL** (the table's `CREATE` statement in a new
  query tab).
- **Export** — streaming CSV, JSON / NDJSON and SQL `INSERT` export of tables or queries (⇧⌘E
  exports the result on screen; a result cut at the row limit can export all its rows by running a
  read-only statement again, never a `DELETE … RETURNING` or `CALL`) with progress and cancel;
  partial files are removed on cancel and on quit.
- **Safe mode** — TablePlus's five levels per connection (Silent, Alert Mode 1/2, Safe Mode 1/2)
  from the toolbar lock: an alert or the connection's password before queries, commits, imports,
  restores and kills reach the server (levels 2 skip SELECT / EXPLAIN / SHOW). A default level for
  new connections lives in Settings → Security.
- **Connections, more** — client SSL key / certificate, Bootstrap commands (SQL run after every
  login), password storage Keychain / Ask every time / No password, `~/.ssh/config` fallback for
  SSH, connection groups (drag a row onto a group), sort by name / tag / driver, keep-alive pings,
  import / export of connections to an encrypted `.tablerconnections` file.
- **Workspace** — workspaces and SQL tabs reopen at startup (Auto Save SQL Query Tab: none / first
  / all), Rename Tab, Show Workspaces (⇧⌘\), remappable shortcuts (Settings → Shortcuts).
- **Database objects** — sidebar Functions (functions, procedures, aggregates) and PostgreSQL
  Sequences that open their definition in a SQL tab; TablePlus item menus (Open Structure, Item
  Overview, Export…, Import ▸, New ▸, Clone…, Truncate ⌥⌫, Delete ⌫ / CASCADE ⇧⌫ / without FK
  checks ⌘⌫, Copy Script As ▸); multi-selection (⌘ / ⇧-click); Item Overview with editable comments;
  Open Anything also finds functions and saved queries.
- **Grid, more** — every TablePlus filter operator (Contains, Not contains, Has prefix / suffix and
  their case variants, ILIKE, NOT BETWEEN, JSON contains), an **Any column** filter, Apply All with
  AND (⌘↩) or OR (⇧⌘↩), filters remembered per table, Quick Look editor (⌘↩: JSON tree, hex,
  image preview, referenced row), bulk edit of many rows in the right sidebar, Set Value ▸ NOW() /
  Add a File…, Copy Rows As ▸ Plain text / JSON / HTML / Markdown / CSV / CSV with header / SQL Insert.
- **Structure, more** — TablePlus header (Name, Primary, Comment, Options, Triggers, Info), default
  presets, MySQL `extra`, column CHECKs and table CHECK constraints, triggers, MySQL engine / charset /
  collation / auto increment, editable view definitions.
- **SQL workflow** — a Queries pane (saved queries in folders, bound to keywords that expand in the
  editor; ⌥⌘S adds one), query parameters (`:name`, `${name}`, … with a values dialog), Return on
  Error, ⚙ editor settings, multiple carets, split pane (⇧⌘D), Save Selected Statements As (⇧⌘S),
  **editable query results** (one source table with its primary key; ⌘S commits), pinned results,
  append-all-results mode, a query time out (300 s by default).
- **Import / export, backup** — CSV export options (NULL → empty, line breaks, quote, decimal…), SQL
  export with DROP / structure / content / Gzip, Excel (.xlsx), export of many tables at once, the
  CSV import wizard (create table, matching strategies, STOP ON ERROR / REPLACE / UPDATE ON
  DUPLICATE) and SQL dump import (.sql / .sql.gz); Backup / Restore through the installed
  `pg_dump` / `pg_restore` / `psql` / `mysqldump` / `mysql` with version picker, option chips, Gzip
  and a file-name pattern.
- **Server admin** — create / rename (PostgreSQL) / drop databases and schemas from ⌘K, Running
  Queries (⌘.) with Kill Query / Kill Connection, User Management (roles, global and database
  privileges, MySQL resource limits; passwords never logged).
- **Wave 2** — IAM ("Command Line") passwords with token refresh, MySQL sockets, SSH
  keyboard-interactive / 2FA prompts, nested connection groups with icons, sidebar groups with filter
  rules and a per-table column list, staged sidebar truncate / drop (⌘S), Copy Script As ▸ Laravel
  Migration, an ER diagram (⌘P ▸ Generate Diagram…, SVG / PNG export), find and replace in results
  (⌘F), PostgreSQL RAISE NOTICE messages, comment-driven charts (`-- TablePlus BarChart x: a, y: b`),
  CSV default options, PostgreSQL trigger enable / disable and index INCLUDE columns, and a Navigate
  menu with Select Next / Previous Pane (⇧⌘] / ⇧⌘[).
- **Comfort** — TablePlus keys by default: Open Anything (⌘P), database switcher (⌘K), connection
  switcher (⇧⌘K), a console log of every statement the app runs (⇧⌘C), keyboard-first everything,
  a native macOS menu bar (File / Edit / View / Window) wired to the same actions, light/dark
  themes that follow macOS, tab management with unsaved-change guards (closing a tab, the window,
  or quitting from the menu bar, ⌘Q or the Dock asks first). The sidebar, status pill and accent always follow the visible
  tab's connection. A dropped connection shows the reconnect banner at the first failing query,
  grid commit or DDL apply.

Values are lossless across the UI: 64-bit integers beyond 2⁵³ and decimals are kept as text,
timestamps keep the server's textual form, binary data is shown as a size and previewed on demand.

## Requirements

- macOS 12 or newer (Apple Silicon or Intel)
- Rust stable (`rustup`), Node 22, pnpm 11
- For development databases: local PostgreSQL / MySQL (e.g. Homebrew) or Docker

## Quick start

```bash
pnpm install
cp .env.example .env          # adjust local database credentials
pnpm dev:db:seed              # creates tabler_dev on the local PostgreSQL and MySQL
pnpm dev:sqlite:build         # builds dev/fixtures/sample.sqlite
pnpm tauri dev                # the first Rust build takes a few minutes
```

Renderer-only loop with an in-memory mock backend (no Rust build): `pnpm dev:mock`, then open
http://localhost:1420.

## Development databases

`pnpm dev:db:seed` (re)creates `tabler_dev` from `dev/seed/` with the same shape on every engine:
`users`, `posts` (FK + trigger), `tags` / `post_tags` (composite key), `audit_log` (no primary
key), `events_big` (100 000 rows), `types_zoo` (every type per engine), quoted identifiers
(`"Order Items"`, `CamelCase`), views, a second schema (`inventory` on PostgreSQL, `tabler_inv` on
MySQL) and edge-case rows (emoji, CJK, RTL, extreme dates, 2⁶³−1).

| Engine     | Default URL (`.env`)                                |
| ---------- | --------------------------------------------------- |
| PostgreSQL | `postgres://apriansyahrs@127.0.0.1:5432/tabler_dev` |
| MySQL      | `mysql://root@127.0.0.1:3306/tabler_dev`            |
| SQLite     | `dev/fixtures/sample.sqlite`                        |

A Docker alternative (PostgreSQL 17, MySQL 9.7, MariaDB 11.8 and an OpenSSH server for tunnel
tests) is in `dev/docker-compose.yml`; see `dev/README.md`.

## Scripts

| Script                                      | What it does                                                                            |
| ------------------------------------------- | --------------------------------------------------------------------------------------- |
| `pnpm tauri dev` / `pnpm tauri build`       | run / package the desktop app                                                           |
| `pnpm dev:mock`                             | renderer only, in-memory IPC mock                                                       |
| `pnpm check`                                | typecheck, lint, format check, vitest, clippy, cargo test                               |
| `pnpm test`                                 | renderer unit and component tests (vitest)                                              |
| `pnpm test:rs`                              | Rust unit tests; integration tests self-skip without `.env`                             |
| `pnpm test:rs:int`                          | Rust integration tests against the databases in `.env` (incl. an in-process SSH server) |
| `pnpm dev:db:seed`, `pnpm dev:sqlite:build` | seed the local databases, build the SQLite fixture                                      |

## Keyboard shortcuts

The defaults follow TablePlus, so its muscle memory carries over (older Tabler keys such as ⌘B,
⌘L, ⌘N and ⌥⇧F still work as alternatives).

| Global    |                           | Data grid   |                             | SQL editor  |                         |
| --------- | ------------------------- | ----------- | --------------------------- | ----------- | ----------------------- |
| ⌘P        | open anything             | ↩ / F2      | edit cell                   | ⌘↩          | run current / selection |
| ⌘K        | switch database           | ⌥↩          | multi-line / JSON edit      | ⇧⌘↩         | run all                 |
| ⇧⌘K       | switch connection         | ⌘S          | commit                      | ⌘.          | cancel                  |
| ⌘T / ⌘E   | new query tab             | ⇧⌘P         | preview SQL                 | ⌘E          | explain                 |
| ⌘W        | close tab                 | ⇧⌘⌫         | discard changes             | ⌘I / ⇧⌘I    | beautify / uglify       |
| ⇧⌘T       | reopen closed tab         | ⌘Z / ⇧⌘Z    | undo / redo staged change   | ⌃\`         | toggle result pane      |
| ⌘[ / ⌘]   | previous / next tab       | ⌘F          | filter bar                  | ⌘H          | history                 |
| ⌘1…9      | go to tab                 | ⌥⌘F         | show / hide columns         | ⌘S / ⌘O     | save / open `.sql`      |
| ⌃⌘[ / ⌃⌘] | data / structure view     | ⌘I          | new row                     | ⌘/          | toggle comment          |
| ⌘0        | toggle sidebar            | ⌘D          | duplicate row               | ⇧⌘E         | export the result       |
| ⇧⌘I       | toggle row detail         | ⌫ / ⌘⌫      | delete rows                 | ⌥⌘S         | add to Queries          |
| ⇧⌘C       | console log               | ⇧⌘N / ⇧⌘D   | set NULL / DEFAULT          | ⇧⌘S         | save selected as…       |
| ⇧⌘F       | search tables             | ␣ (Space)   | row detail                  | ⇧⌘D         | split pane              |
| ⌘R / ⌥⌘R  | refresh                   | ⌘J          | open referenced row         | ⌘D / ⌃⌘G    | next / all occurrences  |
| ⇧⌘R       | reconnect                 | ⌘C / ⌥⌘C    | copy rows / copy as…        | ⌃⇧↑ / ⌃⇧↓   | add caret above / below |
| ⇧⌘E       | export                    | ⌘V          | paste cells / rows          | ⌘+ / ⌘−     | font size               |
| ⌘N        | new connection            | ⌘↩          | Quick Look editor           |             |                         |
| ⇧⌘O       | connection manager        | ⌥↩          | Set Value menu              | **Filters** |                         |
| ⌘,        | settings                  | ⌘← / ⌘→     | previous / next page        | ⌘↩ / ⇧⌘↩    | apply all with AND / OR |
| ⌘.        | running queries           |             |                             | ⌘I / ⇧⌘I    | duplicate / remove row  |
| ⇧⌘\       | show workspaces           | **Sidebar** |                             | ⌘↑ / ⌘↓     | previous / next filter  |
| ⌃⌘← / ⌃⌘→ | previous / next workspace | ⌥⌫          | truncate                    | ⌘B          | toggle the filter       |
| ⌃- / ⌃⇧-  | back / forward            | ⇧⌫ / ⌘⌫     | drop CASCADE / no FK checks |             |                         |
| ⌘Q        | quit (asks if unsaved)    | ⌘A          | select all items            |             |                         |

The full, searchable list is in Settings → Shortcuts. The native menu bar offers the same
commands (File: New Query Tab, Open SQL File…, Close Tab, Close All Tabs, Reopen Closed Tab,
Backup…, Restore…, Export…, Import ▸, Connection Manager; View: Open Anything, Switch Database,
Switch Connection, Data, Structure, Toggle Sidebar, Toggle Row Detail, Toggle Console Log, Refresh,
Full Screen; Connection: Process List…, User Management…, Reconnect, Disconnect; Window: Show
Workspaces, Select Next / Previous Workspace); its shortcuts only act
when the focused part of the page does not handle the key itself, and Edit keeps the standard
Undo / Cut / Copy / Paste / Select All for text fields.

## Architecture

```
src-tauri/src/
  commands/   thin Tauri commands (profiles, connections, schema, browse, mutate, query,
              history, ddl, export, files, settings)
  drivers/    DbSession trait + postgres (tokio-postgres, deadpool), mysql (mysql_async),
              sqlite (rusqlite actor threads); each keeps a dedicated editor session
  sqlgen/     every generated SQL string: quoting, SELECT/COUNT, UPDATE/INSERT/DELETE,
              DDL plans, statement splitter, EXPLAIN
  tunnel/     russh SSH tunnels and host-key trust
  export/     streaming CSV/JSON/SQL writers
  appdb.rs    app SQLite store (profiles, settings, history, trusted hosts)
  secrets.rs  Keychain (keyring) with a dev-only file backend
src/
  features/   connections, sidebar, tabs, datagrid, table-tab, staging, editor, structure,
              export, palette, settings, inspector, console
  lib/ipc/    typed facade over every command + generated bindings + browser mock
  stores/     zustand stores (tabs, per-tab state, connections, prefs, ui)
```

Shared types are defined once in Rust and exported to TypeScript with `ts-rs` when `cargo test`
runs (`src/lib/ipc/bindings/`). Large data never crosses IPC in one piece: query results stream in
≤500-row chunks over Tauri channels, browse pages are bounded, exports are written by Rust.

## Packaging

```bash
pnpm tauri build                 # src-tauri/target/release/bundle/{macos/Tabler.app,dmg/*.dmg}
pnpm tauri build --bundles app   # skip the DMG
```

Builds are ad-hoc signed (`bundle.macOS.signingIdentity: "-"` in `src-tauri/tauri.conf.json`):
the whole bundle is sealed under the identifier `com.mekayastudio.tabler`, so
`codesign --verify --deep --strict --verbose=2 Tabler.app` passes and `codesign -dv --verbose=2`
shows `Identifier=com.mekayastudio.tabler` and `Sealed Resources` (a merely linker-signed bundle
fails that check and is reported as damaged on other Macs). Ad-hoc signing is not a Developer ID:
a downloaded build is still quarantined by Gatekeeper; open it via right-click → Open, or remove the
quarantine flag with `xattr -cr /Applications/Tabler.app`. Notarized releases need an Apple
Developer ID.

## Troubleshooting

- **Repeated Keychain prompts during development** — every rebuild changes the binary's code
  signature. Set `TABLER_SECRETS_BACKEND=file` in `.env` (stores passwords in the app database; dev
  only), or sign debug builds with a stable self-signed identity.
- **MySQL 9 authentication** — `caching_sha2_password` is supported over TLS and plain TCP (RSA key
  exchange); no server changes needed.
- **Port 1420 in use** — `pnpm tauri dev` and `pnpm dev:mock` both use it; stop one of them.
- **First build is slow** — the Rust dependency build takes a few minutes once; later builds are
  incremental. Renderer work can use `pnpm dev:mock` without rebuilding Rust.

See `docs/qa-macos.md` for the manual release checklist and `CLAUDE.md` for contributor
conventions.
