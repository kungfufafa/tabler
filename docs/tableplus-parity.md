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

## B2 — Safety, connections, session

| TablePlus behaviour                                                                   | Tabler before M8              | Batch |
| ------------------------------------------------------------------------------------- | ----------------------------- | ----- |
| Connection tag: Local / Testing / Development / Staging / Production                  | ✅ done in the UX pass        | B2    |
| Safe mode levels: Silent, Alert (all / non-SELECT), Safe (password, all / non-SELECT) | read-only switch only         | B2    |
| New connection from URL, Copy as URL                                                  | ✅ done in the UX pass        | B2    |
| Import / export connections (optionally without passwords)                            | none                          | B2    |
| Reopen the last workspace's tabs on connect                                           | tabs are not restored         | B2    |
| Editable row-detail panel (and bulk edit of many rows)                                | ✅ editable (bulk edit: todo) | B2    |
| Client SSL key/cert, startup ("bootstrap") SQL in the form                            | CA only, no startup SQL field | B2    |

## B3 — SQL workflow

| TablePlus behaviour                                                              | Tabler before M8        | Batch |
| -------------------------------------------------------------------------------- | ----------------------- | ----- |
| Favorites (saved queries) with a keyword that expands like a snippet             | none                    | B3    |
| History per connection; double-click opens it in a new tab                       | ✅ sidebar History pane | B3    |
| Functions / procedures in the sidebar, editable definitions                      | tables and views only   | B3    |
| Option to continue a run after an error                                          | always stops            | B3    |
| Editable query results (staged like the table grid)                              | read-only               | B3    |
| Filter operators: contains, not contains, has prefix/suffix, ILIKE, "any column" | LIKE-based only         | B3    |

## B4 — Data transfer and admin

| TablePlus behaviour                                          | Tabler before M8         | Batch |
| ------------------------------------------------------------ | ------------------------ | ----- |
| Import from CSV / SQL dump (wizard, create table if missing) | none                     | B4    |
| Backup / restore through pg_dump / mysqldump                 | none                     | B4    |
| Create / drop / rename a database from the database switcher | switch only              | B4    |
| Process list with Kill                                       | none                     | B4    |
| Multi-table export, Excel                                    | one source, CSV/JSON/SQL | B4    |

Out of scope for now: metrics board, user management, plugins, the LLM chat panel, iCloud sync.
