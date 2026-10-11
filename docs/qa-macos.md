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

## Connections

- [ ] Create PostgreSQL (`127.0.0.1:5432`, user `apriansyahrs`, db `tabler_dev`), MySQL (`root@127.0.0.1:3306/tabler_dev`) and SQLite (`dev/fixtures/sample.sqlite`) connections; "Test" succeeds for each.
- [ ] Wrong password shows a readable error; MySQL 9 connects without error 2061 (caching_sha2_password).
- [ ] Passwords never appear in `~/Library/Application Support/id.co.oceanspace.tabler/tabler.sqlite` when `TABLER_SECRETS_BACKEND=keychain`; `security find-generic-password -s tabler` finds the entry.
- [ ] Edit, duplicate (password copied) and delete a connection; groups and colours render in the launcher.
- [ ] SSH: a profile with an SSH tunnel prompts for an unknown host key fingerprint, connects after "Trust", and reports a changed key as an error.

## Sidebar & tabs

- [ ] PostgreSQL shows `public` and `inventory` schemas; MySQL shows `tabler_dev`; tables/views/materialized views are grouped with row estimates.
- [ ] `Order Items` and `CamelCase` open correctly (quoted identifiers).
- [ ] ⌘L focuses table search; ⌘T new query; ⌘W closes (asks when there are unsaved edits); ⌘1–9 switch; drag to reorder; ⇧⌘T reopens.

## Data grid

- [ ] `events_big` (100 000 rows): first page < 300 ms, smooth scrolling, last page reachable, sort by `ts` desc is server-side.
- [ ] `users.big_id` shows 9007199254740993 exactly; `balance` keeps 6 decimals; NULL differs from empty string; JSON/array/bytea render; CJK/emoji/RTL rows are correct.
- [ ] Filter bar (⌘F): `IN`, `BETWEEN`, `LIKE`, `IS NULL`, AND/OR; Enter applies.
- [ ] FK arrow on `posts.user_id` opens `users` filtered to that id.
- [ ] Copy (⌘C) pastes as TSV in Numbers; "Copy as JSON/INSERT/Markdown" produce valid output.
- [ ] Inspector (⇧⌘I) shows the full value of the focused cell.

## Editing

- [ ] Double-click / Enter / typing edits a cell; Esc cancels; Tab moves right.
- [ ] Edited cells turn yellow, new rows green, deleted rows red; status bar shows "N pending changes".
- [ ] ⌘S opens Preview SQL with the exact statements; ⌘↩ applies in one transaction; data reloads.
- [ ] Unique violation (duplicate email) rolls back everything and marks the failing row.
- [ ] ⌘⇧N sets NULL, ⌘⇧D sets DEFAULT, ⌘Z undoes, ⌘⇧Z discards; paging with pending edits asks first.
- [ ] `audit_log` (no primary key) shows the warning banner; SQLite `audit_log` edits via rowid.

## SQL editor

- [ ] ⌘↩ runs all / selection; ⇧⌘↩ runs the statement under the cursor; several result sets appear as tabs.
- [ ] `select pg_sleep(30)` / `select sleep(30)` are cancelled within ~1 s with ⌘.
- [ ] Syntax errors highlight the position in the editor; Messages lists affected rows and timings.
- [ ] Autocomplete offers tables and columns; ⌥⇧F formats; history survives a restart.

## Structure & export

- [ ] Structure tab lists columns, indexes, foreign keys, triggers; edits preview `ALTER TABLE` and apply (SQLite uses a table rebuild with a warning).
- [ ] Export `events_big` to CSV (100 001 lines) and JSON (valid); cancel mid-export removes the partial file.

## Packaging

- [ ] `pnpm tauri build` produces `Tabler.app` and a `.dmg`; the app launches from /Applications; icon looks right in Dock and Finder (light and dark).
