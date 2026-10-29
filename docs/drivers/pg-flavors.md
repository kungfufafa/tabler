# PostgreSQL flavors: lane brief

> Seeded by the foundation (step F3b) from the driver plan and the research notes. The lane keeps it current and turns it into the driver's final doc before it lands: what works, the limits, how it was tested. Rules, tooling and the definition of done are in [README.md](README.md).

|                   |                                                                                                            |
| ----------------- | ---------------------------------------------------------------------------------------------------------- |
| Lane              | `pg-flavors`                                                                                               |
| Databases         | CockroachDB, Greenplum, Amazon Redshift                                                                    |
| Drivers           | `postgres` with `pg_flavor`                                                                                |
| Picker kinds      | `cockroachdb`, `greenplum`, `redshift`                                                                     |
| SQL dialect       | `redshift`                                                                                                 |
| Cargo features    | `drv-pg-flavors`                                                                                           |
| Home slot         | `s1`                                                                                                       |
| Bindings prefix   | `Cockroach*`, `Greenplum*`, `Redshift*`                                                                    |
| Lib test filters  | `drivers::postgres::flavor`, `sqlgen::dialect::redshift`, `sqlgen::portable::generated::redshift::`        |
| Real-server suite | `tests/pg_flavors.rs` with `TABLER_TEST_CRDB_URL`, `TABLER_TEST_GREENPLUM_URL`, `TABLER_TEST_REDSHIFT_URL` |

## Owned files

Everything the lane may change (dev/lanes.json is authoritative):

- `src-tauri/src/drivers/postgres/flavor/imp/**`
- `src-tauri/src/sqlgen/dialect/redshift.rs`
- `src-tauri/src/sqlgen/portable/gen/redshift.rs`
- `src-tauri/tests/pg_flavors.rs`
- `src-tauri/tests/fixtures/pg_flavors/**`
- `src-tauri/tests/fixtures/classify-generated/redshift.json`
- `src/features/drivers/pg-flavors/**`
- `src/lib/ipc/mock/drivers/pg-flavors/**`
- `src/lib/ipc/bindings/Cockroach*.ts`
- `src/lib/ipc/bindings/Greenplum*.ts`
- `src/lib/ipc/bindings/Redshift*.ts`
- `src/lib/ipc/bindings/lex/redshift.json`
- `docs/drivers/pg-flavors.md`

Anything else (a hook, a cap, a descriptor value, a dependency, a feature) is a change request to the lead. `node dev/scripts/lane-commit.mjs pg-flavors -m "pg-flavors: …"` refuses other paths.

## Scope of v1

### CockroachDB

- **Identity:** driver `postgres` + `pg_flavor: cockroachdb`, `SqlDialect::Postgres`. Crates: none.
- **UI:** SQL grid.
- **Auth v1:** password (SCRAM), client certificate, JWT (password plus `options=--crdb:jwt_auth_enabled=true`), Command.
- **Required fields:** host, user (defaults: 26257, `root`, `defaultdb`).
- **Out of scope v1:** GSSAPI, pg_dump backup, keyless (ctid) edits, multi-region UI.
- **Test on this machine:** `fake_pg` version sniff; hook unit tests with recorded catalog rows. Opt-in: `cockroach.exe start-single-node --insecure` (experimental Windows binary; approval), `TABLER_TEST_CRDB_URL`.

### Greenplum

- **Identity:** `postgres` + `greenplum`, `SqlDialect::Postgres`. Crates: none.
- **UI:** SQL grid.
- **Auth v1:** password (MD5 or SCRAM), client certificate, Command.
- **Required fields:** host, user (5432, `postgres`).
- **Out of scope v1:** Kerberos, gpbackup and backup, distribution and partition _editing_ (shown read-only), external-table DDL.
- **Test:** `server_version_num`-gated catalog tests (90426 / 120012 / 140000); `fake_pg`. No local server: Docker only.

### Redshift

- **Identity:** `postgres` + `redshift`, `SqlDialect::Redshift` (ext). Crates: none.
- **UI:** SQL grid.
- **Auth v1:** password (MD5), IAM via Command (`aws redshift get-cluster-credentials` / `redshift-serverless get-credentials`).
- **Required fields:** host, user (5439, `dev`).
- **Out of scope v1:** sha256 password auth (request 13), SAML/OIDC, Data API, DDL editing, admin/users, backup.
- **Test:** `fake_pg` that rejects `SET IntervalStyle` / `extra_float_digits=3`; assertions that the spec emits no RETURNING, ON CONFLICT, IS DISTINCT FROM, `::text` or SAVEPOINT. Opt-in: Serverless trial.

## Descriptor values

Seeded from the plan (§5.2). The values in force are the descriptors in `src-tauri/src/types/driver.rs` (keepalive belongs to the session's caps); a change is a request to the lead.

| Driver   | Location | Port (TLS)                        | SSH    | TLS style | Secret label | Command TTL |
| -------- | -------- | --------------------------------- | ------ | --------- | ------------ | ----------- |
| postgres | Host     | 5432 (kinds: 26257 / 5432 / 5439) | Tunnel | Libpq     | Password     | 600         |

| Driver   | Required fields / params | Label overrides | Keepalive | Read-only | Sharing | Token secret |
| -------- | ------------------------ | --------------- | --------- | --------- | ------- | ------------ |
| postgres | host, user               | —               | 30        | Server    | Indep   | no           |

## Read-only and hardening

| Engine                 | When read-only                                                                       | When unattended (MCP) |
| ---------------------- | ------------------------------------------------------------------------------------ | --------------------- |
| CockroachDB, Greenplum | `default_transaction_read_only = on` (existing PG init)                              | —                     |
| Redshift               | ClassifierOnly (the lane may add `BEGIN READ ONLY` on the editor after verifying it) | —                     |

Every classifier is default-deny: a statement or command it does not know is a write. The read-only mode in the caps (`Server`, `PerRequest` or `ClassifierOnly`) must say what the session really enforces.

## What to implement

- No `entry.rs` of its own: the PostgreSQL driver opens these databases. The lane implements `PgFlavorHooks` (plan §2.9) in `drivers/postgres/flavor/imp/**` once F8b has put the seam into `drivers/postgres`; it never edits `drivers/postgres/*.rs` itself.
- Hooks: session init, introspection and process-list SQL, kill, `table_ddl`, `explain_form`, `adjust_caps` (the caps it proves), `sql_dialect` (Redshift).
- `sqlgen/dialect/redshift.rs`: Redshift's `DialectSpec` and `LexProfile`; the `export_bindings_lex_redshift` test.
- Renderer: the three tiles' `KindUi` (the PostgreSQL form with flavor defaults) and a `MockDriver`.

## The seam (F8b)

What the PostgreSQL driver gives the lane, in `drivers/postgres/flavor/mod.rs` (the lane fills `flavor/imp/**` only):

- `imp::hooks(PgFlavor) -> Option<&'static dyn PgFlavorHooks>` is the lane's entry point (a stub answering `None` until then); `flavor::hooks` falls back to `VANILLA`, plain PostgreSQL, which changes nothing.
- At connect the session asks the editor connection `SELECT version()` (one simple-protocol round trip, before the session init) and reads `server_version` from the startup parameters (`server_version_num`, as libpq computes it). The engine `detect` finds there overrides the profile's `pg_flavor` (with a log line); its hooks are picked once and kept for the session. The profile's engine alone picks `default_database`, which is needed before connecting.
- `init_sql` gets the PostgreSQL init statements (`connect::session_init_statements`, since F8e each an `InitStatement` with the `InitKind` it sets: a hook matches the kind, never the text) and whether the session must refuse writes; `init_one_by_one` sends them one at a time, on the editor and on each pool connection.
- The catalog hooks (`list_databases`, `list_schemas`, `list_tables`, `table_meta`, `view_definition`, `completion_metadata`) run before the `pg_catalog` queries; `None` falls through to them. They get a `Traced` worker client (its queries show in the console log) with `server_version_num()` for version-gated SQL (Greenplum 6 / 7, Cloudberry); the core `introspect::table_meta` gates its own (F8e), and `introspect::table_meta_of` reads a relation an engine looked up itself.
- `list_processes`, `kill`, `table_ddl` and `explain_form` answer `DbSession`'s hooks of the same names; `None` keeps the callers' core SQL (`admin.rs`, `commands/ddl.rs`, `commands/query.rs`).
- `adjust_caps(&mut caps, server_version_num)` grants back from the restricted caps; `sql_dialect` is what `DbSession::dialect` reports; `retry_serialization` makes `DbSession::transaction` run a transaction aborted with 40001 again (up to 3 attempts); `row_cap_savepoint` (F8e) false keeps the row-cap SAVEPOINT off a read inside a transaction block, which is then drained.
- Byte-identity of plain PostgreSQL is pinned by `drivers::postgres::tests::plain_postgres_gets_the_same_sql` (golden `src-tauri/tests/fixtures/postgres/vanilla.txt`: the connect sequence and every catalog query as `FakePg` received them) and `connect::tests::the_session_init_stays_byte_identical`. The only change F8b made there is the `SELECT version()` before the init.
- **Pending: a real-server check.** F8b was verified against `FakePg` only: this Windows machine has no PostgreSQL yet. Before the seam is pushed, run the `TABLER_TEST_PG_URL` suites (the `drivers::postgres` lib tests and `src-tauri/tests/postgres.rs` with the other PostgreSQL suites, see `docs/verify-servers.md`) against a PostgreSQL the user approves installing, the Mac, or a CI job with a postgres service (critique A12).

## Phase A status

Done (`drivers/postgres/flavor/imp/**`, `sqlgen/dialect/redshift.rs`, `sqlgen/portable/gen/redshift.rs`, `tests/pg_flavors.rs`), tested on `FakePg` with recorded catalog rows (`lane.mjs pg-flavors test`: 41 tests, plus the generated suite's cases and 3,000 fuzz scripts):

- **Hooks** (`imp::hooks`): one `PgFlavorHooks` per engine; the shared catalog helpers are in `imp/catalog.rs`, the test helpers (a session through the real `PgSession::open`, the core `table_meta` rows) in `imp/testkit.rs`.
- **CockroachDB** (`imp/cockroach.rs`): `defaultdb`; init one statement at a time without DateStyle / IntervalStyle (older versions refuse them unless enabled; a non-UTF-8 client encoding is refused by the server, so the connect fails); `crdb_internal` and `pg_extension` hidden; tables with row estimates from `crdb_internal.table_row_statistics` (the plain list on a server without it) and no sizes; `table_meta` is PostgreSQL's plus `information_schema.columns` (`is_hidden`: the implicit `rowid` key stays the key but is generated and read-only; `crdb_sql_type`: the modifier `format_type` drops, so `numeric(10,2)` and `character varying(10)` with their lengths); `unique_rowid()` / `gen_random_uuid()` defaults count as auto-increment; sessions from `crdb_internal.cluster_sessions` (string ids, the listing one found by `SHOW session_id`, none marked on a server without that setting), kills by `CANCEL QUERIES IF EXISTS …` / `CANCEL SESSION $1` (ids checked as hex, bound); DDL from `crdb_internal.create_statements`; EXPLAIN `(VERBOSE)` and `ANALYZE`; transactions run again on 40001. Caps: KillProcess and Diagram (since F8e the diagram reads the NULL `relispartition` with `IS NOT TRUE`) granted; EditWithoutKey (no `ctid`), Encoding and CreateDatabase (no collations or templates) removed.
- **Greenplum** (`imp/greenplum.rs`): catalog SQL by `server_version_num` (tested at 90426, 120012, 140004): Greenplum 6 gets PostgreSQL's queries without `attidentity`, `attgenerated` and `indnkeyatts` (since F8e the core `table_meta` gates them on the version itself: this file reads only the relation, `legacy_table_meta`), its own process list (`waiting` / `waiting_reason`, no `backend_type`), legacy partition children hidden (`pg_partition_rule`) and parents shown as partitioned, external tables (`relstorage = 'x'`) as foreign tables; 7 and Cloudberry run PostgreSQL's `table_meta` and process list, with partitions hidden (`relispartition`). No table sizes (they ask every segment). Distribution (`pg_get_table_distributedby`) and storage (`relstorage` / the access method, with `reloptions`) read-only in `TableMeta.options` (`row_format`, `engine`: see change requests) and in the DDL, which is PostgreSQL's reconstruction plus `USING am` (7 on), `WITH (…)`, `DISTRIBUTED …` and the partitioning (Greenplum 6's legacy clause, or the declarative key); views keep the core DDL, an external table's is refused. Caps: KillProcess granted (`pg_cancel_backend` by pid); EditWithoutKey removed (`ctid` is per segment); on Greenplum 6 also Sequences, Diagram, CreateDatabase (PostgreSQL 10 catalogs) and SessionDetails (`pg_stat_ssl`), and the import's upsert (`DataCaps::upsert`: no ON CONFLICT before 9.5).
- **Redshift** (`imp/redshift.rs`): `dev`; init without IntervalStyle, client_encoding and default_transaction_read_only, `extra_float_digits = 2`, one at a time; read-only is ClassifierOnly; catalog SQL without `string_agg`, `unnest`, `LATERAL`, `WITH ORDINALITY`, `typcategory` or `::text` (a test checks every statement), the `svv_` views in queries of their own; databases without `padb_harvest`; tables with `svv_table_info` rows and sizes, `svv_mv_info` materialized views (also in `table_meta`, where `pg_class` calls them views) and `svv_external_tables` Spectrum tables (an unreadable view leaves the list as is); columns with DISTKEY / SORTKEY / ENCODE in `extra`, IDENTITY as auto-increment, DISTSTYLE in `TableOptions.row_format`; keys from `pg_get_constraintdef` (informational), unique constraints as unique indexes; late-binding view columns from `pg_get_late_binding_view_cols()`; Spectrum columns from `svv_external_columns`; DDL from `SHOW TABLE` / `SHOW VIEW` / `SHOW EXTERNAL TABLE`; sessions from `stv_sessions` + `stv_recents`, on Serverless `sys_session_history` + `sys_query_history`; kills by `pg_cancel_backend` / `pg_terminate_backend`. Caps from `minimal`: PostgreSQL's namespaces and grid caps without JSON containment, DEFAULT in UPDATE or LOCALTIMESTAMP-style expressions, and Explain (plan only), Format, Edit/Insert/DeleteRows (primary key only), ServerExpressions (`GETDATE()`), ExactCount, TableDdl, ViewDefinition, SwitchDatabase, ProcessList, KillProcess, Timezone, StartupCommands; no keep-alive.
- **Redshift SQL** (`sqlgen/dialect/redshift.rs`): strings read both with and without backslash escapes (escapes first, as the server does), no `E'…'` form (PostgreSQL 8.1's; the string after the `E` is read both ways), `$$` / `$tag$` bodies, nested comments; reads are SELECT, WITH, SHOW, EXPLAIN (no VALUES statement); `write_calls` (also its `side_effect_functions`) holds AWS's system administration functions that change the server (`pg_terminate_backend`, `pg_cancel_backend`, `set_config`, `change_query_priority`, `change_session_priority`, `change_user_priority`, `reboot_cluster`; `current_setting` reads) and the large-object functions (`lo_*`, `lowrite`), `pg_start_backup` / `pg_stop_backup`, `pg_stat_reset*`, the generic file access functions (`pg_read_file`, `pg_read_binary_file`, `pg_stat_file`, `pg_ls_*`), `pg_reload_conf` / `pg_rotate_logfile` and `nextval` / `setval`, which the leader node may still have (AWS lists them as unsupported, which on the leader node need not mean an error); the ones that take one argument are also words of `writes`, since PostgreSQL 8.0's parser calls `name(expr)` for `(expr).name` (attribute notation: `SELECT (123).pg_terminate_backend`), spelt out where a column may share a prefix (`lo_size` stays a read); since F8e `LEX.attribute_calls` makes any name in attribute notation a call, a quoted one too (Redshift folds it to lower case), so these words only add false writes and may go; values go in through their column's type (`$1::varchar(65535)` for text, the full `numeric(p,s)`, `JSON_PARSE($1)` for SUPER); the grid reads booleans, integers, floats, dates and text as sent, other types as `::varchar(65535)`, SUPER through JSON_SERIALIZE, GEOMETRY through ST_AsEWKT and VARBYTE through TO_HEX (those three not editable or fetched whole), HLLSKETCH as sent; a test builds a browse, count, fetch, update, inserts and a delete and finds no RETURNING, ON CONFLICT, IS DISTINCT FROM, `::text`, SAVEPOINT or CURRENT_TIMESTAMP; edits in Redshift SQL run on the fake Redshift, which refuses those, and a delete that hits two rows is rolled back.
- **Generated classifier suite** complete (`suite().check()`, `suite().fuzz(3000)`), fixture `tests/fixtures/classify-generated/redshift.json` blessed: the hard cases (UNLOAD with its query in a string, COPY from S3, CALL, `pg_terminate_backend` / `pg_cancel_backend` / `set_config` in every spelling, `reboot_cluster` and `change_*_priority`, `lowrite`, the file access, signal and sequence functions, the one-argument calls in attribute notation, quoted too (`(123).pg_terminate_backend`, `(1)."PG_TERMINATE_BACKEND"`, `('/etc/passwd').lo_import`, across a line break or a comment before the `.`), CREATE TEMP TABLE … AS, SELECT … INTO including `#temp`, dollar bodies as inert contexts, backslash strings read both ways) plus Redshift's own contexts (casts, TOP, APPROXIMATE, QUALIFY, LISTAGG … WITHIN GROUP, PIVOT, SUPER paths, PartiQL FROM, `#` temp tables).
- **Real-server suite** `tests/pg_flavors.rs`: per engine connect, list, browse with paging, count, fetch, a commit with one failing change, sessions, DDL; CockroachDB's hidden `rowid` and modifiers; cancel and statement timeout (CockroachDB, Greenplum); a Redshift SQL edit and a SUPER column. Not run yet (no server here).

Change requests to the lead (shared files). F8e did 1, 3, 4, 5, 7, 8 and 10 (and changed this lane's `imp/catalog.rs`, `imp/cockroach.rs`, `imp/greenplum.rs`, `imp/redshift.rs`, `dialect/redshift.rs`, `gen/redshift.rs`, the blessed fixture and `lex/redshift.json` for them); 2 is assigned to F9b2, 6 to F10, and 9 is Phase B (README, Change requests):

1. `drivers/postgres/introspect.rs`: gate `table_meta`'s column and index SQL on `Traced::server_version_num` (`attidentity` from 10, `attgenerated` from 12, `indnkeyatts` from 11), which also fixes plain PostgreSQL 9.4–11; then Greenplum 6 needs no copy of it (`imp/greenplum.rs::legacy_table_meta`). Until then, make `typmod_info`, `fk_action`, `kind_of_relkind` and `first_char` `pub(crate)` so `imp/catalog.rs` drops its copies.
2. `types/schema.rs::TableOptions`: fields for an MPP table (`distribution`, `storage`, `sort_key`), shown by the Structure tab; the lane fills `row_format` (distribution, DISTSTYLE) and `engine` (storage and options) meanwhile.
3. `sqlgen/diagram.rs`: `c.relispartition IS NOT TRUE` instead of `NOT c.relispartition` (NULL on CockroachDB, so the diagram is empty); then CockroachDB gets Diagram back.
4. A cap (or `DataCaps` flag) for the import's upsert (`ON CONFLICT`), which Greenplum 6 lacks; today only the server's error stops it.
5. `drivers/postgres/connect.rs`: tag the init statements (date style, interval style, float digits, encoding, time zone, read-only, settings, startup) instead of the hooks matching their text.
6. `export::table_query` through the dialect facade (`sql::select_on`), so a Redshift session can be granted ExportTables; and `PgFlavorHooks` hooks for routines, users and session details (Redshift: `pg_proc_info` / `SHOW PROCEDURE`, `svv_user_info`), Phase B.
7. `FakePg` (`drivers/postgres/testing.rs`): columns of type `"char"`, numeric and timestamp, and declared parameter types, so recorded rows can keep the server's types.
8. **Redshift correctness, before a Redshift session ships:** `drivers/postgres/mod.rs::run_on_editor` sends `SAVEPOINT tabler_row_cap` before a capped read inside a transaction block (`wants_savepoint`). Redshift has no savepoints: the statement fails and aborts the user's open transaction, so any SELECT after BEGIN in the editor breaks it. Add a `PgFlavorHooks::row_cap_savepoint() -> bool` (default true, Redshift false; or a check of `sql_dialect() == SqlDialect::Redshift`) and fold it into `wants_savepoint`; without the savepoint `run_on_editor` already passes no canceller inside a transaction, so a capped read there is drained instead of cancelled. The lane's fake Redshift refuses SAVEPOINT, so a test can then run BEGIN, a capped SELECT and COMMIT on it.
9. `sqlgen/ddl.rs::PgTableContext`: fields for the clauses an MPP table adds after its column list (an access method, the storage options, the distribution, Greenplum 6's legacy partition clause), written by `table_parts`, so `imp/greenplum.rs::table_ddl` stops appending `USING …`, `WITH (…)`, `DISTRIBUTED …` and the partitioning itself. The DDL is display-only and the texts are the server's own (`pg_get_table_distributedby`, `reloptions`, the partition definition) with the access method quoted by `quote_ident`, so this is tidiness under the sqlgen rule, not a fix.
10. **Classifier, attribute notation (safety):** PostgreSQL's parser (8.0's `transformIndirection`, still so in 17) sends `(expr).name` to `ParseFuncOrColumn(…, is_column = true)`, which calls `name(expr)` when `expr` has no field `name`. `sqlgen/portable/classify.rs::calls` (and so `calls_listed_function`, the Metrics Board's check) sees a call only before `(`, so a quoted name there reads: `SELECT (1)."pg_terminate_backend"` is a READ for Redshift, and Redshift folds quoted names to lower case unless `enable_case_sensitive_identifier` is on, so `(1)."PG_TERMINATE_BACKEND"` calls it too. The lane's words of `writes` cover the unquoted form only, and `calls_side_effect_function` misses even that (the board still refuses it, since the classifier calls it a write first). Fix: in `calls`, a callee (word or quoted name) after `)` and `.` is a call of it, for the profiles of the PostgreSQL family (a flag on `LexProfile` or `ClassifyRules`); `dialect::redshift`'s `documented_holes_read_until_their_change_requests` lists the cases that then turn into writes. The same hole is in the core `sqlgen/classify.rs::calls_side_effect_function` for PostgreSQL itself (so also CockroachDB and Greenplum): `SELECT (1).pg_terminate_backend` and `SELECT ('s').nextval` are reads there with no side-effect call (checked on the snapshot), so the Metrics Board runs them.

Left for Phase B: the renderer (`KindUi` of the three tiles, the mock driver), the real-server runs (and recorded catalog rows from them), the red-team review of the Redshift classifier, the change requests above, and on a real server: Redshift's `SET TIME ZONE`, binary results of its types, CancelRequest (then CancelOnServer), `SET DEFAULT` in UPDATE, BEGIN READ ONLY for a read-only editor; CockroachDB's `CANCEL` with a bound id, `autocommit_before_ddl` (then structure editing as DataOnly); Greenplum 6's EXPLAIN options and `pg_get_partition_def`.

## Decisions and corrections for this lane

- Starts after F8b, which puts `PgFlavorHooks` and `VANILLA` into the ungated `drivers/postgres/flavor/mod.rs`: there is nothing to implement before (critique B3). It still lands first (plan §6.1).
- CockroachDB and Greenplum sessions report `SqlDialect::Postgres` and differ from PostgreSQL only in caps and hooks; Redshift reports `SqlDialect::Redshift`, an extension dialect that lexes like PostgreSQL (plan A6).
- Every session with `pg_flavor` set or sniffed loses StructureEdit, Backup, Restore, Users and KillProcess (`flavor::adjust_caps`, applied before the engine hooks run), and without the lane a sniffed Redshift is refused as not built. The lane grants back, in its `adjust_caps`, only what it proves: GP6 (PostgreSQL 9.4) lacks identity columns, `ADD COLUMN IF NOT EXISTS` and `ON CONFLICT`; CockroachDB DDL inside a transaction is not atomic (XXA00), so its structure apply is DataOnly or out of scope (critique D7).
- Redshift's text projection is per column: `::varchar(65535)` fails on GEOMETRY and HLLSKETCH and truncates SUPER, so such columns are not editable and not fetched whole (critique D3).
- PostgreSQL keeps its own TLS; the lane's tests use `FakePg` (`drivers/postgres/testing.rs`, moved there in F8b; critique A12, B4).
- Redshift: no keep-alive (`keepalive_secs: None`) and a free `ping` (plan §5.2).

## Definition of done (this lane)

Beyond the shared list in [README.md](README.md#definition-of-done-v1):

- **Classifier hardening (blocks landing).** `src-tauri/src/sqlgen/portable/gen/redshift.rs` is complete and `src-tauri/tests/fixtures/classify-generated/redshift.json` blessed, as [Classifier hardening](README.md#classifier-hardening) requires: every write keyword, write call and read starter of `dialect::redshift::SPEC` in every shared context and in the contexts only this grammar has, an engine-realistic read corpus, its write forms and fuzz atoms, `suite().check()` and `suite().fuzz(…)` in place of the placeholder. Then a red-team review passes, each finding fixed and kept as a case. Known hard cases: `UNLOAD ('SELECT …') TO 's3://…'` (a write whose query is a string) and `COPY … FROM 's3://…'`; `CALL`; `pg_terminate_backend` / `pg_cancel_backend` / `set_config`; `CREATE TEMP TABLE … AS` and `SELECT … INTO`; dollar-quoted bodies as inert contexts. CockroachDB and Greenplum use the core PostgreSQL classifier, which this suite does not cover.

## Research notes

> Background from the driver research (October 2026), with file references and no line numbers. Verify against the code; where it differs from the scope above or from README.md, those win.

### CockroachDB

**Approach.**

RECOMMENDATION (applies to CockroachDB, Greenplum and Redshift): treat each as a flavor of the existing postgres driver, not as a new Driver variant. Add `PgFlavor {Postgres, Cockroachdb, Greenplum, Redshift}` as `ProfileOptions.pg_flavor: Option<PgFlavor>`. This copies the MariaDB pattern: `mysql_version: Option<MysqlVersion>` (src-tauri/src/types/profile.rs) plus a separate picker tile and badge through `DatabaseKind` / `databaseKind()` / `kindDriver()` (src/features/connections/colors.ts). At connect, PgSession::open (drivers/postgres/mod.rs) runs `SELECT version()` (CockroachDB returns 'CockroachDB CCL v…') to confirm or correct the flavor and stores it on PgSession. Expose it as `DbSession::pg_flavor()` next to `is_mariadb()` (drivers/mod.rs) and on ConnectionInfo (types/profile.rs) so the renderer can hide unsupported features. Keep Dialect::Postgres, because quoting and literals are identical (sqlgen/quote.rs). Pass a capability struct into the sqlgen functions that differ, the same way explain_prefix already takes `mariadb: bool` (sqlgen/explain.rs). Also make Driver::default_port (types/profile.rs, used by ConnectTarget::port at drivers/mod.rs) and the default dbname 'postgres' (postgres/connect.rs) flavor-aware. Why not a new Driver variant: `Driver::` appears at 172 non-test sites in 37 Rust files, and the string "postgres" at about 270 sites in 76 renderer files (for example url.ts). Almost all of those (pool, editor, cancel, tunnel, export, import, safety) are already correct for CockroachDB, Greenplum and Redshift. Vertica is the exception and gets its own Driver (see its entry). Protocol: the existing tokio-postgres stack is used unchanged. Cockroach Labs rates rust-postgres 0.19.x (a sync wrapper over tokio-postgres) and sqlx 0.8.6 as 'Partial' support. CockroachDB-specific work: (1) session init; (2) introspection patches for hidden columns and for NULL reltuples/relispartition; (3) process list and kill through crdb_internal; (4) grants through SHOW GRANTS; (5) EXPLAIN options; (6) DDL text from SHOW CREATE TABLE; (7) unique_rowid() serial columns; (8) retrying transactions on SQLSTATE 40001.

**Windows build.**

No new crates and no C dependencies. The pure-Rust stack in Cargo.toml already builds on MSVC: tokio-postgres 0.7.18 (released 2026-06-12, the newest), postgres-protocol 0.6.12, tokio-postgres-rustls 0.14.0 (2026-05-21), deadpool-postgres 0.14.2 (2026-08-26), rustls 0.23.45 with aws-lc-rs 1.18.1, and rustls-native-certs 0.8 for the Windows certificate store. No OpenSSL is needed, and the binary grows only by the flavor SQL strings. CockroachDB Cloud certificates chain to public roots, so verify-full works with native roots. Self-hosted secure clusters need ca.crt as the CA file plus client.\<user>.crt and client.\<user>.key in PEM form, which tls_files::load_client_certificate already loads.

**Authentication.**

TablePlus uses a PostgreSQL-style form for CockroachDB. Its Windows changelog (build 178) groups ALTER statements for CockroachDB. Open TablePlus issues #2546 (Cloud needs extra query parameters) and #3709 (JWT auth request) show that TablePlus offers only password and client certificates. Tabler should offer four methods: (1) Password. CockroachDB stores SCRAM-SHA-256 by default and accepts cleartext over TLS for LDAP-backed users. tokio-postgres connect_raw supports SCRAM-SHA-256(-PLUS), MD5 and cleartext. CockroachDB has no channel binding, and ChannelBinding::Prefer falls back cleanly. (2) TLS client certificate (mTLS `cert` auth) through the existing ssl_key_path / ssl_cert_path fields (types/profile.rs). (3) JWT: the token goes in the password field, plus `options=--crdb:jwt_auth_enabled=true` through the existing pg_params `options` key (connect.rs). (4) The 'Command Line' password mode for short-lived tokens. CommandPassword already re-runs the command for new pool connections (connect.rs). GSSAPI/Kerberos is not possible, because tokio-postgres rejects AuthenticationGss.

**Data model.**

Same tree as PostgreSQL: cluster → databases → schemas → objects. Databases come from pg_database. Switching database reconnects as it does today (CockroachDB also accepts `USE db`). Schemas are public and user schemas; the system schemas are pg_catalog, information_schema, crdb_internal and pg_extension. Objects are tables, views and materialized views (relkind r / v / m), sequences (S), enums, and UDFs/procedures (pg_proc). There are no inheritance or partition children: pg_inherits is empty, and CockroachDB's PARTITION BY applies to primary-key ranges with zone configs, so the sidebar never shows partitions. Every table has a primary key. A table created without one gets a hidden `rowid INT8 DEFAULT unique_rowid()` key. TablePlus presents CockroachDB like PostgreSQL: database switcher, schema list, and a Structure tab that shows indexes including their STORING columns (TablePlus #3944 is a bug report about that display).

**Query language.**

CockroachDB SQL is the PostgreSQL dialect plus extensions: UPSERT; SHOW TABLES / COLUMNS / INDEXES / CREATE / GRANTS / SESSIONS / STATEMENTS / JOBS; AS OF SYSTEM TIME; IMPORT INTO; EXPORT; CHANGEFEED; CANCEL QUERY / SESSION / JOB; multi-region DDL. The editor sends user SQL unchanged through the existing path: Parse/Describe first (editor::describe, editor.rs), then the simple-query run (editor::stream_simple). All of Tabler's generated SQL is accepted: $n::type binds, ::text casts, ILIKE … ESCAPE, NULLS FIRST/LAST, IS NOT DISTINCT FROM, RETURNING and ON CONFLICT.

**Editing.**

Row editing works with today's sqlgen: WHERE by primary key, `RETURNING col::text` (sqlgen/mutate.rs), and INSERT … ON CONFLICT upserts (sqlgen/transfer.rs). A table without a declared primary key uses the hidden rowid key. That needs attishidden handling, so rowid stays the edit key without being shown as an editable column. The ctid row locator (mutate.rs) must never be used, because CockroachDB has no ctid. `transaction()` (postgres/mod.rs) must retry the whole batch on SQLSTATE 40001, because serializable restarts are normal on CockroachDB. DDL uses PostgreSQL syntax: CREATE/ALTER TABLE, ADD/DROP/RENAME COLUMN, ALTER COLUMN TYPE, CREATE INDEX … STORING, COMMENT ON, CREATE TYPE … AS ENUM, sequences, views, materialized views (REFRESH), functions and procedures. However, the session variable `autocommit_before_ddl` defaults to on, so a staged structure change applied inside one transaction is committed statement by statement. Either send `SET autocommit_before_ddl = off` in session init, or present CockroachDB structure apply as non-atomic. TRUNCATE accepts CASCADE; whether it also accepts RESTART IDENTITY (sqlgen/objects.rs) is unconfirmed — check on a real server. With serial_normalization=rowid, SERIAL columns become `DEFAULT unique_rowid()` with no sequence. So introspect.rs should also count unique_rowid() and gen_random_uuid() defaults as auto-increment. The setval(pg_get_serial_sequence(…)) resets must be skipped when the sequence is NULL (sqlgen/ddl.rs, sqlgen/objects.rs, sqlgen/transfer.rs). Cloning with `LIKE … INCLUDING ALL` (sqlgen/objects.rs) is supported.

**Introspection.**

Keep the queries in drivers/postgres/introspect.rs; they run on CockroachDB. Every construct they use is implemented: string_agg … ORDER BY, unnest WITH ORDINALITY, LATERAL, WITH RECURSIVE, E'\x1f' strings, format_type, pg_get_indexdef(oid, k, true), pg_get_constraintdef, pg_get_triggerdef, pg_get_viewdef, current_schemas, obj_description / col_description, pg_get_userbyid and pg_encoding_to_char. pg_get_expr returns its argument unchanged, which still works because adbin holds the expression text (pg_builtins.go). Patches needed, gated on the CockroachDB flavor:

(1) Hidden columns: add `AND NOT a.attishidden` (a CockroachDB-only pg_attribute column) to table_meta (introspect.rs), completion_metadata and RELATION_NAMES_SQL (editor.rs). Keep rowid as the edit key.

(2) USER_SCHEMA_FILTER (introspect.rs) must also exclude crdb_internal and pg_extension.

(3) Row estimates: reltuples is NULL, so read them from `SELECT table_name, estimated_row_count FROM crdb_internal.table_row_statistics` or `SHOW STATISTICS FOR TABLE t`. pg_total_relation_size, pg_table_size and pg_indexes_size exist; they read system.table_metadata and may be NULL or stale.

(4) DDL text: use `SHOW CREATE TABLE t` or `SELECT create_statement FROM crdb_internal.create_statements WHERE schema_name=$1 AND descriptor_name=$2` instead of rebuilding it.

(5) Type names: format_type ignores the typmod ('Currently, the type modifier is ignored'). Take the spelled type, such as VARCHAR(10) or DECIMAL(10,2), from information_schema.columns.crdb_sql_type or from `SHOW COLUMNS FROM t` (TablePlus #2477 hit this).

(6) Diagram: pg_class.relispartition is NULL on CockroachDB, so `AND NOT c.relispartition` (sqlgen/diagram.rs) drops every row and the diagram comes back empty. Use `c.relispartition IS NOT TRUE`.

(7) Process list: pg_stat_activity is empty. Use `SELECT session_id, node_id, user_name, client_address, application_name, session_start, active_query_start, last_active_query FROM crdb_internal.cluster_sessions` together with `SELECT query_id, session_id, node_id, user_name, start, query, phase FROM crdb_internal.cluster_queries`. Kill with `CANCEL QUERY $1` or `CANCEL SESSION $1`. pg_cancel_backend and pg_terminate_backend do not appear in CockroachDB's builtins source (pg_builtins.go), so kill_sql (sqlgen/admin.rs) cannot be reused as is. Its i64 id also needs a string variant.

(8) Roles: PG_LIST_ROLES and PG_ROLE_ATTRIBUTES (admin.rs) work on pg_roles. aclexplode does not exist, so admin.rs must use `SHOW GRANTS FOR <role>` or `SELECT grantee, table_schema, table_name, privilege_type FROM information_schema.table_privileges`.

(9) Create Database: PG_COLLATIONS, PG_TEMPLATES and PG_TEMPLATE1 (admin.rs) don't fit, because collprovider is NULL and only UTF8 exists. Hide the encoding, collation and template options.

(10) TLS info: pg_stat_ssl exists but is empty (security.rs), so show it as unknown.

(11) These work unchanged: sequences (pg_sequence), routines (pg_proc.prokind, pg_get_functiondef), enums (pg_enum), and indexes (pg_index; pg_am reports 'prefix' or 'inverted'). STORING columns should surface as included columns; verify this against TablePlus #3944.

**Testing without a server.**

A real local server is easy to get. Run `cockroach start-single-node --insecure --store=type=mem,size=2GiB --listen-addr=127.0.0.1:26257` (the official Windows binary is marked experimental), or `cockroach demo`, or the Docker image cockroachdb/cockroach with `start-single-node --insecure`. Point the existing env-gated integration tests (the TABLER_TEST_PG_URL pattern in drivers/postgres/mod.rs) at postgres://root@127.0.0.1:26257/defaultdb. Add a TABLER_TEST_CRDB_URL suite and a docs/verify-servers.md section. For unit tests, cover the flavor-specific sqlgen builders (pure functions) and the session-init string. Extend the fake_pg server in connect.rs to answer version() with a CockroachDB string, to test detection. The CockroachDB Cloud Basic free tier covers TLS and SNI routing.

**What it shares with an existing driver.**

Wire-compatible with drivers/postgres. These all work: connect, TLS and SNI (connect.rs); the deadpool pool; the editor's simple and extended protocol; notices; and the SAVEPOINT used for the row cap (postgres/mod.rs). Cancel through CancelRequest also works: CockroachDB documents pgwire cancellation, with rate limiting.

Differences:

- Session init (connect.rs): client_encoding accepts only UTF8, so a non-UTF8 encoding must be refused, not merely made read-only. `SET DateStyle = 'ISO, YMD'` may be rejected on older versions, so run the init statements one at a time or gate them. Add `SET autocommit_before_ddl = off`.
- Defaults: port 26257 and database defaultdb.
- Empty catalogs: pg_stat_activity, pg_stat_ssl, pg_inherits and pg_range.
- NULL catalog values: reltuples, relispartition, relpartbound and collprovider. pg_get_partkeydef always returns NULL.
- Changed functions: pg_get_expr is an identity function and format_type ignores the typmod. aclexplode, pg_cancel_backend and pg_terminate_backend are missing, and only the transaction-level advisory locks exist.
- EXPLAIN accepts only VERBOSE, TYPES, OPT, VEC, DISTSQL, REDACT and FINGERPRINT. So `EXPLAIN (VERBOSE, COSTS, FORMAT TEXT)` and `EXPLAIN (ANALYZE, BUFFERS, VERBOSE, FORMAT TEXT)` (sqlgen/explain.rs) fail. Use `EXPLAIN (VERBOSE)` and `EXPLAIN ANALYZE`.
- Backup: pg_dump/pg_restore (backup/discover.rs) do not work (TablePlus #3435). Disable Backup, or build it from SHOW CREATE ALL TABLES plus a data export.
- The hidden rowid column appears in pg_attribute, flagged by attishidden.

**Connection fields.**

- Name / Color / Tag / Group
- Host (e.g. \<cluster>.\<region>.cockroachlabs.cloud)
- Port (default 26257)
- User (default root)
- Password + storage mode (Keychain / Ask / None / Command Line)
- Database (default defaultdb; a `postgres` database also exists)
- SSL mode (disable for --insecure clusters, verify-full for Cloud)
- SSL Key / Cert / CA Cert (client.root.key, client.root.crt, ca.crt)
- Cluster routing options (`options=--cluster=<routing-id>` for legacy Serverless), through extra parameters
- Extra parameters (application_name, options, search_path, statement_timeout)
- Read only, Timezone, Bootstrap commands, Connect timeout
- Over SSH
- Safe mode

**Effort:** M.

**Risks.**

- pg_catalog tables are virtual and generated per query. table_meta's 7 catalog queries and completion_metadata are slow on large schemas; consider SHOW / crdb_internal queries or caching.
- autocommit_before_ddl=on (the default) breaks the all-or-nothing structure apply. With it off, schema-change failures can surface only at COMMIT (XXA00).
- Serializable restarts (40001) during staged commits and imports need a retry loop; without one, users see random failures under contention.
- If hidden columns are filtered naively, rowid-keyed tables look PK-less and fall back to the ctid locator, which CockroachDB does not have.
- format_type ignores the typmod, so varchar/decimal sizes are displayed wrong and generated ALTER COLUMN TYPE is wrong unless crdb_sql_type is used.
- SET DateStyle / IntervalStyle support differs by CockroachDB version, and one failing init statement aborts the whole connect (open_editor's batch_execute, connect.rs).
- Session and query ids are strings (CANCEL QUERY '\<hex>'), not i64. This touches KillScope, ProcessRow and the renderer.
- Legacy Serverless needs `options=--cluster=…` (TablePlus #2546). Through a load balancer, pool connections can land on other nodes; that is fine for data, but node-local SHOW SESSIONS differs from the cluster-wide crdb_internal views.

### Greenplum

**Approach.**

A Postgres flavor (PgFlavor::Greenplum, following the recommendation in the CockroachDB entry) on the existing tokio-postgres driver. Greenplum and its forks are real PostgreSQL code: GP6 is based on PG 9.4.26, GP7 on PG 12, and Apache Cloudberry 2.x on PG 14. Other forks are Greengage and WarehousePG. Detect at connect with `SELECT version()`, which contains '(Greenplum Database 6.x …)' or 'Apache Cloudberry', plus `current_setting('server_version_num')`. Choose catalog SQL by server_version_num, not by flavor: the same fallbacks then also fix vanilla PostgreSQL 9.4–11, which today fails in table_meta (attidentity, attgenerated and indnkeyatts are missing). Then add the Greenplum-only metadata to TableMeta.options and to the generated DDL: distribution policy, storage / append-optimized options, legacy partitions and external tables.

**Windows build.**

Nothing new: the same tokio-postgres 0.7.18 / tokio-postgres-rustls 0.14.0 / rustls (aws-lc-rs) stack. Greenplum often runs without TLS or with self-signed certificates; use SSL mode require, or verify-ca with a CA file. Backup: the pg_dump of a PostgreSQL install cannot dump Greenplum correctly; it needs Greenplum's own pg_dump or gpbackup. backup/discover.rs (tool_names at line 30) must look for the Greenplum client tools, or Backup must be hidden.

**Authentication.**

TablePlus added the Greenplum driver in Windows build 124 and macOS build 308, with a PostgreSQL-style form. Tabler should offer: password, using MD5 (the GP6 default) or SCRAM-SHA-256 (GP7 and Cloudberry, PG 10 or later), both handled by tokio-postgres; LDAP or PAM through cleartext over TLS; TLS client certificate (pg_hba `cert`) through the SSL key/cert fields; and the Command Line password mode. Kerberos/GSSAPI, which is common in enterprise Greenplum, cannot be supported because tokio-postgres rejects AuthenticationGss. Document it as unsupported.

**Data model.**

PostgreSQL tree: databases → schemas → objects. System schemas are pg_catalog, information_schema, pg_toast, pg_aoseg, pg_bitmapindex and gp_toolkit. Objects are tables (heap, append-optimized row or column, external, foreign), views, materialized views (GP7), sequences and functions. GP6 legacy partitioning creates each partition as an ordinary table named `<parent>_1_prt_<name>`, which also appears in pg_inherits. The sidebar must hide those children and show only the parent. GP7 and Cloudberry use declarative partitions (relispartition), which Tabler already handles. Extra per-table properties for Structure / Info: DISTRIBUTED BY (cols) / RANDOMLY / REPLICATED, storage (heap, ao_row or ao_column; compresstype and level; orientation) and external location.

**Query language.**

Greenplum SQL is PostgreSQL 9.4 (GP6), 12 (GP7) or 14 (Cloudberry) plus MPP clauses: CREATE TABLE … DISTRIBUTED BY / RANDOMLY / REPLICATED; WITH (appendoptimized=true, orientation=column, compresstype=zstd); legacy PARTITION BY RANGE … (START … END … EVERY …); CREATE [READABLE|WRITABLE] EXTERNAL TABLE … LOCATION ('gpfdist://…'); the gp_toolkit views; and EXPLAIN plans with Motion nodes. The editor sends SQL unchanged through the existing simple-query path.

**Editing.**

Row edits work as on PostgreSQL. UPDATE / DELETE / INSERT with a primary key and RETURNING are supported. Updating the distribution key is allowed from GP6 on (a split update). A PK or UNIQUE constraint must include the distribution key. Critical: the no-PK row locator `ctid = (SELECT ctid … LIMIT 1)` (sqlgen/mutate.rs) is wrong on Greenplum, because ctid is unique only within one segment. Use `(gp_segment_id, ctid) IN (SELECT gp_segment_id, ctid … LIMIT 1)`, or require a primary key. The upsert `INSERT … ON CONFLICT` (sqlgen/transfer.rs) does not exist in GP6 (it is a PG 9.5 feature), so fall back to plain INSERT. GP7 has it, with append-optimized table limits. Supported DDL: ALTER TABLE ADD/DROP/RENAME COLUMN; ALTER COLUMN TYPE (not on distribution-key columns); CREATE INDEX (btree, bitmap, gin, gist; unique only when it includes the distribution key); COMMENT ON; ALTER TABLE SET DISTRIBUTED BY; and GP6 partition maintenance (ADD, DROP and SPLIT PARTITION). The CREATE TABLE built by sqlgen/ddl.rs (create_table_sql_with) must append the DISTRIBUTED and WITH(...) clauses. Otherwise recreated or cloned tables silently become DISTRIBUTED BY (first column).

**Introspection.**

The existing queries run unchanged on GP7 and Cloudberry (PG 12 / 14 catalogs). GP6 (PG 9.4) needs variants gated on server_version_num:

- introspect.rs: `a.attidentity` (PG10) → NULL::text, and `a.attgenerated <> ''` (PG12) → false.
- introspect.rs: `ix.indnkeyatts` (PG11) → ix.indnatts.
- relispartition / relpartbound / pg_get_partkeydef (PG10) at sqlgen/ddl.rs, and objects.rs → the legacy partition catalogs pg_partition, pg_partition_rule (parchildrelid) and the pg_partitions view, plus pg_get_partition_def(oid, true).
- sqlgen/diagram.rs: `array_position` (9.5) → a subquery over generate_subscripts. diagram.rs use relispartition.
- pg_sequence and pg_sequence_last_value (PG10) at objects.rs, sqlgen/ddl.rs and sqlgen/transfer.rs → read the sequence relation itself: `SELECT start_value, increment_by, min_value, max_value, cache_value, is_cycled, last_value FROM <seq>`.
- sqlgen/ddl.rs: `conparentid` (PG11) → drop the condition.
- objects.rs: prokind already falls back to proisagg / proiswindow.
- sqlgen/admin.rs: `collprovider` (PG10) → drop the filter.
- admin.rs, process list: GP6's pg_stat_activity has datid, datname, pid, sess_id, usesysid, usename, application_name, client_addr, client_hostname, client_port, backend_start, xact_start, query_start, state_change, waiting, state, query, waiting_reason, rsgid, rsgname and rsgqueueduration. It has no backend_type, wait_event or wait_event_type, so use waiting / waiting_reason and drop the backend_type filter.
- sqlgen/security.rs: pg_stat_ssl (9.5) does not exist → skip.

Greenplum additions, for all versions:

- Distribution: `SELECT pg_get_table_distributedby(c.oid)`, or gp_distribution_policy (distkey, policytype).
- Storage: on GP6, `c.relstorage` ('h', 'a', 'c', 'x', 'v', 'f') plus pg_appendonly (columnstore, compresstype, compresslevel). On GP7, pg_am.amname through c.relam ('heap', 'ao_row', 'ao_column') plus c.reloptions.
- External tables: pg_exttable (urilocation, fmttype) on GP6; foreign tables on GP7.
- In list_tables (introspect.rs), hide legacy partition children with `NOT EXISTS (SELECT 1 FROM pg_partition_rule r WHERE r.parchildrelid = c.oid)`.
- Drop the per-table pg_total_relation_size from list_tables (introspect.rs), because it is dispatched to every segment. Fetch sizes lazily for Item Overview (objects.rs), from the gp_toolkit size views (e.g. gp_size_of_table_disk).

**Testing without a server.**

There has been no official image since Broadcom closed the source in May 2024. Community Docker images exist: woblerr/greenplum (7.1.0, built from the last open-source code), woblerr/greengage and woblerr/warehousepg (6.x and 7.x forks, which give the PG 9.4 catalogs), and Apache Cloudberry images for PG 14. Unit-test the version-gated SQL selection with server_version_num 90426, 120012 and 140000. Run the existing PG integration suite (TABLER_TEST_PG_URL style) against the containers. Any old postgres:9.6 or postgres:11 image exercises the same fallbacks for vanilla PostgreSQL.

**What it shares with an existing driver.**

Fully wire-compatible: PostgreSQL protocol v3, CancelRequest, savepoints and the extended protocol all work. Every statement in session_init_sql (connect.rs) is accepted: DateStyle, IntervalStyle, extra_float_digits=3, client_encoding, default_transaction_read_only and set_config. The differences are the GP6 catalog gaps listed under introspection, plus Greenplum semantics: ctid is not unique across segments; generated DDL lacks the distribution and storage clauses; GP6 legacy partition children appear as tables; ON CONFLICT is missing on GP6; and backups need Greenplum's own pg_dump. EXPLAIN (VERBOSE, COSTS, FORMAT TEXT) and (ANALYZE, BUFFERS, VERBOSE, FORMAT TEXT) (sqlgen/explain.rs) should be checked on GP6.

**Connection fields.**

- Name / Color / Tag / Group
- Host (coordinator / master host)
- Port (default 5432)
- User (e.g. gpadmin)
- Password + storage mode
- Database (default postgres)
- SSL mode + Key / Cert / CA Cert
- Extra parameters (search_path, statement_timeout, optimizer=on/off, gp_resource_group etc. as server settings)
- Read only, Timezone, Bootstrap commands, Connect timeout
- Over SSH
- Safe mode

**Effort:** M.

**Risks.**

- Greenplum has been closed source since May 2024. Customers run Broadcom builds whose catalogs can differ from the last open-source tags used for testing.
- GP6 is PG 9.4, so table_meta's column query fails today (attidentity / attgenerated / indnkeyatts): Greenplum 6 cannot open tables at all until the fallbacks exist.
- The ctid row locator can match rows on several segments. expect_affected stops wrong writes, but edits of PK-less tables will fail.
- Recreating or cloning tables without the DISTRIBUTED BY / storage options silently changes data distribution and performance.
- pg_total_relation_size and count(\*) are dispatched to all segments and are slow on large clusters.
- Kerberos-only enterprise deployments cannot log in, because tokio-postgres has no GSSAPI.
- GP6 legacy partition syntax (EVERY, SPLIT PARTITION) is not modelled by Tabler's partition DDL code, which targets PG 10+ declarative partitions.

### Amazon Redshift

**Approach.**

A Postgres flavor (PgFlavor::Redshift) that reuses drivers/postgres for the connection, TLS, pool, editor (simple query), notices and cancel. It needs its own introspection and admin module (e.g. drivers/postgres/redshift.rs plus flavor branches in sqlgen), because the Redshift leader node is PostgreSQL 8.0.2 with Redshift-specific catalogs. tokio-postgres can talk to Redshift: rust-postgres issue #167 (2016) made the pg_enum and composite type lookups tolerate UndefinedTable on Redshift, and #330 (2018) fixed the array 'has nulls' flag Redshift rejected. Today's typeinfo lookup falls back to TYPEINFO_FALLBACK_QUERY when pg_range is missing (42P01), which covers Redshift-only type OIDs such as SUPER, GEOMETRY, VARBYTE and HLLSKETCH. Detect the flavor with version(), whose text is 'PostgreSQL 8.0.2 … Redshift 1.0.x'. An optional later path, for IAM-only or Serverless setups without network access to port 5439, is the Redshift Data API through aws-sdk-redshiftdata 1.121.0 (2026-10-01). It runs one statement at a time with no session state, so it is not the primary driver.

**Windows build.**

The wire driver needs no new crates: tokio-postgres 0.7.18, tokio-postgres-rustls 0.14.0, rustls with aws-lc-rs. Redshift certificates chain to Amazon Root CA, which is in the Windows store, so verify-full works through rustls-native-certs. If the Data API path is added, aws-sdk-redshiftdata and aws-config bring in the aws-smithy / hyper stack: several MB of binary and a longer build, but pure Rust with rustls and no OpenSSL.

**Authentication.**

TablePlus added the Redshift driver in Windows build 72 (2019) with a PostgreSQL-style form. TablePlus #3026 requests SSO, so TablePlus has no IdP / SSO support. TablePlus #3315 shows that usernames containing ':' (IAM-style) must be accepted. Tabler should offer: (1) Database user and password. MD5 works with tokio-postgres. Users created with `PASSWORD 'sha256|…'` make the server send authentication request 13, which postgres-protocol cannot parse; libpq fails the same way ('authentication method 13 not supported'). Document this, or vendor-patch tokio-postgres; the repo already vendors mysql_async. (2) IAM temporary credentials through the existing 'Command Line (IAM authentication)' mode: `aws redshift get-cluster-credentials --db-user alice --cluster-identifier c --query DbPassword --output text`, with the user then typed as `IAM:alice`, or on Serverless `aws redshift-serverless get-credentials --workgroup-name wg --query dbPassword --output text`. CommandPassword already refreshes the password for new pool connections (connect.rs). (3) SSL modes disable through verify-full; Redshift supports SSLRequest, and client certificates are not used. Not feasible natively: the browser SAML / OIDC IdP plugins, which exist only in the JDBC/ODBC drivers.

**Data model.**

Cluster or workgroup → databases → schemas → objects. Databases are the local ones from pg_database, plus datashare / consumer databases from SVV_REDSHIFT_DATABASES. On RA3 and Serverless, other databases can be read with db.schema.table names. Schemas are local, external Spectrum / Glue schemas (SVV_EXTERNAL_SCHEMAS) and datashare schemas. Objects are regular tables; external Spectrum tables (TablePlus macOS build 250 fixed loading Spectrum tables, and #3880 reports external schemas missing); views, including late-binding views created WITH NO SCHEMA BINDING, whose columns are not in pg_attribute; materialized views; stored procedures; and SQL / Python UDFs. Redshift has no indexes, triggers, sequences, partitions, enums or inheritance. Tables carry DISTSTYLE / DISTKEY / SORTKEY / ENCODE attributes; TablePlus #3900 asks for them in the DDL view.

**Query language.**

Redshift SQL is the PostgreSQL 8.0.2 dialect plus Redshift extensions: COPY and UNLOAD with S3, DISTKEY / SORTKEY / ENCODE, IDENTITY(seed, step), MERGE, SUPER with PartiQL navigation, QUALIFY, LISTAGG, SHOW TABLE / VIEW / COLUMNS / DATABASES / SCHEMAS / TABLES / PROCEDURE, and CALL. Leader-node-only functions (catalog functions, CURRENT_SCHEMA, FORMAT_TYPE …) cannot be combined with user tables or STL / STV / SVV views; the server answers 'Specified types or functions … not supported on Redshift tables'. Unsupported, per AWS's list of PostgreSQL functions and features: IS DISTINCT FROM, RETURNING, ON CONFLICT, ROLLBACK TO SAVEPOINT, arrays and unnest, string_agg / array_agg, generate_series over tables, ENCODE(), FORMAT(), clock_timestamp, pg_sleep, VALUES lists used as tables, sequences, triggers and indexes.

**Editing.**

Possible, but it needs a different SQL generator path:

- No RETURNING. sqlgen/mutate.rs adds it to every Dialect::Postgres UPDATE / INSERT; re-select after the write instead, as the MySQL path does.
- No IS NOT DISTINCT FROM (mutate.rs, select.rs). Use `(col = $1 OR (col IS NULL AND $1 IS NULL))`.
- No ctid (mutate.rs). Only tables with a primary key are editable; Redshift keys are informational and unenforced, so expect_affected=1 guards against duplicates.
- No ON CONFLICT (transfer.rs). Use MERGE or plain INSERT.
- Bytes: VARBYTE with TO_HEX instead of encode(…, 'hex') (select.rs).
- Casts: a TEXT column is converted to VARCHAR(256). Use `::varchar(65535)` instead of `::text` (select.rs; verify on a cluster), and JSON_SERIALIZE(col) for SUPER columns.

DDL:

- CREATE TABLE with DISTSTYLE / DISTKEY / SORTKEY / ENCODE. Take the existing DDL from `SHOW TABLE s.t`.
- ALTER TABLE ADD / DROP / RENAME COLUMN. ALTER COLUMN TYPE only widens VARCHAR and cannot run inside a transaction block.
- ALTER DISTSTYLE / SORTKEY / ENCODE, and COMMENT ON.
- No CREATE INDEX, TRIGGER or SEQUENCE.
- TRUNCATE takes a single table, has no RESTART IDENTITY or CASCADE (sqlgen/objects.rs), and commits the transaction.
- Clone with `LIKE … INCLUDING ALL` (objects.rs) is invalid. Use `CREATE TABLE x (LIKE y INCLUDING DEFAULTS)`; TablePlus #2289 reports clone failing on Redshift.

Transactions: BEGIN and COMMIT work, but the editor's row-cap savepoint (postgres/mod.rs) must be disabled (wants_savepoint=false). Capped reads inside a transaction are then drained instead of cancelled.

**Introspection.**

Use leader-node catalog queries with no string_agg, unnest, LATERAL, WITH ORDINALITY or FILTER, and build lists client-side. Query SVV views separately; never join pg_\* with svv_\* or stv_\*.

- Databases: `SELECT datname FROM pg_database WHERE datallowconn AND NOT datistemplate AND datname <> 'padb_harvest'`, plus `SELECT database_name, database_type FROM svv_redshift_databases`.
- Schemas: `SELECT nspname FROM pg_namespace WHERE nspname NOT IN ('pg_catalog','information_schema','pg_internal','pg_automv','catalog_history') AND nspname NOT LIKE 'pg\\_temp\\_%'`, plus `SELECT schemaname, databasename FROM svv_external_schemas`.
- Tables: `SELECT c.oid, c.relname, c.relkind, obj_description(c.oid,'pg_class') FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND c.relkind IN ('r','v')`. Sizes and rows: `SELECT "table", size, tbl_rows, estimated_visible_rows, diststyle, sortkey1 FROM svv_table_info WHERE "schema"=$1` (size is in 1 MB blocks). External tables: `SELECT tablename, location FROM svv_external_tables WHERE schemaname=$1`. Materialized views: `SELECT schema, name FROM stv_mv_info`.
- Columns: `SELECT a.attnum, a.attname, format_type(a.atttypid, a.atttypmod), a.attnotnull, pg_get_expr(d.adbin, d.adrelid), col_description(a.attrelid, a.attnum) FROM pg_attribute a JOIN pg_class c … LEFT JOIN pg_attrdef d … WHERE a.attnum > 0 AND NOT a.attisdropped`. An IDENTITY column shows as the default `"identity"(…)`.
- Dist key, sort key and encoding: `SHOW COLUMNS FROM TABLE db.s.t` (sort_key_type, sort_key, dist_key, encoding), or PG_TABLE_DEF, which only covers schemas on the search_path.
- Late-binding view columns: `SELECT * FROM pg_get_late_binding_view_cols() cols(view_schema name, view_name name, col_name name, col_type varchar, col_num int)`. External columns: svv_external_columns.
- Constraints: `SELECT conname, contype, conkey::text, confkey::text, confrelid, pg_get_constraintdef(oid) FROM pg_constraint WHERE conrelid=$1`. Map the attnums client-side; there is no conindid or conparentid.
- Indexes and triggers: none.
- DDL: `SHOW TABLE s.t` and `SHOW VIEW s.v`.
- Routines: `SELECT proname, prokind, prosrc … FROM pg_proc_info`, plus `SHOW PROCEDURE s.p(args)`.
- Processes on provisioned clusters: `SELECT process, user_name, db_name, starttime FROM stv_sessions` and `SELECT pid, status, query, starttime, duration FROM stv_recents WHERE status='Running'`. On Serverless: sys_session_history and `sys_query_history WHERE status='running'` (TablePlus #3426).
- Kill: `SELECT pg_cancel_backend(pid)` or `CANCEL pid`, and `SELECT pg_terminate_backend(pid)`.
- Users: pg_user and svv_user_info. Roles: svv_roles. Privileges: svv_relation_privileges, svv_schema_privileges and svv_database_privileges.

**Testing without a server.**

There is no local emulator; LocalStack's Redshift runs on a PostgreSQL backend and is not faithful. Unit tests:

- the Redshift SQL builders and the session-init string;
- an extended fake_pg (connect.rs) that answers version() with a Redshift string and rejects `SET IntervalStyle` and `extra_float_digits = 3`, proving the flavor init is used;
- assertions that no generated Redshift SQL contains RETURNING, IS DISTINCT FROM, ON CONFLICT, string_agg or unnest.

Real checks: a Redshift Serverless free-trial credit. Record catalog result sets there as fixtures, and add a Redshift section to docs/verify-servers.md.

**What it shares with an existing driver.**

The protocol path is shared: startup, MD5, TLS, simple and extended query, and binary results for the text-cast columns. CancelRequest support still needs checking on a real cluster. Nearly every catalog query breaks:

- introspect.rs:
  - string_agg at 392-395, 531-539, 574-579, 615-617 and 693;
  - unnest / WITH ORDINALITY / LATERAL at 366, 533 and 575;
  - WITH RECURSIVE at 399-406;
  - typcategory, typcollation, attcollation, pg_collation and pg_enum at 385-414;
  - attidentity and attgenerated at 390, indnkeyatts at 534;
  - triggers at 642-646 (tgisinternal, the 2-argument pg_get_triggerdef, the char tgenabled comparison);
  - pg_total_relation_size at 237.
- editor.rs: RELATION_NAMES_SQL binds an oid[] array (231-235, bound at 260). On Redshift the origin lookup fails (it is only logged) and editor results become read-only. Bind separate values with IN ($1, $2, …).
- objects.rs: pg_get_function_\* (133-136, 294-298) must become pg_proc_info plus SHOW PROCEDURE. pg_sequence (343-420, 876-915), the size functions (621-623) and relispartition with a text[] parameter (798-811) break too.
- sqlgen/ddl.rs: the pg_context_\* queries (373-545) error on relispartition. export/structure.rs propagates that error with `?`, so DDL view and structure export fail.
- commands/ddl.rs uses conindid.
- admin.rs (process list, roles, grants) uses backend_type, wait_event, clock_timestamp, aclexplode, VALUES and FILTER.
- security.rs uses pg_stat_ssl.
- explain.rs: use plain `EXPLAIN` / `EXPLAIN VERBOSE`; there is no ANALYZE.
- session_init_sql (connect.rs): IntervalStyle is not a Redshift parameter, extra_float_digits allows at most 2 (so `= 3` errors), and client_encoding and default_transaction_read_only cannot be set. The init must become `SET DateStyle = 'ISO, YMD'; SET extra_float_digits = 2;` plus optional TIME ZONE and search_path. Read-only must be enforced client-side or with BEGIN READ ONLY.
- pg_dump / pg_restore backup is unusable.

**Connection fields.**

- Name / Color / Tag / Group
- Host (cluster endpoint \<id>.\<hash>.\<region>.redshift.amazonaws.com, or Serverless \<workgroup>.\<account>.\<region>.redshift-serverless.amazonaws.com)
- Port (default 5439)
- User (e.g. awsuser, or IAM:\<user>)
- Password + storage mode (Keychain / Ask / None / Command Line for IAM)
- Database (default dev)
- SSL mode (+ CA Cert)
- Extra parameters (search_path, statement_timeout, query_group, enable_case_sensitive_identifier)
- Read only, Timezone, Bootstrap commands, Connect timeout
- Over SSH (bastion host)
- Safe mode

**Effort:** L.

**Risks.**

- Users with sha256-hashed passwords (authentication request 13) cannot log in through tokio-postgres without a vendored patch.
- `::text` casts can be limited to 256 characters (TEXT means VARCHAR(256)) and are invalid for SUPER / GEOMETRY / HLLSKETCH, so grid reads need Redshift-specific casts. Verify the cast semantics on a cluster.
- Leader-node vs compute-node rules: a catalog query that touches an SVV / STV view and also uses a leader-only function fails at runtime.
- Identifiers are lowercased even when quoted, unless enable_case_sensitive_identifier is on.
- DDL restrictions (ALTER COLUMN cannot run in a transaction block; TRUNCATE commits) conflict with Tabler's transactional structure apply and Discard flow.
- Serverless lacks the STV / STL views, so the process list must use SYS_\* views. Cross-database and datashare objects are read-only.
- Unverified until tested on a real cluster: CancelRequest handling, E'' escape-string syntax in leader queries, and the result-format handling for SUPER and GEOMETRY.

## Limits

_The lane lists here what v1 does not do, before it lands (definition of done, item 10)._ So far:

- All engines: no structure editing, backups or user management; no edits of a table without a key.
- CockroachDB: DDL in a transaction is not atomic; the hidden `rowid` shows as a read-only column; completion lists it too; no Create Database options.
- Greenplum: distribution, storage and partitions are shown, not edited; an external table's DDL is not shown; Greenplum 5 and older are not supported; on Greenplum 6 no sequences, diagram, Create Database options, session TLS details or UPDATE ON DUPLICATE import; the DDL leaves out the sequences a serial column draws from.
- Redshift: read-only is Tabler's classifier only; a function the user created (a SQL, Python or Lambda UDF) may have side effects from a SELECT, which the classifier cannot see; SUPER, GEOMETRY and VARBYTE values are shown as text (JSON, EWKT, hex) and are not editable; HLLSKETCH values are shown as the server sends them; no RETURNING (edited rows are read again), no EXPLAIN ANALYZE, no server-side cancel yet, no keep-alive; Spectrum and datashare tables are read-only; a SELECT in the editor after BEGIN is read to its end, not cancelled at the row cap (there are no savepoints to roll a cancel back to); an unquoted column named like a one-argument write call (`pg_terminate_backend`, `nextval`, `pg_ls_…`, `pg_stat_reset…`) makes a query a write.
