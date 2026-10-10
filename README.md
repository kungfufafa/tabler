# Tabler

A desktop database client for PostgreSQL, MySQL/MariaDB and SQLite, built to feel as neat and
comfortable as TablePlus. Tauri v2 + Rust backend, React + TypeScript renderer.

## Requirements

- macOS 12+ (Apple Silicon or Intel)
- Rust stable (`rustup`), Node 22, pnpm 11
- Local PostgreSQL / MySQL servers for the dev databases (Homebrew), or Docker via `dev/docker-compose.yml`

## Quick start

```bash
pnpm install
cp .env.example .env        # adjust local DB credentials
pnpm tauri dev              # first Rust build takes a few minutes
```

Renderer-only loop with a mocked backend: `pnpm dev:mock` (http://localhost:1420).

## Scripts

| Script                                          | What it does                                                  |
| ----------------------------------------------- | ------------------------------------------------------------- |
| `pnpm tauri dev` / `pnpm tauri build`           | run / package the desktop app                                 |
| `pnpm dev:mock`                                 | Vite only, in-memory IPC mock                                 |
| `pnpm check`                                    | typecheck, lint, format check, vitest, clippy, cargo test     |
| `pnpm test`, `pnpm test:rs`, `pnpm test:rs:int` | frontend unit, Rust unit, Rust integration (needs `.env` DBs) |
| `pnpm dev:db:seed`, `pnpm dev:sqlite:build`     | seed `tabler_dev` on local PG/MySQL, build the SQLite fixture |

See `CLAUDE.md` for architecture conventions.
