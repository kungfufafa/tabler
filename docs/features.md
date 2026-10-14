# Tabler features in detail

The [README](../README.md) gives the overview. This page lists what each part of Tabler does,
including the edge cases. Every keyboard shortcut is in [`shortcuts.md`](shortcuts.md), and how
each feature compares with TablePlus is in [`tableplus-parity.md`](tableplus-parity.md).

**Contents:** [Layout and workspace](#layout-and-workspace) ·
[Connections](#connections) · [Browsing data](#browsing-data) · [Editing data](#editing-data) ·
[SQL editor](#sql-editor) · [Structure](#structure) · [Database objects](#database-objects) ·
[Import, export and backup](#import-export-and-backup) · [Safety](#safety) ·
[Server administration](#server-administration) · [Console log](#console-log) ·
[Extensions](#extensions) · [How values are shown](#how-values-are-shown)

## Layout and workspace

### The window

- **Toolbar** with Discard / Preview / Commit and panel toggles. The toolbar can be customised.
- **Status pill** tinted with the connection colour:
  `LOCAL │ PostgreSQL 17.6 : name : database : schema.table`.
- **Connection rail**: every open connection is its own workspace of tabs. Each database gets its
  own workspace too, with a database icon rail.
- **Sidebar** with Items | History.
- **Bottom bar** of a table: `Data | Structure`, `+ Row`, Filters, Columns and paging.
- **Welcome screen** listing the connections with their environment tag (Local, Testing,
  Development, Staging, Production).
- The sidebar, the status pill and the accent colour always follow the visible tab's connection.

### Tabs

- A single click in the sidebar opens a table in a replaceable _preview_ tab (shown in italic), as
  in TablePlus. The sidebar keeps the last five tables under "Recent".
- Data and Structure are two views of the same tab (⌃⌘[ / ⌃⌘]).
- One table can sit in several tabs: Open in New Tab, middle-click, ⌘↩ / ⌘-click in Open Anything,
  or Duplicate Tab.
- Rename Tab, and tool tabs for Item Overview, Metrics Board and SQL Viewer.
- Unsaved-change guards: closing a tab or the window, or quitting from the menu bar, ⌘Q or the Dock
  asks first.
- ⌘W closes the window when there is no tab left to close.

### Windows and workspaces

- Multiple windows (⌘N, Window ▸ Move Tab to New Window) and split workspaces (⇧⌘D).
- Show Workspaces (⇧⌘\\).
- Workspaces and SQL tabs reopen at startup (setting: Auto Save SQL Query Tab — none / first / all).

### Finding things

- **Open Anything (⌘P)** finds tables, functions, saved queries and app commands. A click only
  selects; ↩ or a double-click opens.
- **Database switcher (⌘K)** and **connection switcher (⇧⌘K)**.

### Menu bar

The native macOS menu bar is wired to the same actions as the app:

| Menu       | Commands                                                                                                                                                    |
| ---------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------- |
| File       | New Query Tab, Open SQL File…, Open Recent, Close Tab / Window, Close All Tabs, Reopen Closed Tab, Backup…, Restore…, Export…, Import ▸, Connection Manager |
| Edit       | The standard Undo / Cut / Copy / Paste / Select All for text fields                                                                                         |
| View       | Open Anything, Switch Database, Switch Connection, Data, Structure, Toggle Sidebar, Toggle Row Detail, Toggle Console Log, Refresh, Full Screen             |
| Navigate   | Select Next / Previous Pane (⇧⌘] / ⇧⌘[)                                                                                                                     |
| Connection | Process List…, User Management…, Reconnect, Disconnect                                                                                                      |
| Window     | Show Workspaces, Select Next / Previous Workspace                                                                                                           |
| Help       | Changelog, Report a Bug / Send Feedback… (the project's releases and issue tracker), Enable SSH Debug Log, Show Log File                                    |

Menu shortcuts only act when the focused part of the page does not handle the key itself.

### Look and feel

- Light and dark themes that follow macOS. The chosen theme also sets the native appearance (menu
  bar, file panels and message boxes) in every window.
- Fonts & Colors with custom themes.
- Remappable shortcuts (Settings → Shortcuts) and notifications.

## Connections

### Profiles

- Saved profiles with colours, custom tags and an environment tag.
- **Groups**: nested, sorted by name / tag / driver; drag a row onto a group to move it. A group
  takes one of 24 icons or an image of your own (PNG, JPEG, WebP, SVG or GIF up to 2 MB, drawn onto
  64×64 and kept as a PNG of at most 24 KB; it travels in exported connection files).
- Opening a profile that is already open switches to it instead of starting a second session.
- **New connection** has Import from URL… on its first step and on its form.
- Import / export connections to an encrypted `.tablerconnections` file.
- **Bootstrap commands**: SQL that runs after every login (or a bootstrap script).
- MySQL and PostgreSQL connection options, read-only sessions and keep-alive pings.
- The connect timeout is 60 s unless the form or Settings says otherwise.

### Passwords

- Passwords live in the macOS Keychain, never in plain files.
- Password storage per connection: Keychain / Ask every time / No password.
- IAM ("Command Line") passwords with token refresh.

### SSL / TLS

- SSL modes, and a client SSL key / certificate.
- **MySQL VERIFY-CA** checks the certificate chain (the CA file, else the system's roots) and the
  validity period, but not the host name. **VERIFY-IDENTITY** (VERIFY-SERVER-CERT on the MariaDB
  list) checks the name too.
  - This comes from a patch of the vendored `mysql_async` (`src-tauri/vendor/mysql_async`, see its
    `Cargo.toml`). Upstream looked for the text "NotValidForName" in a rustls error that no longer
    says that, so the name was always checked.

### SSH tunnels

- Password, private key with passphrase, agent, and keyboard-interactive / 2FA prompts.
- Host-key verification.
- Jump hosts, ProxyCommand and IdentityAgent.
- SQLite files and MySQL sockets over SSH.
- **`~/.ssh/config` fallback**:
  - `Include` is followed in place: relative paths from `~/.ssh`, `~/`, absolute paths and
    `*` `?` `[…]` globs in lexical order, nested up to 16 deep; cycles and missing files are
    skipped. So an alias from `Include ~/.orbstack/ssh/config`, Lima, Colima or Tailscale resolves.
  - `Match all` and `Match host` / `originalhost` / `user` / `localuser` (with `!` and several
    criteria) are evaluated. Any other `Match` criterion (`exec`, `localnetwork`, `canonical`,
    `final`…) makes its block not apply.

### PostgreSQL

Connection URLs also carry libpq parameters:

- `sslnegotiation=direct`, `channel_binding=disable` and `connect_timeout` fill their own fields.
- Every other key Tabler does not read itself (`jit=off`, `application_name`, `options`,
  `statement_timeout`…) goes to the form's Parameters field, and back out in Copy as URL.
- libpq keys that are not server settings, such as `keepalives`, are left out.

### MySQL / MariaDB

- **Ver. menu**: Default, MariaDB, Old MySQL (5.x) and SingleStore DB (MariaDB-style SSL list;
  links use `driverVersion=3`).
- **Old MySQL** accepts the pre-4.1 `mysql_old_password` login and talks TLS 1.0 / 1.1 to 5.5 / 5.6
  servers through macOS Secure Transport (rustls, the engine of the other variants, only speaks
  TLS 1.2 and 1.3). Limits:
  - a client certificate (Key… / Cert…) is only sent over TLS 1.2+ (Secure Transport takes
    PKCS#12, not PEM pairs);
  - on platforms other than macOS there is no TLS 1.0 / 1.1.
  - Tested against a fake server that offers nothing newer than TLS 1.1
    (`src-tauri/tests/conn4_oldtls.rs`, needs python3 and openssl).
- **Enable CLEARTEXT_PLUGIN** works through an SSH tunnel with SSL mode DISABLED (the tunnel
  encrypts the connection). Over plain TCP it is still refused.
- **Use socket** takes the first socket that exists among `/tmp/mysql.sock`,
  `/var/run/mysqld/mysqld.sock`, Homebrew (Apple silicon and Intel), MAMP, XAMPP, MacPorts
  (MySQL 8 / 5.7), `/var/lib/mysql/mysql.sock` and `/run/mysqld/mysqld.sock`. Over SSH the same
  list is checked on the server.

### SQLite

- **Choose…** an existing file or **New…** to create one. Test / Connect never create a missing
  file, so a moved or mistyped path is an error.
- Foreign-key enforcement (`PRAGMA foreign_keys`) is a switch under Advanced.
- SQLCipher. Double-clicking an encrypted SQLite file in Finder asks for its key, which is used for
  that connection only and never stored.
- **Quit** closes every SQLite file, so a WAL database leaves no `-wal` / `-shm` files behind and
  its pages are in the main file.

### Reconnecting

- A dropped connection shows the reconnect banner at the first failing query, grid commit or DDL
  apply.
- The banner's Reconnect button and the automatic reconnect (window focus, network back) ping
  first and only reopen a dead session.
- **⇧⌘R (Connection ▸ Reconnect) is a force reconnect.** It opens a new session from the profile
  even when the old one still answers a ping, for example to reset session variables or
  `search_path`, or after a network or SSH tunnel change.
  - Tabs, staged edits and the current database are kept.
  - The old session is released, and Tabler says so when a running query was stopped.
  - A failed forced reconnect leaves the old session as it was.
  - It works from the Connection Manager too.

### Links and URLs

- Open connections from outside the app: `open -a Tabler "postgres://…"`,
  `tabler://connection/<id>`, Finder file associations, "New connection from URL…" and Copy as URL.
- Supported link parameters:
  - `name`, `env`, `safeModeLevel`
  - `statusColor` (a colour that is not a preset stays that custom hex)
  - `windowMode=isolated` (the new window opens the table with the link's row filter)
  - `lazyload` (also for a saved connection: it sets that connection's sidebar lazy-load setting)
  - `schema` / `table` / `view`, and a row filter (`column` + `operation` + `value`)
- A raw SQL condition (`condition` / `raw` / `query`) opens the table with the SQL in the filter
  bar, but first asks in a dialog that shows the SQL ("Apply filter" or "Not now"). A read-only
  connection applies it at once.

## Browsing data

### Sidebar

- Schema / database pickers and a searchable table list with row estimates.
- **Pins**: "Pin to Top" in a table's context menu, kept per connection and database.
- Sidebar groups with filter rules and a per-table column list.
- Lazy-loaded schemas.

### Data grid

- Virtualized, smooth with 100k+ rows.
- Server-side sort and paging.
- Column header menu; resize, hide and drag-to-reorder columns.
- Rectangular cell selection. The footer reads like "15 of ~999,985 rows selected".
- Find and replace in results (⌘F).
- Geometry decoding with a Map view, and hex editing.

### Filters

- Every TablePlus operator: `=`, `≠`, `<`…, `LIKE`, ILIKE, `IN`, `BETWEEN`, NOT BETWEEN,
  `IS NULL`, Contains, Not contains, Has prefix / suffix (and their case variants), JSON contains.
- An **Any column** filter and **Raw SQL** conditions.
- Combine with AND / OR; Apply All with AND (⌘↩) or OR (⇧⌘↩).
- Quick filters from a cell; filters are remembered per table.
- The filter bar's **SQL** button shows the whole generated query with a Copy button.
- ⌘↑ / ⌘↓ move between the filter bar and the data.

### Row details and Quick Look

- **Row-details sidebar** (␣ toggles it), editable. Every text, JSON and XML field is a compact
  CodeMirror editor with multiple carets, search and syntax colours. Full blobs are in its Cell
  view. Select many rows to bulk-edit them here.
- **Quick Look editor** (⌘↩), also as a floating window: a JSON tree and text with sorted keys, a
  blob as text / hex / base64, an image preview, and the referenced row.

### Foreign keys

- Jump to the referenced row. When a column has several keys, a menu asks which one.
- Composite keys filter by every column; binary keys filter by their hex.

### Copying

- Copy as TSV / CSV / JSON / INSERT / Markdown (⌥⌘C opens the picker).
- Copy Rows As ▸ Plain text / JSON / HTML / Markdown / CSV / CSV with header / SQL Insert.
- A result grid's right-click menu has **Export Result…**.

## Editing data

### Editing cells and rows

- Inline, type-aware cell editing. Edited, new and deleted rows are highlighted until you commit.
- ⌥↩ opens multi-line text, and a JSON editor with validation.
- **Paste (⌘V)** cells and rows from Numbers, Excel, TSV or CSV. Comma- or semicolon-separated
  text works too (RFC 4180 quotes); lines with one field per column become new rows.
- **Set Value** ▸ NULL / EMPTY / DEFAULT (also ⌥-click), NOW() and Add a File….
- Duplicate and delete rows; double-click below the last row for a new one.
- Tables without a primary key are handled safely.

### Preview and commit

- **⌘S commits straight away**; ⇧⌘P previews the SQL first.
- Changes apply in a single transaction, with errors reported per row.
- Undo / redo (⌘Z / ⇧⌘Z) of staged changes.
- ⌘S in a cell editor or a Details field stages what was typed first, then commits.

### One commit for the whole workspace

⌘S commits everything staged in the workspace together, in one transaction where the engine
allows it, with server-side `NOW()`:

- staged rows and edits of query results;
- structure changes;
- new and edited views, functions and routines (counted by the toolbar, previewed with ⇧⌘P,
  discarded with ⇧⌘⌫ back to the text they were opened with);
- sidebar operations such as truncate and drop.

On MySQL, row edits alone are one transaction too (a failure says "All changes were reverted"). Its
DDL commits statement by statement, and the sheet says what stayed. After a commit, every other open
tab of a written table reloads.

## SQL editor

### Writing queries

- CodeMirror with schema-aware autocomplete per dialect, function completion and JOIN suggestions.
- Multiple carets, ⚙ editor settings, and a split pane (⇧⌘D).
- Beautify / uglify the current statement, and formatting.
- **Query parameters** (`:name`, `${name}`, …) with a dialog for the values.
- **Saved queries** in the Queries pane:
  - organised in folders and bound to keywords that expand in the editor;
  - ⌥⌘S adds one; "Add Files to Queries…" brings `.sql` files in; saved queries are stored as
    `.sql` files;
  - a saved query's menu has Run, Copy, **Import…** (its `.sql` file through the SQL import
    wizard), Open in New Tab and Insert.

### Running queries

- Run the current statement (⌘↩), all statements (⇧⌘↩) or the selection.
- Multiple result sets, streamed as they arrive. Cancel a running query, Return on Error,
  append-all-results mode, pinned results, and a collapsible result pane.
- Errors are highlighted at their position in the editor.
- PostgreSQL `RAISE NOTICE` messages are shown.
- **Editable query results** when the result comes from one source table with its primary key
  (⌘S commits).
- **Charts** from a comment: `-- TablePlus BarChart x: a, y: b`.
- **EXPLAIN.** EXPLAIN ANALYZE of `INSERT` / `UPDATE` / `DELETE` runs inside a transaction that is
  rolled back, so the rows stay.
- The bar shows the cursor as "Line 4, Column 13, Location 40" (1-based, plus the selected count).

### Row limit and time out

- A new SQL tab starts at **No limit**. The row-limit select also offers 100, 300, 500, 1,000 and
  10,000. "No limit" still stops at one million rows, with a "limit reached" notice.
- A cut result reads "1,000 rows (auto limited 1,000 rows)".
- **Query time out**: 300 s by default (Settings ▸ Connections). It applies to every query the app
  runs: the SQL editor, a table's page loads, filters and sorts, row counts, cell fetches and
  Explain. It cancels the statement on the server, and the grid then shows "Query timed out after
  300 s" and where to change the limit.

### History

- A click selects a statement; a double-click opens it in a new tab and **never runs it**.
- Run in New Tab is in the menu, or press ⌘↩ on the selected row.

### Files

- Open and save `.sql` files; Save Selected Statements As (⇧⌘S); File ▸ Open Recent.
- **Open** (⌘O, the SQL Viewer, links and Open Recent) does not load a huge dump into the editor:
  - Above 8 MB (2 MB gzipped) it asks whether to open the first 5 MB, the whole file, or the SQL
    import wizard (File ▸ Import ▸ SQL dump, which streams the file). The first-5 MB copy is
    unsaved and not linked to the file, so saving never truncates it.
  - A file over 512 MB is never read whole.
- A file that is not UTF-8 opens as Windows-1252 (UTF-16 with a byte order mark as UTF-16) and a
  toast says so. Saving writes UTF-8.

## Structure

### What you can edit

- Columns (with a primary-key toggle), indexes, foreign keys, triggers, column CHECKs and table
  CHECK constraints.
- TablePlus header: Name, Primary, Comment, Options, Triggers, Info. Default presets.
- MySQL `extra`, engine, charset, collation and auto increment.
- PostgreSQL trigger enable / disable and index INCLUDE columns.
- Editable view definitions.
- Create, rename, truncate and drop tables.
- **Open in SQL**: the table's `CREATE` statement in a new query tab.
- **ER diagram**: ⌘P ▸ Generate Diagram…, with SVG / PNG export.

### Adding and removing

- Add a column, index or trigger with **+ Column** / **+ Index** / **+ Trigger**, ⌘I, or by
  double-clicking the blank row at the end of each grid (or the empty space below it).
- Click a column (or ⌘ / ⇧-click several) and press Delete, ⌘⌫ or the right-click
  **Delete N columns** to drop the selection in one undo step.

### How changes are applied

- DDL edits are staged and can be previewed before they run. On SQLite, the table is rebuilt when
  `ALTER TABLE` cannot do the change.
- On MySQL and PostgreSQL, the staged changes of one table become one `ALTER TABLE a, b, c`, as in
  TablePlus. A new statement starts for:
  - a clause the server would run out of order or refuse next to an earlier one;
  - a rename, a comment, a table option, or another table;
  - on MySQL also: a column re-added under the name of a dropped one that a key covers, and a
    foreign key dropped and added under one name (the server refuses both inside one `ALTER`).
- A failing clause leaves the table as it was ("All changes were reverted"). This also holds on
  MySQL for a plan of one statement. A MySQL plan of several statements (several tables) keeps the
  "stopped partway" note.

### MySQL details

- Index columns take a prefix length per char / text / blob column (`name(10)`). A TEXT or BLOB
  column needs one; the Preview refuses the plan without it.
- **on update CURRENT_TIMESTAMP** follows the column's fractional seconds (`datetime(3)` →
  `CURRENT_TIMESTAMP(3)`, also for `DEFAULT`).

## Database objects

- The sidebar lists Functions (functions, procedures, aggregates) and PostgreSQL Sequences. They
  open their definition in a SQL tab.
- TablePlus item menus: Open Structure, Item Overview, Export…, Import ▸, New ▸, Clone…,
  Truncate (⌥⌫), Delete (⌫) / with CASCADE (⇧⌫) / without FK checks (⌘⌫), Copy Script As ▸ (incl.
  Laravel Migration).
- Sidebar truncate and drop are staged and committed with ⌘S.
- Multi-selection with ⌘ / ⇧-click.
- Item Overview with editable comments.

## Import, export and backup

### Export formats

- Streaming CSV, JSON / NDJSON, SQL `INSERT`, HTML and Excel (.xlsx) export of tables or queries,
  with progress and cancel. Partial files are removed on cancel and on quit.
- SQL export options: DROP, structure, content and Gzip.

### What gets exported

- **⇧⌘E / File ▸ Export…** is the multi-table export (the active table is checked). Several
  databases can be exported at once.
- **A table tab's Export buttons** export that table's filtered rows, or only the current page. A
  page after the first is a slice of the table query (`OFFSET`, ordered by the sort, then the
  primary key, `rowid`, or every column when there is no key), so hidden and generated columns are
  left out on every page.
- **In a SQL tab, ⇧⌘E** exports the result on screen.
  - A result cut at the row limit can export all its rows by running a read-only statement again,
    never a `DELETE … RETURNING` or `CALL`.
  - When the run holds several results, the dialog offers **This result / All results (N)**: one
    Excel workbook with a sheet per result, one HTML page with a heading per result, one SQL
    script, or a file per result (`<name>-1.csv`, `-2`…) in a folder for CSV / JSON.
- The file follows the grid's column order and hidden columns ("Columns in grid order").

### CSV options

- Delimiter, line break, NULL → empty.
- Quote: `"`, `'`, **Space** (every value between spaces, a space inside doubled), or only when
  needed.
- Decimal separator: `.`, `,` or **Other…** (one ASCII character that is not a digit, a quote or
  the delimiter).
- The same choices are the default for Copy as CSV, exports and the CSV import (whose quote also
  accepts a space).

### Encoding

The byte encoding follows Connection ▸ View Using Encoding; latin1, cp1252 and UTF-16 connections
preselect that encoding. The text itself is Unicode; what the encoding cannot hold is written as
`?`.

### SQL scripts

- Each `INSERT` ends at the row batch or at about 250 KB. A row is never split, so one huge row
  still goes out whole.
- Scripts start with `SET TIME ZONE` (PostgreSQL) / `SET TIME_ZONE` (MySQL, put back at the end),
  set to the zone the exporting session read the values in. A MySQL `SYSTEM` zone becomes its
  `+HH:MM` offset. So `TIMESTAMP` values load as the same instants in a session of another zone.
- A result whose table is unknown (a join, a computed column) is written to a table named
  `untitled`.

### Import

- CSV import wizard: create a table, matching strategies, STOP ON ERROR / REPLACE / UPDATE ON
  DUPLICATE.
- JSON import.
- SQL dump import (.sql / .sql.gz), streamed.

### Backup and restore

Through the installed `pg_dump` / `pg_restore` / `psql` / `mysqldump` / `mysql`, with a version
picker, option chips, Gzip and a file-name pattern.

## Safety

- **Safe mode**: TablePlus's five levels per connection (Silent, Alert Mode 1/2, Safe Mode 1/2),
  set from the toolbar lock. Before queries, commits, imports, restores and kills reach the server,
  Tabler asks for confirmation or the connection's password. The level-2 modes skip SELECT /
  EXPLAIN / SHOW. A default level for new connections lives in Settings → Security.
- Read-only sessions.
- Touch ID and a passcode lock screen.
- Passwords are never logged.

## Server administration

- Create, rename (PostgreSQL) and drop databases and schemas from ⌘K.
- **Running Queries** (⌘.) with Kill Query / Kill Connection.
- **User Management**: roles, global and database privileges, MySQL resource limits.
- **Metrics Board**.

## Console log

⇧⌘C opens a log of every statement the app runs — the SQL that really ran:

- catalog SELECTs, `COUNT(*)`, page loads and edits;
- the `EXPLAIN` an Explain runs, and the transaction an Analyze wraps around it;
- every statement of a SQL dump import (a long dump keeps the newest 500).

It can be filtered as Meta / Data queries and shows timings, rows and errors. A statement that
carries a password, like a user change, is logged with the password masked. A call that ran no SQL,
like a cached table structure, shows as a `-- …` description.

## Extensions

- **JavaScript plugins** (⌘L opens the plugin manager).
- **AI assistant** (it sees the structure only).
- **MCP server**: `tabler --mcp`.
- A movable data folder (Settings → Locations).

## How values are shown

Values are lossless across the UI:

- 64-bit integers beyond 2⁵³ and decimals are kept as text.
- Timestamps keep the server's textual form.
- A binary value reads in the grid as its text when it is UTF-8 text (up to 256 bytes), and as hex
  (`0x…`, e.g. BINARY(16) keys, bytea ids) up to 32 bytes. A larger one shows as a size and is
  previewed on demand. Quick Look and the Cell view show it as text when its bytes are text
  (Text / Hex / Base64); editing stays hex.
- JSON viewers (Quick Look, the Cell view, the Details fields) can list the keys sorted. This is
  display only: nothing is staged until you save the sorted text.
