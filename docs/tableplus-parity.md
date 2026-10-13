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

## Wave 2 — the "Still open" list ✅

| TablePlus behaviour (spec §)                                                              | Tabler                                                                                                                 | Status |
| ----------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------- | ------ |
| Password storage "Command Line (IAM authentication)" (§1.2, §1.3)                         | login-shell command; token cached 10 min and refreshed for new pool connections; MySQL needs TLS                       | ✅     |
| MySQL "Use socket" (§1.4)                                                                 | auto-detects the usual socket paths; backups use the socket too                                                        | ✅     |
| SSH keyboard-interactive / 2FA prompts (§1.7)                                             | the server's questions in a dialog; works without a saved SSH password                                                 | ✅     |
| SQLCipher (§1.5)                                                                          | a clear "encrypted database" error (no SQLCipher build yet)                                                            | 🟡     |
| Nested connection groups with icons; drag groups; export / import groups (§1.8)           | `A / B / C` paths, 24 icons, `.tablerconnections` v2                                                                   | ✅     |
| Sidebar groups with filter rules, per-table column list (§2.3)                            | rules (prefix / suffix / contains / regex / kind / schema) or manual folders; Show Columns                             | ✅     |
| Staged truncate / drop from the sidebar, committed with ⌘S (§2.4, §9)                     | tables, functions / procedures and sequences; they belong to the connection (disconnect, database switch and quit ask) | ✅     |
| Copy Script As ▸ Laravel Migration; ER diagram (§2.4, §11.4, §11.5)                       | migration classes; a native diagram with crow's-foot relations, auto layout, SVG / PNG export                          | ✅     |
| Native menus incl. Navigate, ⌘F to the last filter, CSV default options (§0, §3.8, §10.1) | Edit / View / Navigate menus, Select Next / Previous Pane (⇧⌘] / ⇧⌘[)                                                  | ✅     |
| Search and replace in results, RAISE NOTICE, comment-driven charts (§3.9, §5.1, §5.3)     | find bar (⌘F in results), notice toasts + Messages, Bar / Line / Pie charts                                            | ✅     |
| Re-run with staged result edits asks first (§5.4)                                         | "Discard N uncommitted result changes and run?"                                                                        | ✅     |
| PG trigger enable / disable, index INCLUDE columns (§4.4, §4.5)                           | staged; INCLUDE editable for new indexes                                                                               | ✅     |
| Export: partitions with their own indexes / triggers, shared sequences (§6.1)             | pg_dump-like script                                                                                                    | ✅     |

Real-server checks (`docs/verify-servers.md`, compose services `pg-ssl` / `mysql-ssl`): client-certificate
SSL works with PEM keys (PKCS#8 / PKCS#1 / SEC1, leaf first); MySQL VERIFY-CA cannot skip the host-name
check (mysql_async limitation — use VERIFY-IDENTITY or REQUIRED); MySQL / MariaDB warnings-as-errors on
CSV import and the dump import over SSH work. A second review pass fixed 23 more issues (MySQL IAM
needing the cleartext plugin, IAM tokens in pools, password commands in imported files, diagram
partitions, INCLUDE quoting, the staged-ops commit chain, chart tick hangs, find in hidden columns…).

### Still open

- SQLCipher databases (needs an OpenSSL-backed SQLite build).
- Pinned results and Item Overview are dialogs or tabs of their own rather than TablePlus's exact
  placement; group images (TablePlus) are icons here.
- Not yet tried against real servers: SSH 2FA against a real PAM / OTP bastion, IAM against RDS.

Out of scope for now: metrics board, plugins, the LLM chat panel, MCP server, Touch ID unlock, passcode, iCloud sync.
