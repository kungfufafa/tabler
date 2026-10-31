<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="design/logo/tabler-logo-horizontal-dark.svg">
    <img src="design/logo/tabler-logo-horizontal.svg" alt="Tabler" width="320">
  </picture>
</p>

<p align="center">
  A fast, keyboard-first desktop database client for <b>PostgreSQL</b>, <b>MySQL / MariaDB</b> and
  <b>SQLite</b>, with <b>SQL Server</b>, <b>LibSQL</b> (Turso) and <b>Cloudflare D1</b> too.<br>
  It looks and works like TablePlus, so your muscle memory carries over.
</p>

<p align="center">
  macOS 12+ · Tauri 2 + Rust · React + TypeScript · version 0.1.0
</p>

---

**Contents:** [Highlights](#highlights) · [Features](#features) ·
[Getting started](#getting-started) · [Keyboard shortcuts](#keyboard-shortcuts) ·
[Development](#development) · [Building the app](#building-the-app) ·
[Troubleshooting](#troubleshooting) · [Documentation](#documentation)

## Highlights

- **Feels like TablePlus.** Same layout, same menus, same shortcuts.
- **Safe editing.** Changes are staged and highlighted first. Preview the SQL (⇧⌘P), then commit
  everything in one transaction (⌘S), or undo it (⌘Z).
- **Fast with big tables.** A virtualized grid handles 100k+ rows. Sorting and filtering run on the
  server, and query results stream in.
- **Secure connections.** SSL/TLS, SSH tunnels (incl. jump hosts and `~/.ssh/config`), passwords in
  the macOS Keychain, and per-connection Safe Mode.
- **No lost precision.** 64-bit integers and decimals are never rounded, and timestamps keep the
  server's own text form.
- **Everything included.** SQL editor, structure editor, ER diagram, import / export, backup /
  restore, user management and a console log of every statement.

## Features

| Area                | What you get                                                                                                                                                       |
| ------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| **Databases**       | PostgreSQL · MySQL / MariaDB · SQLite · SQL Server · LibSQL (Turso, `sqld`) · Cloudflare D1 (see [docs/drivers](docs/drivers/README.md) for what each engine does) |
| **Connections**     | Profiles with colours, groups and environment tags (Local … Production) · SSL / TLS · SSH tunnels · Keychain passwords · read-only sessions · open from a URL      |
| **Browse**          | Searchable sidebar with pins · virtualized data grid · server-side sort, filters and paging · foreign-key jump · row details panel · Quick Look                    |
| **Edit**            | Type-aware inline editing · paste rows from Excel, Numbers or CSV · Set NULL / DEFAULT · bulk edit · Preview SQL · one-transaction commit · undo                   |
| **SQL editor**      | Schema-aware autocomplete · run current / all / selection · streaming multi-result runs · cancel · EXPLAIN · history · saved queries · parameters                  |
| **Structure**       | Edit columns, indexes, foreign keys, triggers and checks · preview the DDL before it runs · create / rename / truncate / drop tables · ER diagram                  |
| **Import / export** | CSV, JSON, SQL, HTML and Excel export · CSV, JSON and SQL dump import · backup / restore with `pg_dump` and `mysqldump`                                            |
| **Safety**          | Five Safe Mode levels per connection · read-only connections · Touch ID and passcode lock                                                                          |
| **Admin**           | Create and drop databases and schemas · running queries with kill · user management and privileges · Metrics Board                                                 |
| **Workspace**       | Multiple windows and split views · tabs restored at startup · Open Anything (⌘P) · console log (⇧⌘C) · themes · plugins · AI assistant · MCP server                |

Every feature, with its edge cases, is described in **[docs/features.md](docs/features.md)**.

## Getting started

### Requirements

- macOS 12 or newer (Apple Silicon or Intel)
- [Rust](https://rustup.rs) stable, Node 22 and pnpm 11
- For the development databases: a local PostgreSQL and MySQL (e.g. Homebrew) or Docker

### Run the app from source

```bash
# 1. Install the dependencies
pnpm install

# 2. Create your local settings, then edit the database credentials in .env
cp .env.example .env

# 3. Create the sample databases (optional, but handy for trying things out)
pnpm dev:db:seed        # creates tabler_dev on your local PostgreSQL and MySQL
pnpm dev:sqlite:build   # builds dev/fixtures/sample.sqlite

# 4. Start the app (the first Rust build takes a few minutes)
pnpm tauri dev
```

> [!TIP]
> Working on the UI only? Run `pnpm dev:mock` and open http://localhost:1420. It uses an in-memory
> mock backend, so there is no Rust build.

## Keyboard shortcuts

The defaults are the same as TablePlus. These are the ones you will use most:

**Everywhere**

| Key       | Action                       |
| --------- | ---------------------------- |
| ⌘P        | Open Anything                |
| ⌘K / ⇧⌘K  | Switch database / connection |
| ⌘T        | New query tab                |
| ⌘W        | Close tab                    |
| ⌘[ / ⌘]   | Previous / next tab          |
| ⌃⌘[ / ⌃⌘] | Data / structure view        |
| ⌘0        | Toggle sidebar               |
| ⇧⌘C       | Console log                  |
| ⌘,        | Settings                     |

**Data grid**

| Key       | Action                      |
| --------- | --------------------------- |
| ↩ / F2    | Edit cell                   |
| ␣ (Space) | Row details                 |
| ⌘I / ⌘D   | New row / duplicate row     |
| ⌫         | Delete rows                 |
| ⌘F        | Filter bar                  |
| ⇧⌘P       | Preview SQL                 |
| ⌘S        | Commit                      |
| ⇧⌘⌫       | Discard changes             |
| ⌘Z / ⇧⌘Z  | Undo / redo a staged change |

**SQL editor**

| Key      | Action                             |
| -------- | ---------------------------------- |
| ⌘↩       | Run current statement or selection |
| ⇧⌘↩      | Run all                            |
| ⌘.       | Cancel                             |
| ⌥⌘E      | Explain                            |
| ⌘I / ⇧⌘I | Beautify / uglify                  |
| ⌘/       | Toggle comment                     |
| ⌘S / ⌘O  | Save / open a `.sql` file          |

The full list is in [docs/shortcuts.md](docs/shortcuts.md) and in the app under Settings →
Shortcuts, where every key can be remapped.

## Development

### Scripts

| Command                 | What it does                                                             |
| ----------------------- | ------------------------------------------------------------------------ |
| `pnpm tauri dev`        | Run the desktop app                                                      |
| `pnpm dev:mock`         | Run only the UI in a browser, with a mock backend                        |
| `pnpm check`            | All checks: typecheck, lint, format, vitest, clippy, cargo test          |
| `pnpm test`             | UI unit and component tests (vitest)                                     |
| `pnpm test:rs`          | Rust unit tests (integration tests skip themselves without `.env`)       |
| `pnpm test:rs:int`      | Rust integration tests against the databases in `.env`                   |
| `pnpm dev:db:seed`      | (Re)create the sample databases on your local PostgreSQL and MySQL       |
| `pnpm dev:sqlite:build` | Build the sample SQLite file                                             |
| `pnpm tauri build`      | Build `Tabler.app` and a DMG (see [Building the app](#building-the-app)) |

### Sample databases

`pnpm dev:db:seed` creates the same `tabler_dev` database on every engine. It is full of cases worth
testing: foreign keys and triggers, a composite key, a table without a primary key, a 100,000-row
table, every column type, odd names (`"Order Items"`, `CamelCase`), views, a second schema and
edge-case rows (emoji, CJK, RTL, extreme dates, 2⁶³−1).

| Engine     | Default URL (in `.env`)                             |
| ---------- | --------------------------------------------------- |
| PostgreSQL | `postgres://apriansyahrs@127.0.0.1:5432/tabler_dev` |
| MySQL      | `mysql://root@127.0.0.1:3306/tabler_dev`            |
| SQLite     | `dev/fixtures/sample.sqlite`                        |

Prefer Docker? `dev/docker-compose.yml` runs PostgreSQL 17, MySQL 9.7, MariaDB 11.8 and an SSH
server for tunnel tests. See [dev/README.md](dev/README.md).

### Project layout

```
src-tauri/src/        Rust backend
  commands/           thin Tauri commands, one file per area
  drivers/            the DbSession trait + postgres, mysql and sqlite drivers, and the driver lanes (sqlserver, libsql, d1)
  sqlgen/             every SQL string the app generates (quoting, CRUD, DDL plans, EXPLAIN)
  tunnel/             SSH tunnels and host-key trust
  export/             streaming CSV / JSON / SQL writers
  appdb.rs            the app's own SQLite store (profiles, settings, history)
  secrets.rs          Keychain access

src/                  React UI
  features/           one folder per feature (connections, datagrid, editor, structure, …)
  lib/ipc/            typed calls to the backend + generated types + browser mock
  stores/             zustand stores
```

How it fits together:

- The UI talks to the backend only through `src/lib/ipc/`.
- Shared types are written once in Rust and exported to TypeScript (`src/lib/ipc/bindings/`) when
  `cargo test` runs.
- Large data never crosses in one piece: query results stream in chunks of up to 500 rows, table
  pages are bounded, and exports are written by Rust.

Contributor conventions are in [CLAUDE.md](CLAUDE.md).

## Building the app

```bash
pnpm tauri build                 # Tabler.app and a DMG
pnpm tauri build --bundles app   # Tabler.app only, no DMG
```

The output is in `src-tauri/target/release/bundle/` (`macos/Tabler.app` and `dmg/*.dmg`).

> [!NOTE]
> Builds are ad-hoc signed, not notarized (that needs an Apple Developer ID). On another Mac,
> Gatekeeper blocks the first launch. Right-click the app and choose **Open**, or run:
>
> ```bash
> xattr -cr /Applications/Tabler.app
> ```

The signing setup and how to check it with `codesign` are in the release checklist,
[docs/qa-macos.md](docs/qa-macos.md).

### Windows

Needs the Visual Studio C++ build tools and [Strawberry Perl](https://strawberryperl.com) (the
portable zip is enough). SQLCipher has no system OpenSSL to use on Windows, so OpenSSL is built from
source and linked in, and its configure script needs a Windows Perl (the one that comes with Git
does not work). NASM is optional: without it OpenSSL is built without assembly.

```powershell
$env:OPENSSL_SRC_PERL = "C:\path\to\strawberry-perl\perl\bin\perl.exe"   # unless Strawberry Perl is on PATH
pnpm tauri build
```

`src-tauri/tauri.windows.conf.json` switches the bundles to an NSIS setup (`bundle/nsis/*.exe`,
per-user, no admin rights) and an MSI (`bundle/msi/*.msi`); both download WebView2 when it is
missing. The builds are not code-signed, so SmartScreen warns on the first run (**More info** →
**Run anyway**).

### Both platforms from CI

The `build` workflow ([.github/workflows/build.yml](.github/workflows/build.yml)) builds a universal
macOS DMG (Apple Silicon and Intel) and the Windows installers. Run it from the Actions tab, or push
a `v*` tag; the installers are attached to the run as artifacts.

## Troubleshooting

| Problem                                 | Fix                                                                                                                                                                                            |
| --------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Keychain keeps asking for access in dev | Every rebuild changes the app's signature. Set `TABLER_SECRETS_BACKEND=file` in `.env` (dev only: passwords go into the app database) or sign debug builds with a stable self-signed identity. |
| Port 1420 is in use                     | `pnpm tauri dev` and `pnpm dev:mock` both use it. Stop one of them.                                                                                                                            |
| The first build is slow                 | The Rust dependencies build once, then builds are incremental. For UI work, use `pnpm dev:mock`.                                                                                               |
| MySQL 9 login (`caching_sha2_password`) | Supported over TLS and plain TCP. No server changes needed.                                                                                                                                    |

## Documentation

| Document                                             | What it covers                                             |
| ---------------------------------------------------- | ---------------------------------------------------------- |
| [docs/features.md](docs/features.md)                 | Every feature in detail, including edge cases              |
| [docs/shortcuts.md](docs/shortcuts.md)               | Every keyboard shortcut and how it compares with TablePlus |
| [docs/tableplus-parity.md](docs/tableplus-parity.md) | TablePlus parity roadmap and status                        |
| [docs/verify-servers.md](docs/verify-servers.md)     | Checks against real servers (SSL, MariaDB, SSH)            |
| [docs/qa-macos.md](docs/qa-macos.md)                 | Manual release checklist                                   |
| [dev/README.md](dev/README.md)                       | Sample databases and the Docker setup                      |
| [design/logo/README.md](design/logo/README.md)       | Logo, colours and usage rules                              |
| [CLAUDE.md](CLAUDE.md)                               | Contributor conventions                                    |
