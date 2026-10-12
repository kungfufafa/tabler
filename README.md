# Tabler

A native-feeling desktop database client for **PostgreSQL**, **MySQL / MariaDB** and **SQLite**,
built to be as tidy and comfortable as TablePlus. Tauri v2 + Rust backend, React + TypeScript
renderer.

## Features

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
  drag-to-reorder, inspector (cell and row view, full blobs), foreign-key jump, copy as
  TSV/CSV/JSON/INSERT/Markdown (⇧⌘C picker).
- **Edit** — inline, type-aware cell editing with staged changes (edited, new and deleted rows are
  highlighted), ⌘V paste of cells and rows (from Numbers, Excel or TSV), ⌥↩ multi-line text and a
  JSON editor with validation, `Preview SQL` before commit, single-transaction apply with per-row
  errors, undo, NULL/DEFAULT, duplicate/delete rows, tables without primary keys handled safely.
- **SQL editor** — CodeMirror with schema-aware autocomplete per dialect, run all / selection /
  statement under the cursor, multiple result sets, streaming results, cancel, EXPLAIN (EXPLAIN
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
- **Comfort** — command palette (⌘K), keyboard-first everything, a native macOS menu bar (File /
  Edit / View / Window) wired to the same actions, light/dark themes that follow macOS, tab
  management with unsaved-change guards (closing a tab, the window, or quitting from the menu
  bar, ⌘Q or the Dock asks first). The sidebar, status bar and accent always follow the visible
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

| Global   |                        | Data grid |                          | SQL editor |                         |
| -------- | ---------------------- | --------- | ------------------------ | ---------- | ----------------------- |
| ⌘K / ⌘P  | command palette        | ↩ / F2    | edit cell                | ⌘↩         | run all / selection     |
| ⌘T       | new query tab          | ⌥↩        | multi-line / JSON edit   | ⇧⌘↩        | run statement at cursor |
| ⌘W       | close tab              | ⌘S        | commit (Preview SQL)     | ⌘.         | cancel                  |
| ⇧⌘T      | reopen closed tab      | ⇧⌘Z       | discard changes          | ⌘E         | explain                 |
| ⌘1…9     | go to tab              | ⌘Z        | undo staged change       | ⌥⇧F        | format                  |
| ⌃⇥ / ⌃⇧⇥ | next / previous tab    | ⌘F        | filter bar               | ⌘H         | history                 |
| ⌘B       | toggle sidebar         | ⌘N        | new row                  | ⌘S / ⌘O    | save / open `.sql`      |
| ⇧⌘I      | toggle inspector       | ⌘D        | duplicate row            | ⌘/         | toggle comment          |
| ⌘L       | search tables          | ⌘⌫        | delete rows              | ⇧⌘E        | export the result       |
| ⌘R       | refresh                | ⇧⌘N / ⇧⌘D | set NULL / DEFAULT       |            |                         |
| ⇧⌘E      | export                 | ⇧⌘F       | open referenced row      |            |                         |
| ⇧⌘O      | connection manager     | ⌘C        | copy (TSV)               |            |                         |
| ⌘,       | settings               | ⇧⌘C       | copy as… (format picker) |            |                         |
| ⌘Q       | quit (asks if unsaved) | ⌘V        | paste cells / rows       |            |                         |

The full, searchable list is in Settings → Shortcuts. The native menu bar offers the same
commands (File: New Query Tab, Open SQL File…, Close Tab, Reopen Closed Tab, Export…, Connection
Manager; View: Command Palette, Toggle Sidebar, Toggle Inspector, Refresh, Full Screen); its
shortcuts only act when the focused part of the page does not handle the key itself, and Edit
keeps the standard Undo / Cut / Copy / Paste / Select All for text fields.

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
              export, palette, settings, inspector, statusbar
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
the whole bundle is sealed under the identifier `id.co.oceanspace.tabler`, so
`codesign --verify --deep --strict --verbose=2 Tabler.app` passes and `codesign -dv --verbose=2`
shows `Identifier=id.co.oceanspace.tabler` and `Sealed Resources` (a merely linker-signed bundle
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
