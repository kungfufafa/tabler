# Tabler — repo guide for Claude

Desktop database client (PostgreSQL, MySQL/MariaDB, SQLite) built to feel like TablePlus.
Tauri v2 + Rust backend (`src-tauri/`), React 19 + TypeScript + Vite 8 + Tailwind v4 + shadcn (radix) renderer (`src/`).

## Commands

- `pnpm tauri dev` — full app (Vite on :1420 + Rust). First Rust build takes minutes; Rust edits restart the app.
- `pnpm dev:mock` — renderer only in a browser with the in-memory IPC mock (fast UI loop; `.claude/launch.json` → `tabler-mock`).
- `pnpm check` — typecheck + lint + format check + vitest + clippy + cargo test.
- `pnpm test` / `pnpm test:rs` — frontend / Rust unit tests. `pnpm test:rs:int` — Rust integration tests against the DBs in `.env`.
- `pnpm dev:db:seed` — (re)create `tabler_dev` on the local Homebrew PostgreSQL/MySQL from `dev/seed`. `pnpm dev:sqlite:build` — build `dev/fixtures/sample.sqlite`.
- `rtk`: the user's global instructions ask to prefix shell commands with `rtk`, but the `rtk` binary is NOT on PATH in this environment. Use plain commands until it is installed.

## Conventions

- Tauri commands (`src-tauri/src/commands/*`) stay thin; logic lives in plain modules (`drivers`, `sqlgen`, `appdb`, `secrets`, `tunnel`) that are unit-testable without Tauri.
- All SQL text that the app generates goes through `src-tauri/src/sqlgen/` (quoting per dialect, bind params). Never format identifiers or literals elsewhere.
- Values crossing IPC use the tagged `Value` enum (`{t, v}`): ints only when within JS safe range, otherwise `bigInt`/`decimal` as strings; timestamps as the DB's textual form.
- Shared types are `#[derive(TS)]` in `src-tauri/src/types/` and exported by `cargo test` into `src/lib/ipc/bindings/` (generated, committed, never hand-edited).
- The renderer talks to the backend only through `src/lib/ipc/` (facade + real/mock transport). Every new command gets a mock implementation.
- `src/components/ui/` is shadcn-generated; do not hand-edit (re-add instead).
- Pin `tauri = "2"` and `@tauri-apps/*@^2` (3.0 alpha exists). Keep the template's TypeScript version.

## Dev databases

- Homebrew PostgreSQL 17 on 127.0.0.1:5432 and MySQL 9.6 on 127.0.0.1:3306 (both already running); credentials in `.env` (copy from `.env.example`).
- Optional `dev/docker-compose.yml` (needs `colima start` + `brew install docker-compose`): pg :54329, mysql :33069, mariadb :33079, openssh :2222 for SSH tunnel tests.
- Secrets: Keychain service `tabler`; set `TABLER_SECRETS_BACKEND=file` in `.env` during dev to avoid repeated Keychain prompts.

## Milestones

- [x] M0 toolchain + scaffold + empty shell
- [x] M1 app store + secrets + profiles + connect + sidebar (drivers: sqlite, postgres, mysql)
- [x] M2 read-only data grid (virtualized grid, sort/filter/paging, inspector, copy formats, FK jump)
- [x] M3 inline editing + staging + Preview SQL (commit ⌘S, discard ⌘⇧Z, undo ⌘Z, NULL/DEFAULT, new/duplicate/delete rows)
- [x] M4 SQL editor (CodeMirror, streaming multi-result runs, cancel, explain, history, .sql files)
- [x] M5 structure + DDL (staged column/index/FK edits, preview/apply, create/rename/truncate/drop)
- [x] M6 tunnel + export + palette + settings + polish
- [x] M7 packaging + docs (icon, production CSP, `pnpm tauri build` → Tabler.app + DMG, README, docs/qa-macos.md)
