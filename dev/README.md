# dev/ — seed databases for Tabler

Deterministic fixtures for manual testing and the Rust integration tests. The same
object names exist on PostgreSQL, MySQL and SQLite so tests can run against any engine.

## What is seeded

| Object                                          | Rows     | Notes                                                                                                                                                                                                                  |
| ----------------------------------------------- | -------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `users`                                         | 61       | enum `role`, bool, numeric(20,6), `big_id` (±2^63, 2^53+1), uuid, json, array/set, blob avatar, 10 000-char `bio`, edge names (emoji, CJK, RTL, `\n`, `''`, padded), extreme `created_at`, `score` -0.0 / 1e308 / NULL |
| `posts`                                         | 200      | FK → users (cascade), enum `status`, `updated_at` trigger, indexes `(user_id)`, `(status, published_at)`                                                                                                               |
| `tags` / `post_tags`                            | 15 / 420 | `post_tags` has a composite PK `(post_id, tag_id)` (`WITHOUT ROWID` on SQLite)                                                                                                                                         |
| `audit_log`                                     | 30       | **no primary key**                                                                                                                                                                                                     |
| `events_big`                                    | 100 000  | generated in SQL with modulo arithmetic (no random), index `(kind, ts)`                                                                                                                                                |
| `types_zoo`                                     | 3        | one column per engine scalar type; row 1 all NULL, row 2 "min", row 3 "max". SQLite: `STRICT` table + `types_zoo_affinity`                                                                                             |
| `"Order Items"`                                 | 10       | table name with a space and capitals, composite PK                                                                                                                                                                     |
| `"CamelCase"`                                   | 5        | mixed-case table, reserved-word columns `select`, `user`, `from`                                                                                                                                                       |
| `active_users`, `post_counts`                   | views    | PG also has materialized view `post_counts_mv`                                                                                                                                                                         |
| `inventory.items` / `inventory.stock_movements` | 20 / 40  | PG: schema `inventory`. MySQL: second database `tabler_inv`. SQLite: not present                                                                                                                                       |

Seed files: `seed/<engine>/001_schema.sql`, `002_data.sql`, `003_big.sql` (events_big), `004_edge_cases.sql`
(types_zoo, odd names, edge rows); SQLite is a single `seed/sqlite/seed.sql`.

## Local servers (default)

Connection URLs live in `.env` (copy `.env.example`). Defaults assume Homebrew PostgreSQL 17
(`postgres://<you>@127.0.0.1:5432/tabler_dev`), Homebrew MySQL (`mysql://root@127.0.0.1:3306/tabler_dev`)
and `dev/fixtures/sample.sqlite`.

```sh
pnpm dev:db:seed          # DROP + CREATE tabler_dev (PG, MySQL) and tabler_inv (MySQL), apply seeds, print counts
sh dev/scripts/db-seed.sh pg       # one engine only (pg | mysql)
pnpm dev:sqlite:build     # rebuild dev/fixtures/sample.sqlite (~12 MB, gitignored)
```

`db-seed.sh` refuses to drop a database whose name does not start with `tabler_`.

## Docker alternative

`dev/docker-compose.yml` runs the same seeds on first start of a fresh volume
(`pnpm dev:db:up` / `pnpm dev:db:reset`). Ports are bound to 127.0.0.1 and differ from the Homebrew ones:

| Service                            | Address           | Credentials                | URL                                                   |
| ---------------------------------- | ----------------- | -------------------------- | ----------------------------------------------------- |
| postgres:17                        | `127.0.0.1:54329` | `tabler` / `tabler`        | `postgres://tabler:tabler@127.0.0.1:54329/tabler_dev` |
| mysql:9.7                          | `127.0.0.1:33069` | `root` / `tabler`          | `mysql://root:tabler@127.0.0.1:33069/tabler_dev`      |
| mariadb:11.8 (`--profile mariadb`) | `127.0.0.1:33079` | `root` / `tabler`          | `mysql://root:tabler@127.0.0.1:33079/tabler_dev`      |
| openssh-server                     | `127.0.0.1:2222`  | `tabler` / `tabler` or key | tunnel target: `postgres:5432`, `mysql:3306`          |

Before the first `up`, create the SSH key the `ssh` service mounts (gitignored):

```sh
ssh-keygen -t ed25519 -N '' -f dev/ssh/tabler_dev
ssh -p 2222 -i dev/ssh/tabler_dev tabler@127.0.0.1 -L 15432:postgres:5432   # manual tunnel check
```

## Engine-specific notes

- MySQL: `email`, `tags.name`, `"Order Items".item` are `varchar` (TEXT cannot carry a UNIQUE/PK index without a prefix);
  `created_at` minimum is `1000-01-01` (DATETIME range); `-0.0` is inserted via `CAST('-0' AS DOUBLE)` because a
  literal `-0.0` is stored as `0`. `003_big.sql` raises the recursive-CTE limit through version-gated comments
  (`/*!80000 SET SESSION cte_max_recursion_depth */` for MySQL, `/*M!100200 SET SESSION max_recursive_iterations */` for MariaDB).
- PostgreSQL: the `0001-01-01` / `9999-12-31` `created_at` rows are stored at noon UTC so the date is the same in every session time zone.
- SQLite: stores integral REALs as integers, so `-0.0` reads back as `0.0`; `uid` of generated users is a deterministic v4-shaped UUID.
