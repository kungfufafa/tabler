# TablePlus UX target (M8 — UX pass)

How Tabler's screens are laid out to match TablePlus's flow. Sources: the current workspace
screenshots on tableplus.com (light and dark), the SQL editor and "Open Anything" screenshots, and
docs.tableplus.com (toolbar, sidebars, filters, code review). Behaviour only — no TablePlus assets
(icons, illustrations, logos) are copied.

## 1. Window anatomy (workspace)

```
┌──────────────────────────────────────────────────────────────────────────────────────────────┐
│ ●●●  [⊟] [ⓧ 👁 ≡↑] [🔒] [🗄 SQL]  ┃ LOCAL │ PostgreSQL 17.6 : tabler_dev : public.users  ┃ [⟳] [⌕] [▢▢▢] │  toolbar (40px)
├──┬───────────────┬─────────────────────────────────────────────────────────┬─────────────────┤
│  │[Items|History]│ ‹ ›  users │ posts │ Query 1                       [+] │ Search field…  │  tab bar (28px)
│▣ │ Search…   [≡] ├─────────────────────────────────────────────────────────┤ id        int8 │
│  │ ▸ Recent      │ (filter bar, when shown)                                │ [ 5          ] │
│▣ │ ▾ Tables      │ grid                                                    │ email     text │
│  │   users   50  │                                                         │ [ eko@…      ] │
│  │   posts  200  │                                                         │ …              │
│  │ ▸ Views       │                                                         │                │
│  ├───────────────┼─────────────────────────────────────────────────────────┤                │
│  │ ⚙  [+⌄] [public⌄]│ [Data|Structure] [+ Row]  1–200 of 1,000  [Filters][Columns] ‹ ⚙ › │                │  bottom bar (32px)
└──┴───────────────┴─────────────────────────────────────────────────────────┴─────────────────┘
 rail  left sidebar   main                                                     right sidebar
```

There is **no window-wide status bar** at the bottom any more: what it showed moves to the toolbar
status pill (connection), the bottom bar (rows, timing) and the toolbar action control (pending
changes).

### Toolbar (one row, in the macOS title bar)

Left → right, after the traffic-light inset:

1. **Toggle left sidebar** button.
2. **Action control** — a segmented group of three icon buttons: **Discard** (ⓧ, ⇧⌘⌫),
   **Preview SQL** (eye, ⇧⌘P), **Commit** (⌘S). Disabled when the visible tab has nothing staged;
   when it has, Commit shows the count (e.g. a small badge "3").
3. **Safe mode** lock button: locked icon when the connection is read-only; a click opens a popover
   with the mode and "Edit Connection…" (safe-mode levels come with B2).
4. **Databases** button (⌘K) and **SQL** button (new query, ⌘T).
5. **Status pill** — centred, flexible width (min 280px, max ~640px), rounded, tinted with the
   connection's colour (light tint background, darker text), monospace text:
   `TAG │ PostgreSQL 17.6 : SSH : <connection name> : <database> : <schema>.<table or "SQL Query">`.
   The tag segment is a small uppercase label (LOCAL, TESTING, DEV, STAGING, PRODUCTION) only when
   the profile has a tag. Clicking the pill opens the connection switcher (⇧⌘K). When no connection
   is open the pill reads "No connection" (muted). While reconnecting / lost, the pill shows that
   state (warning tint) — replaces the old banner's prominence, but keep the banner too.
6. Right side: **Refresh** (⌘R), **Open Anything** (⌘P, magnifier icon), and the **panel toggles**:
   left sidebar, bottom console (⇧⌘C), right sidebar (⇧⌘I) — three small "panel" icons in one
   segmented group, highlighted when the panel is open.
   Everything is `data-tauri-drag-region` where empty. In the launcher (no workspace) the toolbar
   shows only the traffic-light inset, the title "Tabler" and nothing else.

**Windows** follows TablePlus for Windows instead: the window has no native title bar and no menu
bar (the backend turns the decorations off and hides the menu, whose accelerators keep working). The
toolbar is the title bar: a **⋮** button after the sidebar toggle pops the whole app menu up (File,
Edit, View, Connection, Plugins, Navigate, Window, Help), and **minimize / maximize-restore / close**
sit flush at its right end (close turns red on hover). The empty toolbar drags the window and a
double-click maximizes it. The launcher keeps a bare toolbar with just ⋮ and the window buttons.
Linux keeps its native title and menu bars.

### Workspace rail (far left, 52px)

Shown whenever **a connection is open** (so `+` is always one click away). One button per open
workspace: a rounded square tinted with the connection colour holding the database's initials
(TablePlus's database icons; the connection's without a database), the database and connection
names underneath (10px / 9px, truncated; the right-click menu hides either), the active one
highlighted. Tabler addition: a small badge in the tag's colour under the square for every tag but
`local` (PROD, STG, DEV, TEST, a custom tag's first four letters) and a lock at its corner for a
read-only connection, so two workspaces of the same database never differ by colour alone. Click
switches the active connection. A `+` at the bottom opens the connection manager. Right-click:
Show Database / Connection Name, Open in New Window, Disconnect.

### Tab bar (below the toolbar, 28px)

Native macOS document tabs: `‹ ›` back/forward buttons at the left (navigate the tab history:
previously active tabs), then **equal-width tabs that fill the row** (min 110px; one tab spans the
bar; titles centred, truncated), the active tab uses the window background, inactive ones a slightly
darker surface, separated by 1px borders; a close × appears on hover at the tab's left side
(macOS style); preview tabs keep the italic title. `+` at the right opens a new query tab.
Always visible in the workspace (with one tab it shows that single tab full width).
Drag-to-reorder, middle-click close, context menu, dirty dot — keep everything that exists.

### Left sidebar

- Top: segmented control **Items | History** (Queries/favourites come with B3).
- **Items**: search field "Search for item…" (⇧⌘F) with a small filter-settings button; then a
  tree with **collapsible section headers** (disclosure triangle, bold 11px, count): Recent,
  Pinned, Tables, Views, Materialized views — collapse state remembered per section. Rows are 22px,
  table icon + name; the row estimate shows muted on the right.
- **History**: the query history list (today in the SQL editor's ⌘H sheet) as a sidebar list:
  search, newest first, each entry = first line of SQL (monospace, truncated) + relative time;
  a click only selects the entry, a double-click (or ↩) opens it in a new SQL tab and never runs
  it (running is Run in New Tab in the menu or ⌘↩ on the selected row). ⌘H still opens the sheet.
- **Bottom bar** (32px, same height as the main bottom bar): a settings (⚙) menu button (show
  Recent / show row counts toggles), a `+` menu (New Table…, New Query), and the **schema picker**
  (PostgreSQL schemas / SQLite attached DBs) as a compact dropdown. The database is switched with
  ⌘K / the toolbar Databases button; no database dropdown in the sidebar any more.
- The old sidebar header (connection switcher + database + schema dropdowns) is removed: the pill
  and the rail replace it.

### Main area — table tab

- No header/toolbar row above the grid any more (the tab title + pill say where you are).
- Filter bar (⌘F) at the top, TablePlus style: each row `[✓] [column ⌄] [operator ⌄] [value…]
[Apply] [−] [+]`; under the rows a strip: `Export` `SQL` … hints … `Clear` `Apply All`. `SQL` shows
  the whole generated query (highlighted, with Copy; "WHERE only" narrows it); ⌘I and `+` add a
  blank filter on the same column and operator; ⌘↑ / ⌘↓ link the bar and the grid.
- Grid (see §3).
- **Bottom bar** (32px): `[Data | Structure]` segmented · `[+ Row]` button · centre: the row
  summary — "1–200 of 1,000 rows" or, when rows are selected, "3 of 1,000 rows selected" (the
  table's row count, with TablePlus's "~" when it is an estimate: "15 of ~999,985 rows selected";
  the rows of the page while the count is pending) — plus the query time muted ("4 ms") · right:
  `[Filters]` toggle (highlighted when the bar is open or filters are applied, shows the count),
  `[Columns]` (the visibility menu), Export (icon; this table's filtered rows), and the pagination
  `‹  ⚙  ›` where ⚙ opens a popover with **Limit** (page size) and **Offset** fields and a "Go"
  button, plus First/Last page and the exact-count action.
- Structure view: same bottom bar with `[Data | Structure]` on the left and its own actions.

### Right sidebar — row details (⇧⌘I, Space)

Replaces the "Inspector" header + Cell/Row tabs. Top: segmented **Details | Cell** (Details is
default). **Details**: a search field "Search for field…", then one block per column: a header line
with the column name (11px, muted) and its type right-aligned (10px, muted), and below it the
value in an input-like box (monospace 12px). Multi-line / JSON values show a taller box. NULL shows
a muted `NULL` placeholder, empty string a muted `EMPTY` placeholder. On a table tab with an
editable table the boxes are **editable**: typing stages an edit exactly like the grid (`edit`
callback on `InspectedRow`), Enter/blur commits the field, a per-field menu offers Set NULL / EMPTY
/ DEFAULT. Read-only otherwise. **Cell**: today's single-cell view (hex dump, load full value).
With nothing selected: "No row selected" centred.

### Bottom console

The console log panel (⇧⌘C) docks under the main area across main + right sidebar width, as now.

## 2. Flows

1. **Launch → Welcome**: the launcher becomes a TablePlus-like welcome screen: a left column
   (240px) with the app name, version and three actions (Create a new connection…, New connection
   from URL…, Import… disabled until B2), and the connection **list** on the right: search field
   (⌘F) on top, groups as collapsible headers, each connection a 44px row: a driver badge (rounded
   square tinted with the colour, label PG/MY/SQ), name (13px semibold) + tag pill (lowercase,
   coloured: local=green, testing=blue, development=purple, staging=orange, production=red), the
   summary below (`host : port : user : database`, muted, monospace 11px), last-connected time on
   the right. Single click selects, **double-click / Enter connects**, right-click: Connect / Edit…
   / Duplicate / Copy as URL / Delete. `⌘N` opens the new-connection form.
2. **New / edit connection form**: one page, two columns of label + field like TablePlus:
   Name · Tag ⌄ / Status color (swatches) / Host · Port / User · Password (Keychain) / Database /
   SSL mode ⌄ (+ CA file) ; buttons at the bottom: **Over SSH** (toggle, reveals the SSH block
   inline), Advanced… (disclosure with read-only, timezone, timeout, SQLite FK), and on the right
   **Test**, **Save**, **Connect**. The driver is picked first (a compact driver grid: PostgreSQL /
   MySQL·MariaDB / SQLite) for a new connection, changeable later via a select.
3. **Connected**: the workspace shows the empty main area with a centred hint ("Select an item in
   the sidebar, or press ⌘P to open anything") until a table opens.
4. **Browse → edit → commit**: single click = preview tab; edits colour the cells; the toolbar
   action control lights up with the count; **⌘S commits straight away** (Confirm before commit is
   now off by default; ⇧⌘P previews); ⇧⌘⌫ discards.
5. **Query**: the SQL tab's own toolbar moves **below the editor**: left "Line 4, Column 13, Location 40";
   right: row limit select ("No limit / 100 / 300 / 500 / 1,000 / 10,000"), **Beautify ⌘I**, **Run Current ⌘↩ ⌄**
   (the existing split button). Results under it.

## 3. Grid look

- Row height 22px (compact), cell font = UI font for text, **monospace for numbers, dates, ids**
  is not needed — keep one font but use `tabular-nums`; numbers right-aligned.
- Header: column name centred, semibold 12px; type in the tooltip (as now).
- Alternating row background (subtle), full-row highlight for selected rows (accent tint), the
  focused cell outlined.
- `NULL` muted italic, empty string shows muted `EMPTY`.

## 4. Audit round 2 (hands-on, after the first install)

Walking every flow like a TablePlus user turned up these differences. Each is fixed in the "UX
audit" pass; the TablePlus behaviour is the target.

### Grid

1. **Row selection.** TablePlus selects rows: clicking a cell highlights the whole row (accent
   colour) and focuses that cell; ↑/↓ move the row selection, ←/→ the focused cell; ⇧-click /
   ⇧↑↓ extend it, ⌘-click toggles a row; dragging selects a range of rows. Tabler selected single
   cells (a faint row tint) and ⇧↓ selected a cell range in one column.
2. **Delete / ⌫ delete the selected rows** (staged, red). Tabler cleared the cell and started
   editing it — the most misleading difference.
3. **Typing does not edit.** Only ↩, F2 or a double-click start editing (Space toggles the row
   details). Tabler started editing with the typed character, so a stray key changed data.
4. **⌘C copies the selected rows** (TSV of the visible columns; "Copy Cell Value" is in the context
   menu). Tabler copied the selected cells.
5. **⌘V pastes rows as new rows** when the clipboard holds whole rows (as many fields as visible
   columns); anything else pastes into the focused cell.
6. **Bottom bar: "N of M rows selected"** when rows are selected.
7. **Context menu in TablePlus order:** Open Row Details (␣), Copy, Copy Rows As ▸, Copy Cell
   Value, Copy Column Name, Paste, Set Value ▸, Quick Filter ▸, Insert Row, Duplicate, Delete;
   "Discard Changes" only on rows that have staged changes.

### Tabs, sidebar, toolbar

8. **Tabs fill the whole tab bar**, equal widths, titles centred (one tab spans the bar).
9. **One highlight in the sidebar:** the open table. Keyboard focus is a thin ring, not a second
   filled row.
10. **Recent is off by default** (it grew on every open and pushed the list under the pointer, so
    the second click of a double-click hit another table). It stays available in ⚙.
11. **Row counts are off by default** in the sidebar (TablePlus shows names only); ⚙ turns them on.
12. **Open Anything (⌘P)** lists database objects first — tables, views, schemas, databases,
    connections — as one flat ranked list `name — kind`. Since TablePlus b676 it also searches the
    app's features: from two typed characters on, up to five matching commands follow the objects
    under a "Commands" heading, each with its shortcut; `>` still lists every command. Matching is
    strict (word starts / contiguous), so `po` does not find "Duplicate row". A single click only
    selects a row; ↩ or a double-click opens it, ⌘↩ / ⌘-click opens a table in a new tab.
13. **The workspace rail is always shown** while connected, so the connection list (+) is always
    one click away, even with a single connection.
14. **The lock button is not a dead button:** it opens a small popover with the connection's mode
    (read-only or normal) and "Edit Connection…" (safe-mode levels come with B2).
15. **Data | Structure** is a clear segmented control (selected segment filled with the accent).

### Welcome

16. **Compact welcome window:** a centred panel (≈ 860 × 560) on the window background instead of
    a full-bleed page.
17. **No "Ungrouped" header:** connections without a group come first, without a header.
18. **No pre-selected row** with a strong highlight on load; selection appears on click / ↑↓.
