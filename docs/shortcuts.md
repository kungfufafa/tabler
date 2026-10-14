# Tabler keyboard shortcuts

Every key Tabler binds, where it works, and where it differs from TablePlus for macOS. The
TablePlus column follows `docs/research/tableplus-functional-spec.md` §10.2 (public docs, blog,
changelog and issue tracker only).

- **Source of truth**: `src/lib/keys/keymap.ts` (`KEYMAP`). Settings ▸ Shortcuts lists it and remaps
  any binding except ⌘R and ⌘0, which are fixed as in TablePlus (#3802). The native menu shows
  the same keys and follows a remap.
- **Scopes**: a key means something different depending on what has the focus: the SQL editor,
  a data grid, the filter bar, a cell editor, the sidebar, or a dialog. Where nothing scoped has
  the focus (the tab bar, the toolbar, a tab's bottom bar) the **Global** binding applies.
- `⌘` is Ctrl off macOS. "Menu" names the native menu item that shows the key.
- **Fixed** keys cannot be remapped. **Built-in** keys are handled by a component itself (the grid,
  CodeMirror, the sidebar tree); they are listed in Settings ▸ Shortcuts for reference.

## Global

| Key            | Action                                                                                                                                                                                                           | Menu                                        | TablePlus                                                                                                                                            |
| -------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------- |
| ⌘N             | New window                                                                                                                                                                                                       | File ▸ New Window                           | Same ("New window / new workspace / open a new connection")                                                                                          |
| ⌘,             | Settings                                                                                                                                                                                                         | Tabler ▸ Settings…                          | Same (Preferences)                                                                                                                                   |
| ⌘Q             | Quit (asks about unsaved work)                                                                                                                                                                                   | Tabler ▸ Quit                               | Same                                                                                                                                                 |
| ⇧⌘O            | New SQL Viewer                                                                                                                                                                                                   | File ▸ New SQL Viewer                       | Same                                                                                                                                                 |
| ⌘T             | New query tab (the "Command + T should open" setting)                                                                                                                                                            | File ▸ New Query Tab                        | Same                                                                                                                                                 |
| ⌘E             | Open the SQL editor, also from inside the SQL editor (⌥⌘E is its Explain)                                                                                                                                        | —                                           | Same                                                                                                                                                 |
| ⌘↩             | Open the SQL editor from a table, structure or tool tab; Run Current in a SQL tab                                                                                                                                | —                                           | Same (context-dependent ⌘↩)                                                                                                                          |
| ⌘W             | Close the tab; with no tab to close (welcome screen, Connection Manager, an empty workspace, a ⌘N window) close the window, asking about unsaved work first. In Settings and the standalone SQL Viewer: close it | File ▸ Close Tab / Window                   | Same                                                                                                                                                 |
| ⇧⌘W            | Close all tabs of the workspace                                                                                                                                                                                  | File ▸ Close All Tabs                       | Same ("Close all Tabs", b354)                                                                                                                        |
| ⇧⌘T            | Reopen closed tab                                                                                                                                                                                                | File ▸ Reopen Closed Tab                    | Tabler addition                                                                                                                                      |
| ⌘R **(fixed)** | Reload workspace: the current tab, the sidebar, the other table tabs' rows, structures, enums and completion                                                                                                     | File ▸ Reload Workspace                     | Same                                                                                                                                                 |
| ⌥⌘R            | Reload the current tab (and the sidebar list)                                                                                                                                                                    | View ▸ Refresh                              | Same                                                                                                                                                 |
| ⇧⌘R            | Force reconnect: a new session even when the old one still answers a ping (tabs, staged edits and database are kept). Also on the Connection Manager                                                             | Connection ▸ Reconnect                      | Same (b136 "Force reconnect"); the banner button and auto-reconnect ping first                                                                       |
| ⌘.             | Process list (in the SQL editor: cancel the running query)                                                                                                                                                       | Connection ▸ Process List…                  | Same; the docs also call it "Run custom script"                                                                                                      |
| ⌘S             | Commit staged changes (in the SQL editor: save the file). A cell editor (grid cell, Details field) stages what was typed first, then commits                                                                     | —                                           | Same                                                                                                                                                 |
| ⇧⌘P            | Preview the pending SQL (also with the SQL editor focused, while the workspace holds staged changes)                                                                                                             | —                                           | Same                                                                                                                                                 |
| ⇧⌘⌫            | Discard all staged changes (also with the SQL editor focused, while the workspace holds staged changes)                                                                                                          | —                                           | Same                                                                                                                                                 |
| ⌘P             | Open Anything                                                                                                                                                                                                    | View ▸ Open Anything…                       | Same                                                                                                                                                 |
| ⌘K             | Switch database (palette)                                                                                                                                                                                        | View ▸ Switch Database…                     | Same                                                                                                                                                 |
| ⇧⌘K            | Switch connection (palette)                                                                                                                                                                                      | View ▸ Switch Connection…                   | Same                                                                                                                                                 |
| ⌘0 **(fixed)** | Toggle the left sidebar                                                                                                                                                                                          | View ▸ Toggle Sidebar                       | Same                                                                                                                                                 |
| ⌘B             | Toggle the left sidebar (alternative; ⌘B in the filter bar checks a filter)                                                                                                                                      | —                                           | Tabler addition                                                                                                                                      |
| ⇧⌘I            | Toggle the row detail panel (in the SQL editor: Uglify)                                                                                                                                                          | View ▸ Toggle Row Detail                    | Tabler key; TablePlus has no documented key                                                                                                          |
| ⇧⌘C            | Toggle the console log (in a table grid: copy the cell value)                                                                                                                                                    | View ▸ Toggle Console Log                   | ⚠ TablePlus sources conflict: docs say console, b446 says Copy Cell Value. Tabler keeps both: the focus decides (grid: copy, anywhere else: console) |
| ⇧⌘F            | Focus the sidebar's item search                                                                                                                                                                                  | —                                           | Same                                                                                                                                                 |
| ⌘L             | Manage plugins                                                                                                                                                                                                   | Plugins ▸ Manage Plugins…                   | Same                                                                                                                                                 |
| ⇧⌘E            | Export…                                                                                                                                                                                                          | File ▸ Export…                              | Same                                                                                                                                                 |
| ⌥⌘F            | Show / hide columns                                                                                                                                                                                              | View ▸ Columns                              | TablePlus "column filter"                                                                                                                            |
| ⌃⌘[ / ⌃⌘]      | Data / structure view of a table tab                                                                                                                                                                             | View ▸ Data / Structure                     | Same                                                                                                                                                 |
| ⌘[ / ⌘]        | Previous / next tab                                                                                                                                                                                              | Navigate ▸ Select Previous / Next Tab       | Same                                                                                                                                                 |
| ⌃⇧⇥ / ⌃⇥       | Previous / next tab (alternatives)                                                                                                                                                                               | —                                           | Tabler addition                                                                                                                                      |
| ⇧⌘[ / ⇧⌘]      | Previous / next pane (sidebar → tab → row detail → console; a split workspace's two groups and a split SQL tab's two editors are panes of their own)                                                             | Navigate ▸ Select Previous / Next Pane      | Same (the 2018 blog says ⌥⌘[ / ⌥⌘])                                                                                                                  |
| ⌘1…⌘8, ⌘9      | Go to tab N, last tab                                                                                                                                                                                            | Navigate ▸ Go to Tab                        | Same                                                                                                                                                 |
| ⌃- / ⌃⇧-       | Back / forward in the tab history                                                                                                                                                                                | Navigate ▸ Back / Forward                   | Tabler addition                                                                                                                                      |
| ⌃⌘← / ⌃⌘→      | Previous / next workspace                                                                                                                                                                                        | Navigate ▸ Select Previous / Next Workspace | TablePlus lists the items (Keymap ▸ Navigate) without a documented default key                                                                       |
| ⇧⌘\\           | Show workspaces                                                                                                                                                                                                  | View ▸ Show Workspaces                      | Same                                                                                                                                                 |
| ⇧⌘D            | Split right: from the grid, the sidebar, the filter bar, the structure tab and the tab bar (not in the SQL editor, which splits its pane, nor in a cell editor)                                                  | View ▸ Split Right                          | Same ("split screen right", b600)                                                                                                                    |
| ⌃`             | Toggle the SQL tab's result pane                                                                                                                                                                                 | —                                           | Tabler addition                                                                                                                                      |
| ⌃⌘L            | Lock Tabler                                                                                                                                                                                                      | Window ▸ Lock Tabler                        | Tabler addition                                                                                                                                      |

## Menu items without a key

TablePlus gives these no key either, unless noted. They can be bound in Settings ▸ Shortcuts only
when they have a keymap entry (Connection Manager and New Workspace do not).

| Menu item                                                                          | What it does                                                                                                                 | Note                                                                                         |
| ---------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------- |
| File ▸ New Connection…                                                             | The connection form                                                                                                          |                                                                                              |
| File ▸ New Workspace                                                               | The connection picker (⇧⌘K); the picked connection opens in a workspace of its own, or its open workspace comes to the front | ⚠ TablePlus's 2018 menu shows ⌘N here; Tabler's ⌘N is New Window                             |
| File ▸ Open Recent ▸                                                               | The last 10 connections and the last 10 `.sql` files, Clear Menu                                                             | A file opens in a query tab of the active connection, or in the SQL Viewer when none is open |
| File ▸ Open SQL File…                                                              | ⌘O in the SQL editor; with no connection open the file opens in the SQL Viewer                                               | TablePlus "Open…"                                                                            |
| File ▸ Save SQL As…, Save Selected SQL Statements As… (⇧⌘S), Add to Queries… (⌥⌘S) | The SQL editor's files                                                                                                       |                                                                                              |
| File ▸ Backup… / Restore… / Import ▸                                               | Backup, restore, CSV / JSON / SQL import                                                                                     |                                                                                              |
| File ▸ Connection Manager                                                          | The welcome screen with the saved connections                                                                                | Was ⇧⌘O before the SQL Viewer took it                                                        |
| Edit ▸ Copy Cell Value                                                             | The focused grid cell's value                                                                                                | ⇧⌘C in a grid (a scoped key: the menu item shows none; elsewhere ⇧⌘C is the console log)     |
| Edit ▸ Toggle Line / Block Comment                                                 | ⌘/ and ⌥⌘/ in the editor                                                                                                     |                                                                                              |
| Edit ▸ Set Value ▸ EMPTY / DEFAULT / Now                                           | Staged on the selected cells (⇧⌘N NULL, ⌥⌘D DEFAULT in the grid)                                                             |                                                                                              |
| Connection ▸ View Using Encoding…, User Management…, Disconnect                    |                                                                                                                              |                                                                                              |
| View ▸ Metrics Board                                                               |                                                                                                                              |                                                                                              |
| Window ▸ Move Tab to New Window                                                    |                                                                                                                              |                                                                                              |

## SQL editor

| Key                         | Action                                                         | TablePlus                                                                      |
| --------------------------- | -------------------------------------------------------------- | ------------------------------------------------------------------------------ |
| ⌘↩                          | Run the current statement (or the selection)                   | Same (remappable, b336)                                                        |
| ⇧⌘↩                         | Run all                                                        | Same                                                                           |
| ⌘.                          | Cancel the running query (nothing running: process list)       | Same                                                                           |
| ⌥⌘E                         | Explain                                                        | Tabler addition (TablePlus has no key for it; ⌘E opens a SQL editor, here too) |
| ⌘I / ⌥⇧F                    | Beautify                                                       | Same (⌥⇧F is a Tabler alternative)                                             |
| ⇧⌘I                         | Uglify                                                         | Same                                                                           |
| ⌃I                          | SQL Format (plugin)                                            | Same (SQL Formatter plugin)                                                    |
| ⌘S                          | Save the file (commits staged result edits first)              | Same (#1438)                                                                   |
| ⌘O                          | Open a `.sql` file                                             | Same                                                                           |
| ⇧⌘S                         | Save the selected statements as…                               | Same                                                                           |
| ⌥⌘S                         | Add to Queries…                                                | Tabler addition                                                                |
| ⌘H                          | Query history                                                  | Tabler addition                                                                |
| ⌘/                          | Toggle line comment (remappable)                               | Same                                                                           |
| ⌥⌘/                         | Toggle block comment (remappable)                              | Same (b210, no documented key)                                                 |
| ⌘= (⇧⌘=) / ⌘-               | Font size up / down                                            | Same (⌘+ / ⌘−)                                                                 |
| ⇧⌘D                         | Split the editor (or close the split)                          | Same                                                                           |
| ⌘D                          | Select the next occurrence (built-in)                          | Same                                                                           |
| ⌃⌘G                         | Select every occurrence (built-in)                             | Same                                                                           |
| ⌘-click, ⌃⇧↑ / ⌃⇧↓          | Add a caret / a caret above or below (built-in)                | Same                                                                           |
| ⎋                           | Back to a single caret (built-in)                              | Same                                                                           |
| ⌘F, ⌘G / ⇧⌘G, F3 / ⇧F3, ⌥⌘G | Find and replace, next / previous match, go to line (built-in) | ⌘F same; the rest are CodeMirror's                                             |
| ⌥↑ / ⌥↓, ⌃Space             | Move line, show completions (built-in)                         | CodeMirror's                                                                   |
| ⌘Z / ⇧⌘Z                    | Undo / redo (built-in)                                         | Same                                                                           |

The SQL Viewer (⇧⌘O) uses the same editor keys except the run, explain and history keys: it has no
connection and runs nothing. ⌘S / ⌘O / File ▸ Save SQL As… act on the viewer while it shows.

## Table grid

| Key                                                                   | Action                                                                   | TablePlus                                                                                                                                                           |
| --------------------------------------------------------------------- | ------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| ⌘↩, middle-click                                                      | Quick Look editor of the focused cell                                    | Same (⌘↩ also peeks a foreign key in TablePlus)                                                                                                                     |
| ⌥↩, ⌥-click                                                           | Set Value menu                                                           | Same                                                                                                                                                                |
| Space                                                                 | Toggle the row detail                                                    | Same                                                                                                                                                                |
| ⌘C / ⌥⌘C / ⌘V                                                         | Copy rows, copy rows as…, paste cells / rows (TSV or CSV text)           | Same (⌥⌘C is a Tabler addition)                                                                                                                                     |
| ⌘F                                                                    | Filter rows (opens the filter bar on the last filter)                    | Same                                                                                                                                                                |
| ⌘I                                                                    | New row                                                                  | Same                                                                                                                                                                |
| ⌘D                                                                    | Duplicate row                                                            | Same                                                                                                                                                                |
| ⌘⌫, ⌫, ⌦                                                              | Delete the selected rows (staged)                                        | Same (⌫)                                                                                                                                                            |
| ⇧⌘N / ⌥⌘D (or ⌥⇧⌘D)                                                   | Set NULL / DEFAULT                                                       | Tabler additions. ⇧⌘D is Split Right now; macOS keeps ⌥⌘D for Show / Hide Dock, so ⌥⇧⌘D is the same action (or turn the Dock key off in System Settings ▸ Keyboard) |
| ⇧⌘C                                                                   | Copy the focused cell's value                                            | Same (b446; outside a grid ⇧⌘C is the console log)                                                                                                                  |
| ⌘J                                                                    | Open the referenced row (a menu when the column has several keys)        | Tabler addition                                                                                                                                                     |
| ⌘Z / ⇧⌘Z                                                              | Undo / redo a staged change                                              | Same                                                                                                                                                                |
| ⌘← / ⌘→                                                               | Previous / next page                                                     | Same                                                                                                                                                                |
| ↑ ↓ ← → ⇥ ⇧⇥, ⇧↑ / ⇧↓, ⌘↑ / ⌘↓, ⌘Home / ⌘End, Home / End, PgUp / PgDn | Move, extend the selection, first / last row and column, page (built-in) | Same at the edges: ⌘↑ on the first row focuses the filter bar (when shown), ⌘↓ on its last filter row the data                                                      |
| ↩, F2                                                                 | Edit the cell (built-in)                                                 | Same                                                                                                                                                                |
| ⎋                                                                     | Select only the focused row (built-in)                                   | Same                                                                                                                                                                |
| ⌘A                                                                    | Select all rows (built-in)                                               | Same                                                                                                                                                                |

Cell editors: ↩ saves and moves down, ⇥ / ⇧⇥ save and move, ⎋ cancels, ⌥↩ adds a line (opens the
multi-line editor), ⌘↩ saves in the JSON / multi-line editor, ⌘S stages the edit where the cell is
and then commits the tab's staged changes (text that does not parse stays in the editor with its
error, and nothing is committed).

## Structure tab

| Key                             | Action                                                                                                        | TablePlus                          |
| ------------------------------- | ------------------------------------------------------------------------------------------------------------- | ---------------------------------- |
| ⌘I                              | New column; in the Indexes grid a new index, in the Triggers view a new trigger                               | Same                               |
| Double-click a blank row        | Add a column / index / trigger: the blank row that ends each grid, its "No …" row or the empty space below it | Same ("double-click a blank row")  |
| Click a row, ⌘-click / ⇧-click  | Select a column (several for the right sidebar and for Delete); the row takes the focus                       | Same (multi-select takes priority) |
| ⌫, ⌦, ⌘⌫ (right-click ▸ Delete) | Drop the focused row (stage the drop); on a selection of several columns all of them, in one undo step        | Same (Delete, right-click Delete)  |
| ⌘↩                              | Open the SQL editor                                                                                           | Same (context-dependent ⌘↩)        |
| ⌘Z / ⇧⌘Z, ⌘S, ⇧⌘P, ⇧⌘⌫          | Undo / redo a staged change, commit, preview the SQL, discard                                                 | Same                               |

Delete in a text cell edits the text. The staged changes of one table are one `ALTER TABLE` on MySQL and
PostgreSQL (see the README); MySQL index columns take a prefix length per char / text / blob column.

## Row details (right sidebar)

Fields stage their value like a grid cell: ↩ (⌘↩ once the text spans lines, and for JSON), blur or
a click elsewhere stages it, ⎋ reverts, ⌘S stages it and commits the tab. Every text-like field
(text, JSON, XML, enum without a list, set, array, spatial and other values, one line or many) is a
CodeMirror editor (the Quick Look Editor's, compact; it grows with its text up to about twelve lines,
then scrolls): ⌘-click and ⌃⇧↑ / ⌃⇧↓ add carets, ⌘D the next occurrence, ⌘F find and replace, JSON and
XML are coloured, ⌘Z / ⇧⌘Z undo and redo what was typed in the field (a new row or a new value in the
field starts a new undo history, so ⌘Z never brings back another row's value), ⎋ closes the search
panel or goes back to one caret before it reverts anything. A one-line value is staged by ↩ like a
native box and ⌥↩ adds a line; once the text spans lines (and always for JSON) ↩ is a plain line
break and ⌘↩ stages. ⇥ moves to the next field. Numbers, dates, times and UUIDs stay plain one-line
inputs. The field's ⋯ menu has Pretty JSON and Sort keys (JSON, display only: nothing is staged until
you stage the text).

Foreign keys: the → arrow of an FK cell, ⌘J and the cell menu's Open jump to the referenced table,
filtered to the value (a binary key by its hex; a composite key by every column of the key, read
from the row). A column that is part of several keys opens a small menu of `table (column)` under
the cell first (↑ ↓ ↩ pick, ⎋ closes); Generate JOIN asks the same way.

Paste: ⌘V reads tab separated text (spreadsheets, Copy) and CSV: comma or semicolon separated
text, with the Table Data CSV delimiter tried first and `"` quoting (RFC 4180, line breaks inside
quotes). Lines of one field per visible column become new rows; narrower blocks fill cells from the
focused one. CSV is taken when every line has the same number of fields (at least two) and there
are several lines, or the single line has one field per visible column; a lone `Smith, John` stays
one value. The first line is data, not a header.

## Filter bar

| Key      | Action                                                                      | TablePlus |
| -------- | --------------------------------------------------------------------------- | --------- |
| ⌘↩ / ↩   | Apply all filters                                                           | Same      |
| ⇧⌘↩      | Apply all filters with OR                                                   | Same      |
| ⌘I / ⇧⌘I | Duplicate (column and operator, blank value, ready to type) / remove filter | Same      |
| ⌘↑ / ⌘↓  | Previous / next filter; ⌘↓ on the last one focuses the grid                 | Same      |
| ⌘B       | Check / uncheck the filter                                                  | Same      |
| ⌘← / ⌘→  | Column / operator popup                                                     | Same      |
| ⎋        | Close the filter bar                                                        | Same      |

In the grid, ⌘↑ on the first row (or with no rows) focuses the last filter row while the bar is shown;
anywhere else ⌘↑ / ⌘↓ still jump to the first / last row. The `+` button adds a blank filter on the
same column and operator, like ⌘I. The **SQL** button shows the whole generated query (SELECT,
ORDER BY, LIMIT / OFFSET, highlighted) with a Copy button; "WHERE only" narrows it to the condition.

## Sidebar

| Key                           | Action                                     | TablePlus                                                                          |
| ----------------------------- | ------------------------------------------ | ---------------------------------------------------------------------------------- |
| ⇧⌘F                           | Focus the item search                      | Same                                                                               |
| ⌘-click, ⇧-click, ⇧↑ / ⇧↓, ⌘A | Select items                               | Same                                                                               |
| ⌘C                            | Copy the selected names                    | Tabler addition                                                                    |
| ⌫                             | Delete the selected items (staged)         | Same                                                                               |
| ⌥⌫                            | Truncate the selected tables               | Same (b233)                                                                        |
| ⇧⌫                            | Delete with CASCADE (PostgreSQL)           | Same                                                                               |
| ⌘⌫                            | Delete ignoring foreign key checks (MySQL) | Same (b270)                                                                        |
| ⌘↩ (History, Queries)         | Run the entry in a new query tab           | Tabler addition; the global ⌘↩ (open the SQL editor) does not apply in the sidebar |

## Sidebar quick navigation buttons

The History pane follows TablePlus (b436): a **click only selects** the entry (⌘-click / ⇧-click select
several, ⌘C copies them), a **double-click or ↩ opens the statement in a new SQL tab and never runs
it**; running is Run in New Tab in the menu or ⌘↩ on the selected row. The menu reads Copy, Run in
New Tab, Open in New Tab, Insert to SQL Editor, Add to Queries…, Delete. In the ⌘H sheet a click
selects, a double-click (or ↩) inserts the statement at the cursor without running it, and ⌘↩ (or the
entry's Run button) runs it.

Three icon buttons sit above the Items | Queries | History switch (TablePlus's left sidebar, §2.2).
Each tooltip shows the key as it is bound now, so a remapped key shows up there.

| Button            | Key | What it does                                                                              | TablePlus |
| ----------------- | --- | ----------------------------------------------------------------------------------------- | --------- |
| Show Connections  | ⇧⌘K | Opens the connection palette                                                              | Same      |
| Show Databases    | ⌘K  | Opens the database palette                                                                | Same      |
| Open Query Editor | ⌘↩  | Opens a new SQL tab (the button always opens a new one; in a SQL tab ⌘↩ runs a statement) | Same      |

Show Databases and Open Query Editor belong to a connection and are disabled while none is open;
Show Connections is always enabled, it is how you open one.

## Welcome screen and palette

- Welcome screen: ⌘F searches the connections (TablePlus #1760); ↩ opens the selected one.
- Open Anything / ⌘K / ⇧⌘K: ↑ ↓ (⌃P / ⌃N) move, ↩ or a double-click opens, a single click only selects
  the row, ⌘↩ or ⌘-click opens a table in a new tab (b256, #2454: always a new one, even when the
  table is open already: one table can sit in several tabs), ⎋ closes; pressing the palette's own
  key again closes it, another palette key switches the list.
- Open Anything also searches the app's features (TablePlus b676): after the objects it lists up to
  five matching commands under "Commands", from two typed characters on, each with its shortcut.
  `>` first lists every command.
- Several tabs of one table: sidebar right-click ▸ Open in New Tab and middle-click, the palette's
  ⌘↩ / ⌘-click and the tab menu's Duplicate Tab (it copies the tab's filters, sort and columns, not
  its staged edits) each open another tab. Double-click, ↩ and the single-click preview show the
  table's own tab. (⌘-click in the sidebar selects several tables, as in TablePlus.)

## TablePlus keys with a different meaning in Tabler

| Key                       | TablePlus                                    | Tabler                             | Why                                                                        |
| ------------------------- | -------------------------------------------- | ---------------------------------- | -------------------------------------------------------------------------- |
| ⌘N                        | New window / new workspace (File menu, 2018) | New window                         | New Workspace is a menu item without a key                                 |
| ⌘↑ / ⌘↓ in a grid         | Focus the filter bar / the data (#2974)      | First / last row; bar at the edge  | ⌘↑ on the first row goes up to the filter bar (shown); elsewhere both jump |
| ⌘.                        | "Run custom script" (docs label)             | Process list / cancel              | The blog and #343 describe cancel / process list                           |
| ⌘B outside the filter bar | —                                            | Toggle sidebar (alternative to ⌘0) | Kept from earlier Tabler builds                                            |

## Open items

None: every key of the TablePlus table (§10.2) that Tabler binds is listed above.
