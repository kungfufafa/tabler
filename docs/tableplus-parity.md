# TablePlus parity roadmap (M8)

Tabler copies how TablePlus _behaves_, not how it is built. Everything here comes from TablePlus's
public docs (docs.tableplus.com), its changelog and maintainer comments on its GitHub issue
tracker, plus hands-on use. Nothing comes from inspecting the TablePlus app itself (its EULA
forbids reverse engineering).

Legend: ✅ done · 🟡 partial · ⬜ todo. **B1–B4** = implementation batch.

## B1 — Muscle memory and layout ✅

Done in M8 B1. Every row below is the default behaviour now; the old Tabler keys stay as
alternatives where they do not collide (⌘B, ⌘L, ⌘N, ⌥⇧F, ⇧⌘[ / ⇧⌘]).

| TablePlus behaviour                                                                                                                                                                         | Tabler                                                                 | Status |
| ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------- | ------ |
| Data / Structure toggle in the same tab (bottom segmented control, `⌃⌘[` / `⌃⌘]`)                                                                                                           | `setView` switches the tab in place; both views keep their staged work | ✅     |
| Preview tabs: a single click (or ↑/↓) in the sidebar reuses an _italic_ tab; editing, filtering, staged DDL or a double-click pins it; middle-click / "Open in New Tab" opens a regular tab | same; also "Keep Open" in the tab menu                                 | ✅     |
| `⌘P` Open Anything (tables, tabs, schemas, databases, connections, actions), `⌘K` switch database, `⇧⌘K` switch connection                                                                  | palette modes; ↩ opens in the preview tab, ⌘↩ in a new tab             | ✅     |
| `⌘↩` runs the current statement (or the selection), `⇧⌘↩` runs all; the Run button's default is selectable                                                                                  | same                                                                   | ✅     |
| `⌘I` / `⇧⌘I` beautify / uglify the current statement                                                                                                                                        | same                                                                   | ✅     |
| `⌘I` inserts a row (grid) or a column (structure)                                                                                                                                           | same (`⌘N` kept)                                                       | ✅     |
| `⇧⌘P` preview SQL, `⇧⌘⌫` discard, `⌘Z` / `⇧⌘Z` undo / redo                                                                                                                                  | same (redo of staged row edits)                                        | ✅     |
| `Space` toggles the row-detail panel                                                                                                                                                        | opens the inspector on its Row view                                    | ✅     |
| `⌘[` / `⌘]` previous / next tab, `⌘0` toggle sidebar                                                                                                                                        | same                                                                   | ✅     |
| `⌘E` opens the SQL editor (outside the editor)                                                                                                                                              | same; inside the editor ⌘E is Explain                                  | ✅     |
| `⌥⌘R` reload current tab, `⇧⌘R` reconnect                                                                                                                                                   | same                                                                   | ✅     |
| `⇧⌘F` focuses the sidebar search                                                                                                                                                            | same                                                                   | ✅     |
| `⌥⌘F` column visibility                                                                                                                                                                     | opens the Columns menu                                                 | ✅     |
| ``⌃` `` toggles the query result pane                                                                                                                                                       | same                                                                   | ✅     |
| Set Value ▸ NULL / EMPTY / DEFAULT (also ⌥-click a cell); Quick filter from a cell (=, ≠, IN, NOT IN, IS NULL); "Filter with column"                                                        | same                                                                   | ✅     |
| Filter bar keys: ↩ / ⌘↩ apply, ⌘I duplicate, ⇧⌘I remove, ⎋ close                                                                                                                            | "filter" keymap scope                                                  | ✅     |
| Double-click the blank grid area inserts a row                                                                                                                                              | same                                                                   | ✅     |
| Sidebar "Recent" section (last 5 opened items)                                                                                                                                              | per connection and database                                            | ✅     |
| Console log (`⇧⌘C`) of executed SQL, filterable                                                                                                                                             | All / Data / Structure / Query, with timings, rows and errors          | ✅     |

## UX pass — TablePlus layout and flow ✅

Target and details: [`tableplus-ux.md`](tableplus-ux.md). Done:

- Toolbar with the action control (discard / preview / commit with a count), safe-mode lock,
  Databases and SQL buttons, the **status pill** tinted with the connection colour
  (`TAG │ PostgreSQL 17.6 : name : database : schema.table`), refresh, Open Anything and the
  left / bottom / right panel toggles. The window-wide status bar is gone.
- Workspace **rail** of open connections; **each connection is its own workspace** (the tab bar,
  ⌘1…9, ⌘[ / ⌘], Close Others / All stay inside it).
- macOS-style tab bar under the toolbar with ‹ › tab history.
- Sidebar: Items | History, collapsible sections, query history pane, bottom bar with settings,
  `+` and the schema picker.
- Table tab: no header row; TablePlus bottom bar (`Data | Structure`, `+ Row`, row summary, Filters,
  Columns, Export, `‹ ⚙ ›` with limit / offset); TablePlus filter rows. Structure view shares it.
- Right sidebar **Details** (editable row form, stages like the grid) and Cell views.
- Welcome screen (connection list with driver badges and tag pills, double-click to connect,
  New connection from URL, Copy as URL) and a one-page connection form (driver picker, Tag, colour,
  Over SSH, Advanced).
- SQL tab: the editor bar sits under the editor (cursor position, run summary, row limit,
  Beautify, Explain, Run Current).
- Grid: 22px rows, centred semibold headers, right-aligned numbers, zebra rows, full-row selection,
  `NULL` / `EMPTY` placeholders.
- ⌘S commits straight away (Confirm before commit is off by default; ⇧⌘P previews); ⌘N opens a
  new connection.

## Functional wave (B2–B4) ✅

Built from the public-source checklist in [`research/tableplus-functional-spec.md`](research/tableplus-functional-spec.md)
(363 items, each with its docs / changelog / issue source). Section numbers below refer to it.

### B2 — Safety, connections, session

| TablePlus behaviour (spec §)                                                              | Tabler                                                                                      | Status |
| ----------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------- | ------ |
| Safe mode levels Silent / Alert 1 / Alert 2 / Safe 1 / Safe 2, per connection (§1.12, §9) | toolbar lock menu, alert and password prompts on every send path, default level in Security | ✅     |
| Connection tag, colour, URL import, Copy as URL (§1.2, §1.10)                             | done in the UX pass; Copy as URL also carries `env` / `safeModeLevel`                       | ✅     |
| Import / export connections, optionally with passwords and a file password (§1.9)         | `.tablerconnections`, AES-256-GCM + PBKDF2 when a file password is set                      | ✅     |
| Groups: New Group…, drag in / out, rename, delete, export a group (§1.8)                  | persisted group list; no group icons, no nested groups                                      | 🟡     |
| Password: Store in keychain / Ask every time / No password (§1.2)                         | same (Command Line / IAM: todo)                                                             | 🟡     |
| SSL Key… / Cert… / CA Cert…, Bootstrap commands… (§1.2, §1.6)                             | same; SQLite runs bootstrap SQL too                                                         | ✅     |
| SSH: `~/.ssh/config` fallback, local port, keep-alive (§1.7)                              | HostName / User / Port / IdentityFile from the config                                       | ✅     |
| Keep connection alive (ping every 30 s), query time out 300 s (§1.11)                     | prefs in Settings → Connections                                                             | ✅     |
| Reopen closed workspaces at startup, Auto Save SQL Query Tab (§2.1, §5.1)                 | settings key `session`; staged edits are never restored                                     | ✅     |
| Editable row-detail panel with bulk edit ("Multiple values") (§3.5)                       | same                                                                                        | ✅     |
| Keymap: remap any shortcut (§10.1)                                                        | Settings → Shortcuts, also updates the native menu accelerators                             | ✅     |

### B3 — SQL workflow and objects

| TablePlus behaviour (spec §)                                                                       | Tabler                                                              | Status |
| -------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------- | ------ |
| Queries pane (favorites) with folders and keyword binding (§5.5, §5.2)                             | stored in the app DB (no `.sql` folder on disk, no Show in Finder)  | ✅     |
| History pane, Add to Queries from history (§5.5)                                                   | same                                                                | ✅     |
| Query parameters with the six regex choices (§5.3)                                                 | same, with per-tab remembered values                                | ✅     |
| ⚙ editor menu, Return on Error, multiple carets, split pane, Save selected as (§5.1)               | same                                                                | ✅     |
| Editable query results when the source table and PK are detectable (§5.4)                          | PG table OID, MySQL org_table, SQLite column metadata               | ✅     |
| Results footer, pin result, append all results in one tab (§5.4)                                   | same                                                                | ✅     |
| Functions / procedures / sequences in the sidebar, open and drop (§2.3, §4.5)                      | same; open = definition in a SQL tab                                | ✅     |
| Item menus, Clone…, Truncate / Delete modifiers, Copy Script As (§2.4)                             | same; truncate / delete confirm in a dialog instead of being staged | 🟡     |
| Item Overview with editable comments (§11.2)                                                       | a large dialog, not a tab                                           | ✅     |
| Open Anything over functions and saved queries (§11.1)                                             | same                                                                | ✅     |
| Filter operators incl. Contains / Has prefix / case variants, Any column, Apply All with OR (§3.8) | same; filter defaults in the bar's ⚙                                | ✅     |
| Quick Look editor, Set Value ▸ NOW() / Add a file…, Copy Rows As formats (§3.3, §3.4, §3.6)        | same                                                                | ✅     |
| Structure header, CHECKs, triggers, MySQL table options, view definitions (§4)                     | same; table-level CHECKs are shown too                              | ✅     |

### B4 — Data transfer and admin

| TablePlus behaviour (spec §)                                            | Tabler                                                                          | Status |
| ----------------------------------------------------------------------- | ------------------------------------------------------------------------------- | ------ |
| Export CSV / JSON / SQL options, Gzip, Excel, multi-table export (§6.1) | multi-table export lists the current schema only                                | ✅     |
| Import CSV wizard, Import SQL dump (§6.2)                               | large CSVs commit in chunks (the report says so); `COPY … FROM stdin` → Restore | ✅     |
| Backup / Restore through dump binaries (§7)                             | uses the installed binaries (Homebrew, Postgres.app…) instead of bundled ones   | ✅     |
| Create / rename / drop databases, schemas (§8.1)                        | from ⌘K; rename is PostgreSQL only, as in TablePlus                             | ✅     |
| Process list with Kill (§8.3)                                           | Running Queries (⌘.), Kill Query / Kill Connection                              | ✅     |
| User management (§8.2)                                                  | MySQL and PostgreSQL; passwords never reach history or the console              | ✅     |

### Review pass

Four adversarial reviews (security / safe mode, generated SQL, data integrity, renderer wiring)
found ~45 issues; all were fixed with regression tests. Notable behaviour that came out of it:

- Editable query results are read-only whenever the source row cannot be identified for sure:
  self-joins (PG / SQLite), set operations, views on SQLite, MySQL derived tables and CTEs, `CALL`.
- PG parameter casts drop the type modifier, so over-long values raise an error instead of being
  cut (import, grid edits, filters).
- SQL dumps run on their own short-lived connection; a dump that stops inside its own transaction
  is rolled back and reported; invalid UTF-8 is repaired per byte run, `_binary` blobs stay exact.
- PG exports create sequences, reset them after the data, keep partitioning and add foreign keys
  last (pg_dump order); serial → identity detaches the old sequence.
- Safe mode fails closed, refuses restores / imports on read-only connections, and protects
  Read-only / password changes of a Safe-level connection behind its password.
- MySQL literals follow the session's `NO_BACKSLASH_ESCAPES`; database GRANTs escape `_` / `%`;
  PG role passwords are sent as SCRAM-SHA-256 verifiers.

### Still open

- Command Line (IAM) password mode, SQLCipher, MySQL socket / driver-version picker, SSH keyboard-interactive prompts.
- Nested connection groups with icons; groups with filter rules in the sidebar; per-table column list in the sidebar.
- Staged (⌘S) truncate / drop from the sidebar; Laravel migration / diagram scripts.
- Native-menu entries for the newer editor and grid actions; ⌘F jumping to the last applied filter; the CSV
  delimiter preference for Copy; ~250 KB chunks for copied INSERTs.
- Re-running a query with staged result edits discards them with a toast instead of asking.
- Search and replace inside results; Raise Notice notifications; comment-driven charts.
- Column reordering in the structure editor; PG trigger enable / disable; PG index INCLUDE columns.
- PG partitions recreated with their parent come without their own local indexes / triggers; a sequence
  shared by several tables is created but not reset by exports.
- PG domains over sized types still cut values on import (the cast keeps the domain).
- Not yet tried against real servers: client-certificate SSL, MySQL warnings-as-errors on MariaDB, the
  dedicated dump connection over SSH.

Out of scope for now: metrics board, plugins, the LLM chat panel, MCP server, Touch ID unlock, passcode, iCloud sync.
