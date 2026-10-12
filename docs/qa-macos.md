# Tabler — manual QA checklist (macOS)

Automated coverage: `pnpm check` (typecheck, lint, format, vitest, clippy, cargo test) and the
env-gated Rust integration tests (`pnpm test:rs:int`, needs `.env` and the local databases).
WebDriver does not work with WKWebView on macOS, so the items below are verified by hand in
`pnpm tauri dev` (real IPC) before a release. Renderer-only flows can be smoke-tested faster in
`pnpm dev:mock`.

Prepare: `cp .env.example .env`, `pnpm dev:db:seed`, `pnpm dev:sqlite:build`, `pnpm tauri dev`.

## Window & chrome

- [ ] Traffic lights sit inside the 40px toolbar; dragging empty toolbar space moves the window; double-click zooms.
- [ ] Fullscreen (⌃⌘F): the left inset disappears, tabs start at the edge.
- [ ] Window size and position are restored after quitting and relaunching.
- [ ] Light/Dark follow macOS appearance when the theme is "System"; explicit Light/Dark override it.

## Menu bar & quitting

- [ ] The menu bar shows Tabler (About, Settings… ⌘,, Services, Hide, Hide Others, Show All, Quit Tabler ⌘Q), File (New Query Tab ⌘T, Open SQL File… ⌘O, Close Tab ⌘W, Reopen Closed Tab ⇧⌘T, Export… ⇧⌘E, Connection Manager ⇧⌘O), Edit, View (Open Anything… ⌘P, Switch Database… ⌘K, Switch Connection… ⇧⌘K, Data ⌃⌘[, Structure ⌃⌘], Toggle Sidebar ⌘0, Toggle Row Detail ⇧⌘I, Toggle Console Log ⇧⌘C, Refresh ⌘R, Reconnect ⇧⌘R, Enter Full Screen) and Window (Minimize, Zoom).
- [ ] Every File / View item works when chosen with the mouse; Open SQL File… outside a query tab opens a new query tab first.
- [ ] ⌘C / ⌘V / ⌘X / ⌘Z / ⌘A still work in text fields (connection form, filter bar, search) and in the SQL editor; in the grid ⌘C / ⌘V / ⌘Z keep their grid meaning (one action, not two).
- [ ] With a staged edit or an unsaved query: Tabler → Quit Tabler (mouse) and ⌘Q both show "Discard N pending changes?"; Cancel keeps the app open with everything intact; Discard quits.
- [ ] With nothing unsaved, Quit (menu or ⌘Q) quits at once.
- [ ] Menu shortcuts stay inert behind dialogs, like the page's own shortcuts: with New Connection (or Edit…) open and a host typed, ⌘T and ⇧⌘O do nothing and the form keeps its values; with Settings open ⌘T opens no tab behind it; with the Preview SQL sheet open ⌘R does not refresh the grid. ⌘W still closes Settings. Choosing those menu items with the mouse while a dialog is open does nothing either.
- [ ] Dock → Quit with a staged edit or an unsaved query shows "Discard N pending changes?": Cancel keeps the app open (a second Dock Quit asks again), Discard closes the window and the app exits. With nothing unsaved, Dock Quit (and logout / shutdown) quits at once without holding up the logout. Background: tao 0.37.1 implements no `applicationShouldTerminate:`, so the backend adds it to tao's delegate class at startup (log: "Dock / system quit goes through the unsaved-changes guard"). Verified on the debug binary with the quit Apple event the Dock sends (`osascript -l JavaScript -e "ObjC.import('AppKit'); $.NSRunningApplication.runningApplicationWithProcessIdentifier(<pid>).terminate"`): with unsaved work reported the first quit is cancelled (log: "system quit with unsaved work: asking the renderer"); with nothing unsaved the app exits right away; a renderer that never answers is overruled by a second quit 5 s later. Re-check when tao is upgraded (a warning "Dock / system quit is not guarded" means tao now implements the method itself).
- [ ] Start an export of `events_big`, then quit (menu, ⌘Q after discarding, or Dock): no `*.tabler-export` temporary file is left next to the target (the log says "removed 1 partial export file(s) on exit").

## Connections

- [ ] Create PostgreSQL (`127.0.0.1:5432`, user `apriansyahrs`, db `tabler_dev`), MySQL (`root@127.0.0.1:3306/tabler_dev`) and SQLite (`dev/fixtures/sample.sqlite`) connections; "Test" succeeds for each.
- [ ] Wrong password shows a readable error; MySQL 9 connects without error 2061 (caching_sha2_password).
- [ ] Passwords never appear in `~/Library/Application Support/com.mekayastudio.tabler/tabler.sqlite` when `TABLER_SECRETS_BACKEND=keychain`; `security find-generic-password -s tabler` finds the entry.
- [ ] Edit, duplicate (password copied) and delete a connection; groups, colours and tag pills render on the welcome screen.
- [ ] SSH: a profile with an SSH tunnel prompts for an unknown host key fingerprint, connects after "Trust", and reports a changed key as an error.
- [ ] Welcome screen: double-clicking a connection (or ↩ on the selected row) opens exactly one session (`list_connections` / server process list shows one); a profile that is already open switches to it. ↑/↓ move the selection, ⌘F searches, ⌫ asks to delete, right-click offers Connect / Edit… / Duplicate / Copy as URL / Delete….
- [ ] ⌘N (anywhere) shows the driver picker, then the one-page form (Name, Tag, Status color, Host/Port, User, Password, Database, SSL; Over SSH and Advanced reveal their fields); Test, Save and Connect work. "New connection from URL…" fills the form from `postgres://user:pass@host:5432/db`.
- [ ] SQLite: Test and Connect on a mistyped or moved path report "SQLite database file not found: … Use “New…” to create one." and no file appears at that path.
- [ ] SQLite: "New…" opens a save panel (`database.sqlite`), creates a valid empty database (`sqlite3 <file> .tables` works) and fills the path; choosing an existing file in the panel leaves its data untouched.
- [ ] SQLite: Advanced → "Enforce foreign keys" off lets an orphan `posts.user_id` be committed; on (default) rejects it.
- [ ] With PostgreSQL running and connected, stop it (`brew services stop postgresql@17`) and page the grid: the "Connection … lost" banner appears within seconds (not at the next 60 s keepalive); Reconnect works once the server is back.
- [ ] Same with the errors that come back inside a result: after stopping the server, `select 1` in a query tab, ⌘S on a staged grid edit, and Apply in the Structure tab each show their error **and** the lost banner within seconds.

## Sidebar & tabs

- [ ] PostgreSQL shows `public` and `inventory` schemas; MySQL shows `tabler_dev`; tables/views/materialized views are grouped with row estimates.
- [ ] `Order Items` and `CamelCase` open correctly (quoted identifiers).
- [ ] ⇧⌘F (and ⌘L) focuses table search; ⌘T / ⌘E new query; ⌘W closes (asks when there are unsaved edits); ⌘1–9 and ⌘[ / ⌘] switch; drag to reorder; ⇧⌘T reopens.
- [ ] Preview tabs: a single click on a table opens it in an italic tab; clicking another table replaces it; ↑/↓ on a focused sidebar row opens the next table in that tab while focus stays in the sidebar. A double-click (on the table or on the tab), a staged edit, an applied filter or a staged structure change makes the tab regular (upright title). Middle-click or "Open in New Tab" opens a regular tab.
- [ ] "Recent" lists the last five tables opened from the sidebar (per connection and database), above "Pinned", and survives a restart.
- [ ] ⌃⌘] / ⌃⌘[ (or the Data | Structure switch at the bottom-left) switch the same tab between rows and structure; staged row edits and staged structure changes both survive the switch and both count in the tab's dirty dot.
- [ ] ⌘P (Open Anything) finds tables, tabs, schemas, databases, connections and actions; ↩ opens a table in the preview tab, ⌘↩ in a new tab. ⌘K lists the databases (and schemas) and switches; ⇧⌘K lists the connections and connects or switches.
- [ ] ⇧⌘C shows the console log under the tab: browsing, counting, commits, structure loads and editor statements appear with time, connection, duration and rows; the All / Data / Structure / Query filter and Clear work.
- [ ] Two connections open: the rail at the far left shows both; clicking one switches the whole workspace (sidebar, tab bar, status pill, accent colour). Each connection keeps its own tabs; ⌘1–9, ⌘[ / ⌘], Close Others and Close All stay inside the visible workspace; ⌘T opens the query on the visible connection.
- [ ] The status pill shows the tag, server, connection, database and the visible table (or "SQL Query"); clicking it opens the connection switcher; a lost connection turns it amber.
- [ ] The toolbar's Discard / Preview / Commit light up with staged changes (Commit shows the count); the panel toggles open / close the sidebar, console and row details.
- [ ] Pins: right-click a table → "Pin to Top" lists it under "Pinned" (with a pin icon) and still in its group; search filters pinned rows too; pins survive a restart and are separate per connection and per database; "Unpin" removes it.

## Data grid

- [ ] `events_big` (100 000 rows): first page < 300 ms, smooth scrolling, last page reachable, sort by `ts` desc is server-side.
- [ ] `users.big_id` shows 9007199254740993 exactly; `balance` keeps 6 decimals; NULL differs from empty string; JSON/array/bytea render; CJK/emoji/RTL rows are correct.
- [ ] Filter bar (⌘F): `IN`, `BETWEEN`, `LIKE`, `IS NULL`, AND/OR; Enter applies.
- [ ] Filter bar "Raw SQL" (first entry of the column list): `age > 30 AND name LIKE 'A%'` filters the rows, combines with other conditions under AND/OR, and a broken fragment shows the server error.
- [ ] Column header menu (sort, hide, copy name…) and drag-to-reorder columns; with columns hidden or reordered, editing a cell changes the right column (check Preview SQL).
- [ ] FK arrow on `posts.user_id` opens `users` filtered to that id.
- [ ] Clicking a cell selects its whole row (TablePlus); ⇧-click / ⇧↑↓ extend, ⌘-click toggles rows, dragging selects a range; the bottom bar reads "N of M rows selected".
- [ ] ⌘C copies the selected rows as TSV (pastes into Numbers); "Copy Rows As" JSON / INSERT / Markdown produce valid output; "Copy Cell Value" copies one value.
- [ ] ⌥⌘C opens the copy-as picker; each format copies the selected rows.
- [ ] ⌘V of whole rows (as many tab-separated fields as columns) adds them as new rows in one undo step; a single value pastes into the focused cell.
- [ ] ␣ (Space) on the grid opens the row details sidebar and closes it again; ⇧⌘I toggles it too. Details lists every column (search, types, NULL / EMPTY placeholders); editing a field (↩ or blur) stages it like a grid edit; its ⋯ menu sets NULL / EMPTY / DEFAULT. The Cell view shows the full value of the focused cell (binary as a hex dump) and loads large values on demand.
- [ ] Right-click a cell → Quick Filter (= / ≠ / IN / NOT IN / IS NULL) filters the table; header menu → Filter with Column adds a focused condition. In the filter bar ⌘I duplicates a condition, ⇧⌘I removes it, ⌘↩ applies all, ⎋ closes it.
- [ ] ⌥⌘F opens the Columns menu.

## Editing

- [ ] Double-click / ↩ / F2 edit a cell (typing a letter does NOT edit); Esc cancels; Tab moves right. Bool and enum cells open a picker (with NULL for nullable columns).
- [ ] ⌫ / Delete on selected rows marks them for deletion (red), never clears a cell.
- [ ] Edited cells turn yellow, new rows green, deleted rows red; the toolbar's Commit shows the count; ⌘S applies straight away (with "Confirm before commit" off, the default).
- [ ] ⌘S opens Preview SQL with the exact statements; ⌘↩ applies in one transaction; data reloads. Esc (or Apply) hands the keyboard back to the grid: ⌘Z / arrows work without clicking it first.
- [ ] Unique violation (duplicate email) rolls back everything and marks the failing row.
- [ ] ⇧⌘N sets NULL, ⇧⌘D sets DEFAULT, Set Value ▸ EMPTY (or ⌥-click a text cell) sets ''; ⌘Z undoes, ⇧⌘Z redoes, ⇧⌘⌫ discards, ⇧⌘P previews the SQL; paging with pending edits asks first.
- [ ] ⌘I (or double-click below the last row) adds a row and focuses it.
- [ ] ⌘V pastes a block copied from Numbers into the selected cells (extra rows become new rows) as staged edits.
- [ ] ⌥↩ in `users.bio` opens the multi-line editor; a multi-line value commits and reloads with its line breaks intact.
- [ ] A JSON / JSONB cell opens the JSON editor; invalid JSON is refused with the parse error, valid JSON saves.
- [ ] `audit_log` (no primary key) shows the warning banner; SQLite `audit_log` edits via rowid.

## SQL editor

- [ ] ⌘↩ runs the statement under the cursor (or the selection); ⇧⌘↩ runs all; several result sets appear as tabs. The Run button's dropdown switches what the button runs.
- [ ] `select pg_sleep(30)` / `select sleep(30)` are cancelled within ~1 s with ⌘.
- [ ] Syntax errors highlight the position in the editor; Messages lists affected rows and timings.
- [ ] Autocomplete offers tables and columns; ⌘I beautifies and ⇧⌘I uglifies the current statement (⌥⇧F still formats); ⌃` hides / shows the result pane; history survives a restart.
- [ ] After `ALTER TABLE users ADD COLUMN x int` in a query tab, ⌘R (or switching to the table) shows the new column in the sidebar, grid and autocomplete.
- [ ] EXPLAIN ANALYZE of `DELETE FROM posts WHERE id < 10` shows the plan and the rows are still there afterwards (PostgreSQL and MySQL).
- [ ] ⇧⌘E in a query tab exports the result on screen (not the table). For `select * from events_big` (cut at the row limit) the dialog offers "All rows", which runs the statement again. A writing statement is never run again: on PostgreSQL run `BEGIN`, then `UPDATE events_big SET id = id RETURNING *` (also cut at the limit); ⇧⌘E offers only the rows shown; then run `ROLLBACK`.

## Structure & export

- [ ] Structure tab lists columns, indexes, foreign keys, triggers; edits preview `ALTER TABLE` and apply (SQLite uses a table rebuild with a warning).
- [ ] Export `events_big` to CSV (100 001 lines) and JSON (valid); cancel mid-export removes the partial file.
- [ ] "Open in SQL" in the Structure tab opens the table's `CREATE` statement in a new query tab.
- [ ] The primary-key toggle on a column stages the PK change and previews the right `ALTER TABLE`.

## Packaging

- [ ] `pnpm tauri build` produces `Tabler.app` and a `.dmg`; the app launches from /Applications; icon looks right in Dock and Finder (light and dark).
- [ ] Ad-hoc signature (`signingIdentity: "-"`): `codesign -dv --verbose=2 src-tauri/target/release/bundle/macos/Tabler.app` shows `Identifier=com.mekayastudio.tabler`, `Signature=adhoc` and `Sealed Resources`; `codesign --verify --deep --strict --verbose=2 …/Tabler.app` passes (prints "valid on disk" and "satisfies its Designated Requirement").
- [ ] The DMG copied to another Mac opens after right-click → Open (not "damaged").
