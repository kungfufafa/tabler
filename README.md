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
  drag-to-reorder, an editable row-details sidebar (␣ toggles it; every text, JSON and XML field is a
  compact CodeMirror editor with multiple carets, search and syntax colours; full blobs in its Cell view), foreign-key
  jump (a menu when a column has several keys; composite keys filter by every column, binary keys by
  their hex), quick filters from a cell, copy as TSV/CSV/JSON/INSERT/Markdown (⌥⌘C picker). Like
  TablePlus, a single click opens a table in a replaceable _preview_ tab (italic) and the sidebar
  keeps the last five tables under "Recent"; Data and Structure are two views of the same tab
  (⌃⌘[ / ⌃⌘]).
- **Edit** — inline, type-aware cell editing with staged changes (edited, new and deleted rows are
  highlighted), ⌘V paste of cells and rows (from Numbers, Excel, TSV or CSV), ⌥↩ multi-line text and a
  JSON editor with validation, `Preview SQL` before commit, single-transaction apply with per-row
  errors, undo / redo, Set Value ▸ NULL / EMPTY / DEFAULT (also ⌥-click), duplicate/delete rows,
  double-click below the last row for a new one, tables without primary keys handled safely.
- **SQL editor** — CodeMirror with schema-aware autocomplete per dialect, run the current
  statement (⌘↩) / all (⇧⌘↩) / the selection, multiple result sets, streaming results, cancel,
  beautify / uglify the current statement, a collapsible result pane, EXPLAIN (EXPLAIN
  ANALYZE of `INSERT` / `UPDATE` / `DELETE` runs inside a transaction that is rolled back, so the
  rows stay), formatting, error positions highlighted in the editor, query history, open/save
  `.sql` files. A new SQL tab starts at **No limit** (the row-limit select also offers 100, 300, 500,
  1,000 and 10,000; "No limit" still stops at one million rows with a "limit reached" notice) and a
  cut result reads "1,000 rows (auto limited 1,000 rows)"; the bar shows the cursor as
  "Line 4, Column 13, Location 40" (1-based, plus the selected count). In the History pane a click
  selects, a double-click opens the statement in a new tab and **never runs it** (Run in New Tab is
  in the menu, ⌘↩ on the selected row). **Open** (⌘O, the SQL Viewer, links and Open Recent) does
  not load a huge dump into the editor: above 8 MB (2 MB gzipped) it asks to open the first 5 MB
  (an unsaved copy that is not linked to the file, so saving never truncates it), the whole file,
  or the SQL import wizard (File ▸ Import ▸ SQL dump, which streams the file); a file that is not
  UTF-8 opens as Windows-1252 (UTF-16 with a byte order mark as UTF-16) and a toast says so, saving
  writes UTF-8; a file over 512 MB is never read whole.
- **Structure** — columns (with a primary-key toggle), indexes, foreign keys and triggers; staged
  DDL edits with preview (SQLite table rebuilds when `ALTER TABLE` cannot do it),
  create/rename/truncate/drop tables, **Open in SQL** (the table's `CREATE` statement in a new
  query tab). The staged changes of one table are one `ALTER TABLE a, b, c` on MySQL and
  PostgreSQL, as in TablePlus (a clause the server would run out of order or refuse next to an
  earlier one, a rename, a comment, a table option or another table starts a new statement; on
  MySQL also a column re-added under the name of a dropped one that a key covers, and a foreign
  key dropped and added under one name, both of which the server refuses inside one `ALTER`): a
  failing clause leaves the table as it was ("All changes were reverted"), which also holds on
  MySQL for a plan of one statement; a MySQL plan of several statements (several tables) keeps the
  "stopped partway" note. Add a column, index or trigger with **+ Column** / **+ Index** /
  **+ Trigger**, ⌘I, or by double-clicking the blank row that ends each grid (or the empty space
  below it); click a column (or ⌘ / ⇧-click several) and press Delete, ⌘⌫ or the right-click
  **Delete N columns** to drop the selection in one undo step. MySQL index columns take a prefix
  length per char / text / blob column (`name(10)`; a TEXT or BLOB column needs one, the Preview
  refuses the plan without it), and **on update CURRENT_TIMESTAMP** follows the column's
  fractional seconds (`datetime(3)` → `CURRENT_TIMESTAMP(3)`, also for `DEFAULT`).
- **Export** — streaming CSV, JSON / NDJSON, SQL `INSERT`, HTML and Excel export of tables or
  queries with progress and cancel; partial files are removed on cancel and on quit. ⇧⌘E / File ▸
  Export… is the multi-table export (the active table checked); a table tab's own Export buttons
  export that table's filtered rows, or only the current page: a page after the first is a slice of
  the table query (`OFFSET`, ordered by the sort then the primary key; `rowid` / every column
  without a key), so hidden columns and generated columns are left out on every page. In a SQL tab
  ⇧⌘E exports the result on screen; a result cut at the row limit can export all its rows by
  running a read-only statement again, never a `DELETE … RETURNING` or `CALL`; when the run holds
  several results the dialog offers **This result / All results (N)** (one Excel workbook with a
  sheet per result, one HTML page with a heading per result, one SQL script, or a file per result
  `<name>-1.csv`, `-2`… in a folder for CSV / JSON). CSV options: delimiter, quote (`"`, `'`,
  **Space**: every value between spaces, a space inside doubled; or only when needed), line break,
  and a decimal separator that is `.`, `,` or **Other…** (one ASCII character that is not a digit,
  a quote or the delimiter; the same choices are the default of Copy as CSV, exports and the CSV
  import, whose quote also accepts a space). The file follows the grid's column order and hidden
  columns ("Columns in grid order"), and its byte encoding follows Connection ▸ View Using Encoding
  (latin1, cp1252 and UTF-16 connections preselect that encoding; the text itself is Unicode, what
  the encoding cannot hold is written as `?`). SQL scripts end each `INSERT` at the row batch or at
  about 250 KB (a row is never split, so one huge row still goes out whole), start with
  `SET TIME ZONE` (PostgreSQL) / `SET TIME_ZONE` (MySQL, put back at the end) set to the zone the
  exporting session read the values in (a MySQL `SYSTEM` zone becomes its `+HH:MM` offset), so
  `TIMESTAMP` values load as the same instants in a session of another zone, and write `untitled`
  as the table of a result whose table is unknown (a join, a computed column).
- **Safe mode** — TablePlus's five levels per connection (Silent, Alert Mode 1/2, Safe Mode 1/2)
  from the toolbar lock: an alert or the connection's password before queries, commits, imports,
  restores and kills reach the server (levels 2 skip SELECT / EXPLAIN / SHOW). A default level for
  new connections lives in Settings → Security.
- **Connections, more** — client SSL key / certificate, Bootstrap commands (SQL run after every
  login), password storage Keychain / Ask every time / No password, `~/.ssh/config` fallback for
  SSH, connection groups (drag a row onto a group), sort by name / tag / driver, keep-alive pings,
  import / export of connections to an encrypted `.tablerconnections` file.
- **Connection details** — the connect timeout is 60 s unless the form or Settings says otherwise.
  **~/.ssh/config:** `Include` (relative paths from `~/.ssh`, `~/`, absolute, `*` `?` `[…]` globs in
  lexical order, nested up to 16 deep, cycles and missing files skipped) is followed in place, so an
  alias from `Include ~/.orbstack/ssh/config`, Lima, Colima or Tailscale resolves; `Match all`,
  `Match host` / `originalhost` / `user` / `localuser` (with `!` and several criteria) are evaluated,
  any other `Match` criterion (`exec`, `localnetwork`, `canonical`, `final`…) makes its block not
  apply. **MySQL SSL:** VERIFY-CA checks the certificate chain (the CA file, else the system's
  roots) and the validity period but not the host name; VERIFY-IDENTITY (VERIFY-SERVER-CERT on the
  MariaDB list) checks the name too. This is a patch of the vendored `mysql_async`
  (`src-tauri/vendor/mysql_async`, see its `Cargo.toml`): upstream looked for the text
  "NotValidForName" in a rustls error that no longer says that, so the name was always checked.
  **Ver. menu:** Default, MariaDB, Old MySQL (5.x) and SingleStore DB (MariaDB-style SSL list;
  links' `driverVersion=3`). Old MySQL accepts the pre-4.1 `mysql_old_password` login and talks
  TLS 1.0 / 1.1 for 5.5 / 5.6 servers through macOS Secure Transport (rustls, the other variants'
  engine, only speaks TLS 1.2 and 1.3); limits: a client certificate (Key… / Cert…) is only sent
  over TLS 1.2+ with Old MySQL (Secure Transport takes PKCS#12, not PEM pairs), and on other
  platforms than macOS Old MySQL has no TLS 1.0 / 1.1. Tested against a fake server that offers
  nothing newer than TLS 1.1 (`src-tauri/tests/conn4_oldtls.rs`, needs python3 and openssl).
  **Enable CLEARTEXT_PLUGIN** also works through an SSH tunnel with SSL mode DISABLED (the tunnel
  encrypts the connection); over plain TCP it is still refused. **Use socket** looks for the
  socket at `/tmp/mysql.sock`, `/var/run/mysqld/mysqld.sock`, Homebrew (Apple silicon and Intel),
  MAMP, XAMPP, MacPorts (MySQL 8 / 5.7), `/var/lib/mysql/mysql.sock` and `/run/mysqld/mysqld.sock`,
  the first that exists (over SSH the same list on the server). **New connection** has Import from
  URL… on its first step and its form. **Quit** closes every SQLite file, so a WAL database leaves
  no `-wal` / `-shm` files and its pages are in the main file. **Help** has Changelog and Report a
  Bug / Send Feedback… (the project's releases and issue tracker) next to Enable SSH Debug Log and
  Show Log File.
- **Workspace** — workspaces and SQL tabs reopen at startup (Auto Save SQL Query Tab: none / first
  / all), Rename Tab, Show Workspaces (⇧⌘\), remappable shortcuts (Settings → Shortcuts).
- **Database objects** — sidebar Functions (functions, procedures, aggregates) and PostgreSQL
  Sequences that open their definition in a SQL tab; TablePlus item menus (Open Structure, Item
  Overview, Export…, Import ▸, New ▸, Clone…, Truncate ⌥⌫, Delete ⌫ / CASCADE ⇧⌫ / without FK
  checks ⌘⌫, Copy Script As ▸); multi-selection (⌘ / ⇧-click); Item Overview with editable comments;
  Open Anything also finds functions and saved queries.
- **Grid, more** — every TablePlus filter operator (Contains, Not contains, Has prefix / suffix and
  their case variants, ILIKE, NOT BETWEEN, JSON contains), an **Any column** filter, Apply All with
  AND (⌘↩) or OR (⇧⌘↩), filters remembered per table, Quick Look editor (⌘↩: JSON tree and text
  with sorted keys, a blob as text / hex / base64, image preview, referenced row), bulk edit of
  many rows in the right sidebar, Set Value ▸ NOW() /
  Add a File…, Copy Rows As ▸ Plain text / JSON / HTML / Markdown / CSV / CSV with header / SQL Insert.
  ⌘V takes comma / semicolon separated text as well (RFC 4180 quotes; lines of one field per column
  become new rows). The footer reads "15 of ~999,985 rows selected"; the filter bar's **SQL** button
  shows the whole generated query with a Copy button; ⌘↑ / ⌘↓ move between the filter bar and the
  data; a result grid's right-click menu has **Export Result…**.
- **Structure, more** — TablePlus header (Name, Primary, Comment, Options, Triggers, Info), default
  presets, MySQL `extra`, column CHECKs and table CHECK constraints, triggers, MySQL engine / charset /
  collation / auto increment, editable view definitions.
- **SQL workflow** — a Queries pane (saved queries in folders, bound to keywords that expand in the
  editor; ⌥⌘S adds one; a saved query's menu has Run, Copy, **Import…** (its `.sql` file through the
  SQL import wizard), Open in New Tab and Insert, while "Add Files to Queries…" brings `.sql` files
  in), query parameters (`:name`, `${name}`, … with a values dialog), Return on
  Error, ⚙ editor settings, multiple carets, split pane (⇧⌘D), Save Selected Statements As (⇧⌘S),
  **editable query results** (one source table with its primary key; ⌘S commits), pinned results,
  append-all-results mode, a query time out (300 s by default, Settings ▸ Connections; it applies to
  every query the app runs — the SQL editor, a table's page loads, filters and sorts, row counts,
  cell fetches and Explain — and cancels the statement on the server; the grid then shows "Query
  timed out after 300 s" and where to change the limit).
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
- **Wave 3** — multiple windows (⌘N, Window ▸ Move Tab to New Window), one workspace per database
  with a database icon rail, split workspaces (⇧⌘D), tool tabs (Item Overview, Metrics Board, SQL
  Viewer); SQLCipher, custom tags, MySQL / PostgreSQL connection options, a bootstrap script,
  SSH jump hosts / ProxyCommand / IdentityAgent, SQLite and MySQL sockets over SSH, deep links
  (`open -a Tabler "postgres://…"`) and Finder file associations; Touch ID, a passcode lock screen
  and a customisable toolbar; geometry decoding with a Map view, rectangular cell selection, hex
  editing and a floating Quick Look; function completion and JOIN suggestions, `.sql` files for saved
  queries and a movable data folder (Locations); HTML export, JSON import, multi-database export,
  file encodings and notifications; **one commit for the whole workspace** (⌘S) with server-side
  `NOW()`: staged rows, result edits, structure, **new / edited views, functions and routines**
  (counted by the toolbar, previewed with ⇧⌘P, discarded with ⇧⌘⌫ back to the text they were opened
  with) and sidebar operations commit together, in one transaction where the engine allows it. On
  MySQL, row edits alone are one transaction too (a failure says "All changes were reverted");
  its DDL commits statement by statement, and the sheet says what stayed. After a commit every
  other open tab of a written table reloads; lazy-loaded schemas; File ▸ Open Recent;
  the **Metrics Board**, **JavaScript plugins**, an **AI assistant** (structure only) and an
  **MCP server** (`tabler --mcp`); Fonts & Colors with custom themes. Every shortcut and where it
  differs from TablePlus is in [`docs/shortcuts.md`](docs/shortcuts.md).
- **Opening things** — connection URLs and links (`open -a Tabler "postgres://…"`, `tabler://connection/<id>`,
  "New connection from URL…", Copy as URL): `name`, `statusColor` (a colour that is not a preset stays
  that custom hex), `env`, `safeModeLevel`, `windowMode=isolated` (the new window opens the table with the
  link's row filter), `lazyload` (also for a saved connection: it sets that connection's sidebar
  lazy-load setting), `schema` / `table` / `view` and a row filter (`column` + `operation` + `value`).
  A raw SQL condition (`condition` / `raw` / `query`) opens the table with the SQL in the filter bar and
  asks first in a dialog that shows the SQL ("Apply filter" or "Not now"; a read-only connection applies
  it at once). PostgreSQL URLs also carry libpq parameters: `sslnegotiation=direct`,
  `channel_binding=disable` and `connect_timeout` fill their fields, and every other key Tabler does not
  read itself (`jit=off`, `application_name`, `options`, `statement_timeout`…) goes to the form's
  Parameters field and back out in Copy as URL (libpq keys that are no server settings, such as
  `keepalives`, are left out). Double-clicking an encrypted SQLite file in Finder asks for its SQLCipher
  key (used for that connection only, never stored). Connection groups take one of 24 icons or an image
  of your own (a PNG, JPEG, WebP, SVG or GIF up to 2 MB, drawn onto 64x64 and kept as a PNG of at most
  24 KB; it travels in exported `.tablerconnections` files). One table can sit in several tabs (Open in
  New Tab, middle-click, ⌘↩ / ⌘-click in Open Anything, Duplicate Tab). Open Anything also finds app
  commands, a click only selects (↩ or a double-click opens), and ⌘W closes the window when there is no
  tab to close.
- **Comfort** — TablePlus keys by default: Open Anything (⌘P), database switcher (⌘K), connection
  switcher (⇧⌘K), a console log of every statement the app runs (⇧⌘C: the SQL that really ran —
  catalog SELECTs, `COUNT(*)`, page loads, edits, the `EXPLAIN` an Explain runs (and the
  transaction an Analyze wraps around it), every statement of a SQL dump import (a long dump keeps
  the newest 500) — filterable as Meta / Data queries, with timings, rows and errors; a statement
  that carries a password, like a user change, is logged with the password masked; a call that ran
  no SQL, like a cached table structure, is a `-- …` description),
  keyboard-first everything, a native macOS menu bar (File / Edit / View / Window) wired to the same
  actions, light/dark themes that follow macOS (the chosen theme also sets the native appearance:
  menu bar, file panels and message boxes, in every window), tab management with unsaved-change guards (closing a tab, the window,
  or quitting from the menu bar, ⌘Q or the Dock asks first). The sidebar, status pill and accent always follow the visible
  tab's connection. A dropped connection shows the reconnect banner at the first failing query,
  grid commit or DDL apply. ⇧⌘R (Connection ▸ Reconnect) is a **force reconnect**: it opens a new
  session from the profile even when the old one still answers a ping (to reset session variables or
  `search_path`, or after a network or SSH tunnel change), keeps the tabs, staged edits and current
  database, releases the old session and says so when a running query was stopped; a failed forced
  reconnect leaves the old session as it was. It works from the Connection Manager too. The banner's
  Reconnect button and the automatic reconnect (window focus, network back) ping first and only
  reopen a dead session.

Values are lossless across the UI: 64-bit integers beyond 2⁵³ and decimals are kept as text,
timestamps keep the server's textual form. A binary value reads in the grid as its text when it is
UTF-8 text (up to 256 bytes) and as hex (`0x…`: BINARY(16) keys, bytea ids) up to 32 bytes; a
larger one shows as a size, is previewed on demand and reads as text in Quick Look and the Cell
view when its bytes are text (Text / Hex / Base64; editing stays hex). JSON viewers (Quick Look,
the Cell view, the Details fields) can list the keys sorted: display only, nothing is staged
until you save the sorted text. ⌘S in a cell editor or a Details field stages what was typed
first and then commits.

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
⌥⇧F still work as alternatives; ⌘N is New Window and ⌘L the plugin manager, as in TablePlus).

| Global    |                           | Data grid   |                             | SQL editor  |                         |
| --------- | ------------------------- | ----------- | --------------------------- | ----------- | ----------------------- |
| ⌘P        | open anything             | ↩ / F2      | edit cell                   | ⌘↩          | run current / selection |
| ⌘K        | switch database           | ⌥↩          | multi-line / JSON edit      | ⇧⌘↩         | run all                 |
| ⇧⌘K       | switch connection         | ⌘S          | commit                      | ⌘.          | cancel                  |
| ⌘T / ⌘E   | new query tab             | ⇧⌘P         | preview SQL                 | ⌥⌘E         | explain                 |
| ⌘W        | close tab                 | ⇧⌘⌫         | discard changes             | ⌘I / ⇧⌘I    | beautify / uglify       |
| ⇧⌘T       | reopen closed tab         | ⌘Z / ⇧⌘Z    | undo / redo staged change   | ⌃\`         | toggle result pane      |
| ⌘[ / ⌘]   | previous / next tab       | ⌘F          | filter bar                  | ⌘H          | history                 |
| ⌘1…9      | go to tab                 | ⌥⌘F         | show / hide columns         | ⌘S / ⌘O     | save / open `.sql`      |
| ⌃⌘[ / ⌃⌘] | data / structure view     | ⌘I          | new row                     | ⌘/          | toggle comment          |
| ⌘0        | toggle sidebar            | ⌘D          | duplicate row               | ⇧⌘E         | export the result       |
| ⇧⌘I       | toggle row detail         | ⌫ / ⌘⌫      | delete rows                 | ⌥⌘S         | add to Queries          |
| ⇧⌘C       | console log               | ⇧⌘N / ⌥⌘D   | set NULL / DEFAULT          | ⇧⌘S         | save selected as…       |
| ⇧⌘F       | search tables             | ␣ (Space)   | row detail                  | ⇧⌘D         | split pane              |
| ⇧⌘D       | split right (not in SQL)  | ⇧⌘C         | copy cell value             |             |                         |
| ⌘R / ⌥⌘R  | reload workspace / tab    | ⌘J          | open referenced row         | ⌘D / ⌃⌘G    | next / all occurrences  |
| ⇧⌘R       | force reconnect           | ⌘C / ⌥⌘C    | copy rows / copy as…        | ⌃⇧↑ / ⌃⇧↓   | add caret above / below |
| ⇧⌘E       | export tables             | ⌘V          | paste cells / rows          | ⌘+ / ⌘−     | font size               |
| ⌘N        | new window                | ⌘↩          | Quick Look editor           |             |                         |
| ⇧⌘O       | new SQL viewer            | ⌥↩          | Set Value menu              | **Filters** |                         |
| ⌘,        | settings                  | ⌘← / ⌘→     | previous / next page        | ⌘↩ / ⇧⌘↩    | apply all with AND / OR |
| ⌘.        | running queries           |             |                             | ⌘I / ⇧⌘I    | duplicate / remove row  |
| ⇧⌘\       | show workspaces           | **Sidebar** |                             | ⌘↑ / ⌘↓     | between filters / data  |
| ⌃⌘← / ⌃⌘→ | previous / next workspace | ⌥⌫          | truncate                    | ⌘B          | toggle the filter       |
| ⌃- / ⌃⇧-  | back / forward            | ⇧⌫ / ⌘⌫     | drop CASCADE / no FK checks |             |                         |
| ⌘Q        | quit (asks if unsaved)    | ⌘A          | select all items            |             |                         |

The full, searchable list is in Settings → Shortcuts. The native menu bar offers the same
commands (File: New Query Tab, Open SQL File…, Close Tab / Window, Close All Tabs, Reopen Closed Tab,
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
