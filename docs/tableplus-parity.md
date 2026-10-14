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
| `⇧⌘P` preview SQL, `⇧⌘⌫` discard, `⌘Z` / `⇧⌘Z` undo / redo                                                                                                                                  | same; ⇧⌘P / ⇧⌘⌫ also answer in the SQL editor                          | ✅     |
| `Space` toggles the row-detail panel                                                                                                                                                        | opens the inspector on its Row view                                    | ✅     |
| `⌘[` / `⌘]` previous / next tab, `⌘0` toggle sidebar                                                                                                                                        | same                                                                   | ✅     |
| `⌘E` opens the SQL editor                                                                                                                                                                   | same, also in the SQL editor (Explain: ⌥⌘E)                            | ✅     |
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
- ⌘S commits straight away, staged structure included (Confirm before commit is off by default;
  ⇧⌘P previews; a table without a key and a view / function definition still preview first); ⌘N opens a
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
| Groups: New Group…, drag in / out, rename, delete, export a group (§1.8)                  | persisted group list with nested `A / B / C` paths, 24 icons or an image of your own        | ✅     |
| Password: Store in keychain / Ask every time / No password (§1.2)                         | same (Command Line / IAM: todo)                                                             | 🟡     |
| SSL Key… / Cert… / CA Cert…, Bootstrap commands… (§1.2, §1.6)                             | same; SQLite runs bootstrap SQL too                                                         | ✅     |
| SSH: `~/.ssh/config` fallback, local port, keep-alive (§1.7)                              | HostName / User / Port / IdentityFile from the config (`Include` and `Match` since wave 3)  | ✅     |
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

Editor and commit gap-fix notes (wave 3, editor-commit): the History pane's click only selects and its
double-click opens a new SQL tab without running it (the ⌘H sheet: double-click inserts, ⌘↩ runs); a
saved query's menu has Import… (its `.sql` file through the SQL import wizard) and the folder / +
menus say "Add Files to Queries…"; a new SQL tab starts at No limit (100 / 300 / 500 / 1,000 /
10,000 on offer, footer "N rows (auto limited N rows)") and the bar shows "Line, Column, Location";
an `.sql` file above 8 MB (2 MB gzipped) asks before it loads (first 5 MB as an unlinked copy, the
whole file, or the SQL import wizard), non-UTF-8 files open as Windows-1252 / UTF-16, a file over
512 MB is never read whole; MySQL row edits alone are one transaction and a failure says "All
changes were reverted"; ⌘S commits staged structure at once; a new / edited view, function or
routine is a pending workspace change (counted, ⇧⌘P, ⇧⌘⌫, one commit with the rest); and after a
commit every other open tab of a written table reloads.

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
| Nested connection groups with icons or images; drag groups; export / import groups (§1.8) | `A / B / C` paths, 24 icons or an image of your own (64x64 PNG, up to 24 KB), `.tablerconnections` v2                  | ✅     |
| Sidebar groups with filter rules, per-table column list (§2.3)                            | rules (prefix / suffix / contains / regex / kind / schema) or manual folders; Show Columns                             | ✅     |
| Staged truncate / drop from the sidebar, committed with ⌘S (§2.4, §9)                     | tables, functions / procedures and sequences; they belong to the connection (disconnect, database switch and quit ask) | ✅     |
| Copy Script As ▸ Laravel Migration; ER diagram (§2.4, §11.4, §11.5)                       | migration classes; a native diagram with crow's-foot relations, auto layout, SVG / PNG export                          | ✅     |
| Native menus incl. Navigate, ⌘F to the last filter, CSV default options (§0, §3.8, §10.1) | Edit / View / Navigate menus, Select Next / Previous Pane (⇧⌘] / ⇧⌘[)                                                  | ✅     |
| Search and replace in results, RAISE NOTICE, comment-driven charts (§3.9, §5.1, §5.3)     | find bar (⌘F in results), notice toasts + Messages, Bar / Line / Pie charts                                            | ✅     |
| Re-run with staged result edits asks first (§5.4)                                         | "Discard N uncommitted result changes and run?"                                                                        | ✅     |
| PG trigger enable / disable, index INCLUDE columns (§4.4, §4.5)                           | staged; INCLUDE editable for new indexes                                                                               | ✅     |
| Export: partitions with their own indexes / triggers, shared sequences (§6.1)             | pg_dump-like script                                                                                                    | ✅     |

Real-server checks (`docs/verify-servers.md`, compose services `pg-ssl` / `mysql-ssl`): client-certificate
SSL works with PEM keys (PKCS#8 / PKCS#1 / SEC1, leaf first); MySQL VERIFY-CA checks the chain but not the
host name (a patch of the vendored mysql_async since wave 3; it used to check the name too); MySQL / MariaDB warnings-as-errors on
CSV import and the dump import over SSH work. A second review pass fixed 23 more issues (MySQL IAM
needing the cleartext plugin, IAM tokens in pools, password commands in imported files, diagram
partitions, INCLUDE quoting, the staged-ops commit chain, chart tick hangs, find in hidden columns…).

## Wave 3 — everything the gap audit found ✅

A line-by-line audit of the 363-item checklist (228 done, 77 partial, 63 missing, 8 not applicable
after wave 2) was worked off in three groups. A second, independent line-by-line audit of the whole
checklist (10 sections, about 700 items checked, every reported gap re-checked by a skeptic) then found 75
further gaps, mostly details; they are the "last audit" sections below. Highlights, by spec section:

| TablePlus behaviour (spec §)                                                                                                                                                | Tabler                                                                                                                                                                                                                                                                                                                                                              | Status |
| --------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------ |
| Multiple windows, Move Tab to New Window, ⌘N, Open in New Window (§0, §1.1, §2.1)                                                                                           | each window connects again; moving a tab hands its payload over; quit waits for every window; the oldest window owns the saved session                                                                                                                                                                                                                              | ✅     |
| Each database in its own workspace, DB icon bar with reorder and name toggles (§2.1, §2.2, §8.1)                                                                            | ⌘K ↩ opens a new workspace (Open in Current Workspace keeps the old behaviour); rail icons persist their order                                                                                                                                                                                                                                                      | ✅     |
| Split workspace (§2.1)                                                                                                                                                      | ⇧⌘D (not in the SQL editor or a cell editor): two tab groups, drag tabs across; Set DEFAULT is ⌥⌘D                                                                                                                                                                                                                                                                  | ✅     |
| Item Overview as a tab (§11.2); generic tool tabs                                                                                                                           | `tool` tab kind with a registry (Overview, Metrics Board, SQL Viewer)                                                                                                                                                                                                                                                                                               | ✅     |
| SQLCipher, `.sqliterc`, custom tags, Ver. / SSL labels, CLEARTEXT, LOAD DATA LOCAL INFILE (§1.1–1.6)                                                                        | key stored like a password; compatibility 1–4; custom tags with colours; per-file Allow / Deny for LOCAL INFILE                                                                                                                                                                                                                                                     | ✅     |
| PG SSL negotiation Direct, channel binding, extra parameters; bootstrap CLI script (§1.3, §1.6)                                                                             | libpq-style `key=value` list; login-shell script with a 60 s limit run before connecting                                                                                                                                                                                                                                                                            | ✅     |
| Default connection at launch; View Using Encoding (§10.1, §11.7)                                                                                                            | pref + startup hook; per-connection encoding and a live switch with a tab reload                                                                                                                                                                                                                                                                                    | ✅     |
| SSH ProxyJump / ProxyCommand / IdentityAgent, password storage modes, debug log (§1.7, §0)                                                                                  | chain of `direct-tcpip` hops; the forwarded port only serves this process; Help ▸ Enable SSH Debug Log                                                                                                                                                                                                                                                              | ✅     |
| SQLite over SSH; MySQL socket over SSH (§1.5, §1.4)                                                                                                                         | SFTP cache with conflict check on upload; `direct-streamlocal` forwarding                                                                                                                                                                                                                                                                                           | ✅     |
| `open -a` URLs and every URL parameter, `+ssh` URLs, file associations (§1.5, §1.10, §11.7)                                                                                 | Info.plist schemes + `RunEvent::Opened`; extra libpq parameters (`jit=off`…), a custom `statusColor`, `windowMode=isolated` with the row filter and `lazyload` for saved connections all work; a raw SQL condition asks first (read-only connections apply it); a double-clicked encrypted SQLite file asks for its key; temporary profiles are deleted when closed | ✅     |
| Touch ID, passcode with auto lock, Copy as URL with password (§1.1, §1.12, §10.1)                                                                                           | LocalAuthentication; salted PBKDF2 hash; lock screen on idle / sleep / ⌃⌘L; confirm + password before the URL is copied                                                                                                                                                                                                                                             | ✅     |
| Customize Toolbar, status-pill details and recolour, auto-reconnect, console highlighting (§0, §1.8, §1.11, §11.3)                                                          | drag items, icon / text display; TLS / SSH segment; reconnect on focus or network return                                                                                                                                                                                                                                                                            | ✅     |
| Grid: sort cycle, per-table layout, rectangular selection, NULL vs EMPTY copy, PK-preserving paste, hex editing, Quick Look detach, geometry (§3, §10.1)                    | WKB / EWKB / MySQL spatial to WKT, hover preview, offline Map view                                                                                                                                                                                                                                                                                                  | ✅     |
| Foreign-key arrow: several keys ask which, composite and binary keys jump right (§3.7, §3.4)                                                                                | a menu of `table (column)` for the arrow, ⌘J and Generate JOIN; every key column filters; blobs by hex                                                                                                                                                                                                                                                              | ✅     |
| Blob decoded to text when possible; MySQL binary shows hex (§3.6 b272, §3.4 b270)                                                                                           | cell: text up to 256 B or `0x…` hex up to 32 B; Quick Look / Cell view: Text, Hex, Base64; Details: first 40 characters                                                                                                                                                                                                                                             | ✅     |
| JSON viewer with sorted keys (§3.6 b117, b210)                                                                                                                              | Sort keys in Quick Look (Json, Tree), the Cell view and Details; display only, literals and layout kept                                                                                                                                                                                                                                                             | ✅     |
| Auto convert empty Numeric, UUID, JSON or Array to NULL, in the Details fields too (§3.4, §10.1)                                                                            | follows the preference like the grid editor (off: the same error; NOT NULL keeps its own)                                                                                                                                                                                                                                                                           | ✅     |
| Multiple carets and syntax highlighting in fields (§3.5 b300)                                                                                                               | every text-like Details field (text, JSON, XML, enum, set, array, spatial; one line or many) is a compact Quick Look CodeMirror: ⌘-click carets, ⌃⇧↑ / ⌃⇧↓, ⌘F, JSON / XML colours; its undo history restarts with each row / value                                                                                                                                 | ✅     |
| ⌘S in a cell editor commits; ⇧⌘C copies the cell value in a grid (§10.2, §3.3 b446)                                                                                         | editors stage the edit, then commit the tab (invalid text stays open); outside a grid ⇧⌘C is the console                                                                                                                                                                                                                                                            | ✅     |
| Exact count with a custom estimate threshold; "N of ~M rows selected" in the footer (§3.1)                                                                                  | the exact count lands under the shown count's key; the selection reads against the table total                                                                                                                                                                                                                                                                      | ✅     |
| Filter bar: ⌘I clones column + operator, SQL shows the generated query, ⌘↑ / ⌘↓ between bar and data (§3.8, §10.2)                                                          | blank value ready to type; full SELECT highlighted with Copy and a WHERE-only view; ⌘↑ on the first row / ⌘↓ on the last filter                                                                                                                                                                                                                                     | ✅     |
| Export result… in a result grid's right-click menu; ⌘V pastes CSV / TSV into the grid (§5.4, §6.2)                                                                          | Export Result… for the clicked result (pinned ones too); comma / semicolon CSV with RFC 4180 quotes, whole rows become new rows                                                                                                                                                                                                                                     | ✅     |
| Editor: function completion, `alias.*`, JOIN suggestions, autocomplete options, `.sql.gz`, history search / retention, queries as `.sql` files, Locations (§3.7, §5, §10.1) | movable data folder with a safe fallback; one `.sql` file per saved query                                                                                                                                                                                                                                                                                           | ✅     |
| HTML export, JSON import, multi-database export, file encodings, notifications, Homebrew help for dump tools (§6, §7, §11.7)                                                | UTF-8 / BOM / UTF-16 / Windows-1252 / Latin-1; macOS notifications when Tabler is in the background                                                                                                                                                                                                                                                                 | ✅     |
| One commit for the whole workspace; server-side NOW(); routine ⌘S; column order and bulk edit (§9, §3.4, §4.5, §4.1, §4.2)                                                  | `apply_workspace_changes` in one transaction on PG / SQLite; whitelisted server expressions; DROP + CREATE with a preview                                                                                                                                                                                                                                           | ✅     |
| Lazy-load schema items, search menu, direct-stage ⌥⌫ / ⇧⌫ / ⌘⌫ (§1.3, §2.3, §2.4, §11.1)                                                                                    | schema nodes load on expand; ⌘P over loaded schemas plus "Load all schemas"; Undo toast after staging                                                                                                                                                                                                                                                               | ✅     |
| File ▸ Open Recent, New Workspace, ⌘R vs ⌥⌘R, ⌘↩ from non-SQL tabs, SQL Viewer, remappable comment keys (§0, §5.1, §10)                                                     | all keys listed in `docs/shortcuts.md`                                                                                                                                                                                                                                                                                                                              | ✅     |
| Metrics Board (§8.4)                                                                                                                                                        | boards of 8 object types with a 12-column canvas, variables, wiring, refresh modes, `.tablerboard` import / export                                                                                                                                                                                                                                                  | ✅     |
| Plugins (§0, §5.6, §11.4)                                                                                                                                                   | Web Worker sandbox, `context.*` API, ⌘L manager, built-ins: SQL Format, Open URL, Copy as UPDATE, Dummy Data, UUID                                                                                                                                                                                                                                                  | ✅     |
| LLM assistant (§3.5, §10.1, §11.6)                                                                                                                                          | right-sidebar tab; Anthropic / OpenAI-compatible / Ollama; only structure is ever sent                                                                                                                                                                                                                                                                              | ✅     |
| MCP server (§11.6)                                                                                                                                                          | `tabler --mcp`; read-only by default; one-click install for Claude Code, Codex, Cursor, VS Code (+ Insiders)                                                                                                                                                                                                                                                        | ✅     |
| Fonts & Colors, themes, change-marker colours (§9, §10.1)                                                                                                                   | Light / Dark / Solarized + custom `.tablertheme` files; editor and grid fonts; 12 syntax colours; AA contrast tests                                                                                                                                                                                                                                                 | ✅     |
| ALTERs of one table grouped into one statement for MySQL / PG; on an error "All changes were reverted" (§4.6 b388)                                                          | `ALTER TABLE t a, b, c` in op order (a clause the server would run out of order, a rename, comment, option or other table starts a new one); a MySQL plan of one statement is all or nothing, the Structure tab says "All changes were reverted"; several statements keep "stopped partway"                                                                         | ✅     |
| Double-click a blank row adds a column / index / trigger; Delete / right-click Delete on several columns (§4.2, §4.4, §4.5, §4.1)                                           | a blank row ends each grid (also "No …" and the empty space); a click on a row selects and focuses it, Delete / ⌘⌫ / **Delete N columns** act on the selection in one undo step                                                                                                                                                                                     | ✅     |
| MySQL index column length; ON UPDATE CURRENT_TIMESTAMP precision (§4.4 b228, §4.2)                                                                                          | a `len` field per char / text / blob chip (`name(10)`; TEXT / BLOB need one: the Preview refuses the plan); `datetime(3)` → `CURRENT_TIMESTAMP(3)` for ON UPDATE and DEFAULT, re-emitted when the type's precision changes                                                                                                                                          | ✅     |
| ⌘↩ from a structure tab; ⇧⌘[ / ⇧⌘] with a split editor or split workspace (§10.2)                                                                                           | opens the SQL editor; the panes cycle through both groups of a split workspace and both editors of a split SQL tab                                                                                                                                                                                                                                                  | ✅     |

Export (§5.4, §6.1, §10.1, §10.2), the last audit's fixes:

- Several results at once: a run with two or more finished results offers **This result / All results
  (N)** in the export dialog. Excel writes one workbook with a sheet per result (`Result 1`…), HTML one
  page with a heading per result, SQL one script, CSV / JSON a file per result (`<name>-1.csv`…) in a
  folder (`export_results`; the rows are the renderer's, nothing runs on the server). Results cut at the
  row limit or stopped early keep their warnings; a cut binary preview is refused as for one result.
- A result of an unknown table (a join, a computed column) exports as SQL into `untitled` without
  asking for a name (the field can still be typed in; the backend also writes `untitled` for an empty
  name).
- The file follows the grid's column order, also when columns were only dragged (the checkbox reads
  "Columns in grid order"; hidden columns still read "Only visible columns").
- "Export current page" from page 2 on is a slice of the table query (`ExportSource::Table.offset`,
  ordered by the sort then the primary key, else `rowid` / every selected column): hidden columns and
  generated columns work on every page.
- INSERT statements end at the row batch or at about 250 KB, whichever comes first (a row never
  splits). Scripts set the exporting session's time zone first (`SET TIME_ZONE` / `SET TIME ZONE`,
  restored at the end; MySQL `SYSTEM` becomes its `+HH:MM` offset; SQLite has none), so `TIMESTAMP`
  literals load as the same instants in a session of another zone.
- File encoding follows Connection ▸ View Using Encoding: `latin1` / `cp1252` / `UTF-16` connections
  preselect Latin-1 / Windows-1252 / UTF-16 LE ("Follows View Using Encoding (…)"), unless the select
  was changed in the dialog; UTF-8 connections keep the remembered choice. The text is still Unicode.
- CSV quote **Space** and a **custom decimal separator** (Other…, one ASCII character that is not a
  digit, a quote or the delimiter) in the export dialog, Settings ▸ General ▸ CSV default options,
  Copy as CSV and the CSV import (its quote select accepts a space too).
- ⇧⌘E / File ▸ Export… is the multi-table export ("Export tables") with the active table checked; the
  table tab's own Export buttons keep exporting that table's filtered rows.

Connections and Help (§0, §1.1, §1.4, §1.5, §1.7, §1.10, §1.11), the last audit's fixes:

- `~/.ssh/config`: `Include` (globs, `~/`, nesting up to 16, cycles skipped) and `Match all` / `host` /
  `originalhost` / `user` / `localuser` are evaluated; `exec` and the other criteria never match.
- MySQL Ver.: Old MySQL (`secure_auth` off for `mysql_old_password`, TLS 1.0 / 1.1 through Secure Transport,
  tested against a TLS 1.1-only fake server) and SingleStore DB (`driverVersion=3`); VERIFY-CA checks the chain
  but not the name (vendored `mysql_async`); socket auto-detection covers Intel Homebrew, MAMP, XAMPP and
  MacPorts; CLEARTEXT_PLUGIN works through an SSH tunnel; the connect timeout defaults to 60 s.
- New-connection sheet: Import from URL…; quitting closes every SQLite file (no `-wal` / `-shm` left);
  Help ▸ Changelog and Report a Bug / Send Feedback….

Not applicable to Tabler: the free-tier limits, TablePlus's update channel and beta updates, AppCenter crash
reporting, the DBngin companion app, and licensing.

### Still open

Nothing from the checklist is left unimplemented. Known limits, all documented in the feature's own
section of the README or `docs/shortcuts.md`:

- One MySQL driver serves every "Ver." choice: Old MySQL adds the `mysql_old_password` login and TLS 1.0 / 1.1
  (macOS Secure Transport, not with a client certificate; see the README's connection details), SingleStore DB
  uses the MariaDB-style SSL list; `driverVersion` / `tLSMode` URL values are mapped by guess because TablePlus
  does not document them.
- The assistant has no GitHub Copilot or Codex CLI providers and ignores system HTTP proxies.
- SpatiaLite / GeoPackage blobs stay binary.
- Not yet tried against real servers: SSH 2FA against a PAM / OTP bastion, IAM against RDS, PG
  `sslnegotiation=direct` against a live TLS server, PostGIS (parser unit-tested only).

Deliberate differences:

- TablePlus lets you pick the libssh version of the SSH tunnel; Tabler has one pure-Rust SSH client (russh),
  so there is nothing to choose.
- The official `pg_dump` / `mysqldump` binaries are not bundled: Backup and Restore run the ones installed
  on the Mac (version picker over the installed ones) and offer a Homebrew install helper when none is found.
- Plugins run in a Web Worker. Tauri sends the Content-Security-Policy header on HTML responses only, so the
  worker script has none and `new Function` works in it, while the page itself stays blocked from `eval` and
  a `blob:` worker cannot start. Checked in WebKit against the production CSP (`dev/csp-worker-check`); the
  sandbox removes the network APIs itself, since the CSP does not reach the worker.

Out of scope: iCloud sync (Locations can point at an iCloud Drive folder instead).

## Wave 3 — runtime and console ✅

| TablePlus behaviour (spec §)                                                           | Tabler                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            | Status |
| -------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------ |
| Force reconnect `⇧⌘R`, also Connection ▸ Reconnect (§1.11, §10.2 b136)                 | skips the ping and opens a new session from the profile, keeps tabs, staged edits and the current database, releases the old session and says when a running query was stopped; a failed attempt leaves the old session untouched; registered where it is always mounted (works in the Connection Manager). The banner button and auto-reconnect keep the ping-first path                                                                                                                                                                                                                                                                                                                                                                                                                         | ✅     |
| Query time out, default 300 s: TablePlus kills queries with no response (§1.11, §10.1) | applies to every query the app runs: editor runs, table page loads / filters / sorts, row counts, cell fetches and Explain; the backend reads the preference, cancels the statement on the server (`query_cancellable` on every driver) and fails with `QUERY_TIMEOUT` ("Query timed out after N s" + the hint); 0 = no limit                                                                                                                                                                                                                                                                                                                                                                                                                                                                     | ✅     |
| Theme Light / Dark / Auto follows the whole app (§10.1)                                | the choice also sets the native appearance in every window (`setTheme`: menu bar, file panels, message boxes, title bar); Auto hands control back to macOS; custom themes use their own light / dark base                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         | ✅     |
| SQL editor indent default "4 spaces" (§10.1)                                           | Tab size defaults to 4 (2 stays a choice)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         | ✅     |
| Left sidebar font (§10.1 b432)                                                         | Settings ▸ Appearance ▸ Left sidebar: font family (empty = the UI font) next to the size; included in Reset Fonts & Colors                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        | ✅     |
| Console log records executed queries; Meta / Data / All filter (§11.3 b320)            | the sessions record the statements they really send (catalog SELECTs, `COUNT(*)`, page loads, cell fetches, exports…, parameters inlined, with timing, rows and errors); the renderer logs them after each call, once; the Explain command (the wrapped `EXPLAIN`, and the `BEGIN` / `ROLLBACK` around an Analyze) and a SQL dump import (every statement, newest 500 kept) are recorded too (`run_statement_traced`; the SQL editor's own runs are logged by the renderer from the run's events, not twice); statements that carry a password (user management) are recorded with the literal masked and an error scrubbed of it (`drivers/redact.rs`), and the renderer masks the call's own password as a second lock; a call that ran no SQL (cached structure) is still a `-- …` description | ✅     |
