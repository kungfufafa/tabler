# Verification against real servers

`src-tauri/tests/verify_servers.rs` drives Tabler's own code (`drivers::connect`, `import::*`,
`tunnel::open_tunnel`) against real servers for three things the other integration tests do not
cover. Every test skips itself while its variables are unset, so `pnpm test:rs:int` is unaffected
unless you opt in.

| Test                                                       | What it checks                                                                                                                                                                                                                                                                         | Needs                                 |
| ---------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------- |
| `a_postgres_client_certificates`                           | TLS client certificates on PostgreSQL: `cert`, `clientcert=verify-ca` / `verify-full`; key formats, chain order, `verify-full` vs an IP host, SNI through an SSH tunnel                                                                                                                | `pg-ssl`, optional `ssh`              |
| `a_mysql_client_certificates`                              | MySQL `REQUIRE X509` / `REQUIRE SUBJECT` with `require_secure_transport=ON`; same matrix                                                                                                                                                                                               | `mysql-ssl`, optional `ssh`           |
| `b_mariadb_csv_value_changes`, `b_mysql_csv_value_changes` | CSV import turns MySQL value-change notes / warnings into row errors (`mysql_warning_guard`): rounded DECIMAL, DATE losing its time, ON DUPLICATE / REPLACE, `sql_mode ''`                                                                                                             | `mariadb` service / any MySQL         |
| `c_postgres_dump_over_ssh`, `c_mysql_dump_over_ssh`        | A SQL dump on its own connection through the profile's SSH tunnel (as `commands::import::dump_session` opens it): runs, and its sockets / server connections are gone after close while the tunnel stays up; `tunnel.close()` then frees the port and flips the status watch to `Down` | `ssh` + `postgres` / `mysql` services |

The C tests mirror `dump_session` (same `ConnectTarget`, the connection's `connect_addr()`), since a
Tauri `State` cannot be built in an integration test.

## Setup

```sh
sh dev/scripts/gen-certs.sh                                    # dev/certs/ (gitignored)
ssh-keygen -t ed25519 -N '' -f dev/ssh/tabler_dev              # once, for the ssh service
colima start                                                    # mounts $HOME, so the bind mounts work
docker compose -f dev/docker-compose.yml --profile ssl --profile mariadb \
  up -d --wait pg-ssl mysql-ssl mariadb ssh                    # ssh also starts postgres + mysql
```

`docker compose` needs the compose plugin (`brew install docker-compose`, then add
`"cliPluginsExtraDirs": ["/opt/homebrew/lib/docker/cli-plugins"]` to `~/.docker/config.json`), or
call the standalone `docker-compose` binary with the same arguments.

The compose file binds `./certs`, `./ssl`, `./ssh` and `./seed` from `dev/`. A colima VM started
without the home mount sees empty directories there (the containers then fail with
`cp: cannot stat '/certs/ca.crt'`); copy the files into the VM at the same path, or run compose
from a copy of `dev/` that exists at the same absolute path on both sides.

Services added for this (profile `ssl`):

| Service     | Address           | Users                                                                                                                                                                                                                                 |
| ----------- | ----------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `pg-ssl`    | `127.0.0.1:54339` | `tabler_cert` (`cert`), `tabler_ca` / `tabler` (password + `clientcert=verify-ca`), `tabler_full` / `tabler` (password + `clientcert=verify-full`), `tabler` / `tabler` (password); non-TLS rejected (`dev/ssl/postgres/pg_hba.conf`) |
| `mysql-ssl` | `127.0.0.1:33089` | `tabler_x509` (REQUIRE X509), `tabler_subject` (REQUIRE SUBJECT `/O=Tabler Dev/CN=tabler_cert`), `tabler_pw` / `tabler` (REQUIRE SSL); `require_secure_transport=ON`                                                                  |

Certificates (`dev/scripts/gen-certs.sh`): a root CA (`ca.crt`, the only CA the servers trust), an
intermediate that signs the client certificates, a server certificate for `localhost`, `pg-ssl`,
`mysql-ssl` (deliberately no IP SAN), and client material in every format the tests try:
`client.key` (PKCS#8), `client-pkcs1.key`, `client-ec.key` (SEC1), `client-encrypted.key`,
`client-key.der`, `client.p12`, `client-combined.pem` (key + chain), `client.crt` (leaf only),
`client-chain.crt` (leaf + intermediate), `client-chain-reversed.crt`, `other.*` (CN
`someone_else`), `full.*` (CN `tabler_full`).

## Running

```sh
set -a; . ./.env; set +a
export TABLER_TEST_PG_SSL_URL=postgres://tabler_cert@localhost:54339/tabler_dev
export TABLER_TEST_MYSQL_SSL_URL=mysql://tabler_x509@localhost:33089/tabler_dev
export TABLER_TEST_MARIADB_URL=mysql://root:tabler@127.0.0.1:33079/tabler_dev
export TABLER_TEST_SSH_PG_URL=postgres://tabler:tabler@postgres:5432/tabler_dev   # host as the ssh server sees it
export TABLER_TEST_SSH_MYSQL_URL=mysql://root:tabler@mysql:3306/tabler_dev
export TABLER_TEST_SSH_HOST=127.0.0.1 TABLER_TEST_SSH_PORT=2222 TABLER_TEST_SSH_USER=tabler TABLER_TEST_SSH_KEY=dev/ssh/tabler_dev
cargo test --manifest-path src-tauri/Cargo.toml --test verify_servers -- --nocapture --test-threads=1
docker compose -f dev/docker-compose.yml --profile ssl --profile mariadb down -v   # afterwards
```

`TABLER_TEST_SSL_CERT_DIR` (default `dev/certs`) and `TABLER_TEST_SSH_PASSWORD` (password instead
of the key) are optional. Each case prints `pass` / `FAIL` with the outcome; a test fails listing
the cases whose outcome differs from the expected (correct) behaviour. Relative paths are relative
to the repository root.

## Results (2026-10-13: PostgreSQL 17, MySQL 9.7 container + Homebrew MySQL 9.6, MariaDB 11.8.9)

### A. Client certificates

Both drivers connect with a PEM certificate file holding the leaf first followed by its
intermediates, and an unencrypted PEM key in PKCS#8 (`BEGIN PRIVATE KEY`), PKCS#1
(`BEGIN RSA PRIVATE KEY`) or SEC1 (`BEGIN EC PRIVATE KEY`); one PEM holding key + chain works in
both fields. `verify-full` through the SSH tunnel to `pg-ssl` / `mysql-ssl` works (SNI and the name
check use the profile host). `verify-full` to `127.0.0.1` is refused (the certificate has no IP SAN),
as it should be. Not accepted: encrypted keys, DER, PKCS#12 (mysql_async 0.37 with
`default-rustls` takes a PEM/DER cert-chain path and key path; PKCS#12 + password is its
native-tls API only), a leaf without its intermediate (server: `UnknownCA` alert), and a chain in the
wrong order (`KeyMismatch`). So the Key… / Cert… pickers should ask for PEM.

Problems found, and what became of them (the A tests now assert the fixed behaviour):

1. **Fixed.** PostgreSQL error text dropped the cause: tokio-postgres' `Display` is only the kind
   (`error performing TLS handshake`). `src-tauri/src/error.rs` now appends the `source()` chain
   (`error_chain`, repeated causes left out): `verify-full` to an IP host reads
   `error performing TLS handshake: invalid peer certificate: certificate not valid for name
"127.0.0.1"; …`.
2. **Fixed.** PostgreSQL with host `localhost` hid the server's FATAL: tokio-postgres tries every
   resolved address and returns the last error, so `::1`'s "connection refused" replaced
   `connection requires a valid client certificate`, `certificate authentication failed …` and the
   `pg_hba.conf … no encryption` rejection. `drivers/postgres/connect.rs` `connect_once` now tries
   each address of a name that resolves to several itself (`Config::hostaddr`, the name still used
   for TLS / SNI), reports the most telling error (the server's answer, then TLS, then the network)
   and pins the address that worked for the worker pool.
3. **Fixed (wave 3, vendored mysql_async).** MySQL `VERIFY-CA` used to check the host name too:
   mysql_async 0.37.1 (`src/io/tls/rustls_io.rs`) skipped a name mismatch only when the error text
   contained `NotValidForName`, and rustls 0.23.45 reports `NotValidForNameContext`, displayed as
   `certificate not valid for name …`. `src-tauri/vendor/mysql_async` (a `[patch.crates-io]` entry)
   matches the typed `CertificateError` instead, so `VERIFY-CA` now checks the chain, the validity
   period and the signatures but not the name; `VERIFY-IDENTITY` still checks the name against the
   profile host (also through an SSH tunnel) and, when it does not match, says to use a name the
   certificate lists or `VERIFY-CA`. Lift the patch once upstream matches the variants.
4. **Fixed.** MySQL key errors were misleading (mysql_async read a non-UTF-8 key as PKCS#1 DER, and
   an encrypted key failed with `No private key found in the file specified`, no path). Both
   drivers now read the Key… / Cert… files through `src-tauri/src/tls_files.rs`: PEM only, the key
   unencrypted PKCS#8 / PKCS#1 / SEC1, the client certificate first (checked against the key), and
   errors that name the file (`encrypted private keys are not supported`, `expected a PEM file, got
DER (binary)` / `got a PKCS#12 bundle`, `the first certificate in … does not belong to the
private key`). MySQL gets the parsed material as PEM buffers. The form's SSL hint lists the
   accepted formats.
5. **Fixed.** Without a CA file the drivers trusted different roots. MySQL `VERIFY-CA` /
   `VERIFY-IDENTITY` now trust the system's roots (the macOS Keychain, `rustls-native-certs`) as
   well as the CA file and mysql_async's bundled webpki roots, like PostgreSQL.

`src-tauri/tests/conn2_tls.rs` checks 3 and 4 against a local MySQL's auto-generated
certificates (Homebrew: `/opt/homebrew/var/mysql`) without Docker: `VERIFY-CA` to `127.0.0.1` connects
(the certificate lists no name), `VERIFY-IDENTITY` to it fails with a name error, and `VERIFY-CA` with
a CA that signed nothing still fails.

### B. Value changes as CSV import errors

Works on MariaDB 11.8.9 and on MySQL 9.6.0: `1.239` into `DECIMAL(10,2)` and `2026-01-02 10:20:30`
into `DATE` stop the import at row 2 with nothing written (MariaDB reports both as
`Data truncated for column …`, MySQL the date as `Incorrect date value …`); ON DUPLICATE KEY UPDATE
and REPLACE refuse changed values and leave the row as it was; clean rows (also through ON
DUPLICATE KEY UPDATE) pass; with `sql_mode ''` (bootstrap command) text into a DECIMAL and an
out-of-range value are refused too; the session's `sql_mode` is unchanged afterwards. Cosmetic: the
message says "MySQL would store a changed value" on MariaDB too.

### C. SQL dump over SSH

Works for PostgreSQL and MySQL through the openssh container: the dump (SET, CREATE, LOCK TABLES /
BEGIN…COMMIT) runs on its own session through the connection's tunnel; after `close()` (well under
the 5 s bound) the tunnel port is back to the connection's own sockets and the server's connection
count is back to its baseline; the tunnel stays `Up` with no status change during the import; after
`tunnel.close()` there are no sockets, nothing listens on the port, it can be bound again and the
status watch reports `Down`.

## Driver lanes

Each landed driver lane has its own real-server suite (`src-tauri/tests/<id>.rs`), built only with
the lane's feature and `#[ignore]`d, so a plain `cargo test` never reaches a server.
`node dev/scripts/lane.mjs <lane> it` loads `.env` and runs it with `--include-ignored`; each test
skips itself while its variable is unset. What only a real server can confirm is listed in the
lane's brief (`docs/drivers/<lane>.md`, Phase B status).

### LibSQL and Cloudflare D1 (`sqlite-remote`)

| Suite             | Variable                 | Server                                                                                                                                                                                         |
| ----------------- | ------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `tests/libsql.rs` | `TABLER_TEST_LIBSQL_URL` | A local `sqld` (`docker run -p 8080:8080 ghcr.io/tursodatabase/libsql-server`): `http://127.0.0.1:8080`; or Turso: `libsql://<db>-<org>.turso.io?authToken=<token>` (`turso db tokens create`) |
| `tests/d1.rs`     | `TABLER_TEST_D1_URL`     | A Cloudflare account: `d1://:<api-token>@<account-id>/<database name or UUID>`, the token with D1 Edit permission                                                                              |

```sh
node dev/scripts/lane.mjs sqlite-remote it
```

Each suite works in a table of its own (`tabler_it_<pid>`) and drops it again. It covers connect,
the catalog, browse with paging, count, fetch cell, a staged commit with a failing change, an editor
run, a cancel and a time out, and a read-only session's refusals. Turso and D1 bill the rows read.
Not run yet: no server was available when the lane landed (2026-10-31).

### SQL Server (`sqlserver`)

| Suite                | Variable                    | Server                                                                                                                                                                                                                                                                                                                                                |
| -------------------- | --------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `tests/sqlserver.rs` | `TABLER_TEST_SQLSERVER_URL` | SQL Server 2022 Developer or Express with TCP on (`docker run -e ACCEPT_EULA=Y -e MSSQL_SA_PASSWORD=<password> -p 1433:1433 mcr.microsoft.com/mssql/server:2022-latest`): `sqlserver://sa:<password>@127.0.0.1:1433/master`, a SQL Server login and a port (the suite's URL takes no instance name or options; it encrypts and takes any certificate) |

```sh
node dev/scripts/lane.mjs sqlserver it
```

The suite makes its own tables (`tabler_it_<what>_<uuid>` in the URL's database) and drops them. It
covers connect, the catalog, the session setup (ARITHABORT, TEXTSIZE) and the engine edition,
browse with paging, count, a whole binary cell, money, `bigint` past 2^53 and `datetimeoffset`,
Show DDL, a commit with a failing change and the guard on a row that is gone, `@@ROWCOUNT` after a
trigger that writes, whether a batch runs an ODBC `{call p}` escape, the login of a switched
database, cancel, the query time out, Explain's plan XML, and a read-only session's refusals (the
plan of a DELETE that leaves its row, `NEXT VALUE FOR` that leaves its sequence). TLS verify
modes, Entra tokens, NTLM and integrated logins, SQL Browser and the Azure redirect need
environments of their own. Not run yet: no server was available when the lane landed (2026-10-31).

### DynamoDB (`dynamodb`)

| Suite               | Variable                   | Server                                                                                                                                                                                                                                                                                                                                |
| ------------------- | -------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `tests/dynamodb.rs` | `TABLER_TEST_DYNAMODB_URL` | DynamoDB Local (`docker run -p 8000:8000 amazon/dynamodb-local`, or the jar with Java 17+): `dynamodb://local@127.0.0.1:8000/us-east-1`, where the user `local` signs with Local's dummy keys; or AWS itself with the host `aws`: `dynamodb://<access-key-id>:<secret>@aws/<region>`, keys allowed to create, write and delete tables |

```sh
node dev/scripts/lane.mjs dynamodb it
```

Each test creates its own on-demand table (`tabler_it_<uuid>`, waiting until it is active) and
deletes it at the end; a test that fails leaves its table behind. It covers connect and the ping,
the server info, the catalog, `definition()`, browse by cursor pages to the end, 2^53 + 1, decimals,
timestamps as text and maps as JSON, a count, a cell, a commit with a failing change, a PartiQL
read, an `UPDATE` ("1 row affected") and a `DELETE` of a missing key ("OK"), a cancel and the time
out of a count in flight, and a read-only session's refusals (the console's writes, a commit's
plan and a plan made by a writable session) leaving the table and its items as they were. On AWS
the requests are billed. Not run yet: no server was available when the lane landed (2026-10-31).

### Elasticsearch and OpenSearch (`elasticsearch`)

| Suite                    | Variable                        | Server                                                                                                                                                                                                                                                                                                                                                                                                 |
| ------------------------ | ------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `tests/elasticsearch.rs` | `TABLER_TEST_ELASTICSEARCH_URL` | Elasticsearch 7.12+ or OpenSearch 2.x with one node (`docker run -p 9200:9200 -e discovery.type=single-node -e ELASTIC_PASSWORD=changeme docker.elastic.co/elasticsearch/elasticsearch:9.1.5`, or `opensearchproject/opensearch:2.19.1` with `OPENSEARCH_INITIAL_ADMIN_PASSWORD`): `elasticsearch://elastic:changeme@127.0.0.1:9200` for plain HTTP, `?ssl=true` for HTTPS without a certificate check |

```sh
node dev/scripts/lane.mjs elasticsearch it
```

The suite creates its own index (`tabler_it_<random>`) through the `createIndex` call and deletes it
at the end. It covers connect, the version and the ping, inserts through a grid commit (one
`_bulk`), the sidebar, the mapping as columns, cursor pages sorted by a field, `long`
values past 2^53, dates as text and arrays as JSON, a count and a cell, a commit with one change
that meets a version conflict, a console search and SQL where the cluster has it, a cancel and the
query time out (a `_cluster/health` that waits for more nodes than the cluster has), Running
Queries, an object operation refused on a pattern, and a read-only session's refusals (also of a
path a decoding proxy would turn into a write). OpenSearch SQL cursors, Elasticsearch 6.x
(`_xpack/sql`), Elasticsearch Serverless, Amazon OpenSearch Service (SigV4, its `_tasks`
permissions) and TLS verify modes need environments of their own. Not run yet: no server was
available when the lane landed (2026-11-01).

### MongoDB (`mongodb`)

| Suite              | Variable                  | Server                                                                                                                                                                                                                                                                                                                                                                   |
| ------------------ | ------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `tests/mongodb.rs` | `TABLER_TEST_MONGODB_URL` | MongoDB 4.2+ (`docker run -p 27017:27017 -e MONGO_INITDB_ROOT_USERNAME=root -e MONGO_INITDB_ROOT_PASSWORD=<password> mongo:8`): `mongodb://root:<password>@127.0.0.1:27017/?authSource=admin`; for transactions a one-node replica set (`mongod --replSet rs0`, then `rs.initiate()`) with `?replicaSet=rs0`; or Atlas with `mongodb+srv://<user>:<password>@<cluster>/` |

```sh
node dev/scripts/lane.mjs mongodb it
```

The connection string's password goes to the password field and the rest, options included, to the
profile's URL. Each test works in a scratch database of its own (`tabler_it_<id>`) and drops it at
the end. It covers connect and the version, the sidebar, the sampled columns with `_id` as the key,
browse with paging, an exact count, a cell and the collection's definition, a commit with one
failing change (rolled back on a replica set, the changes before it kept on a standalone server),
a console run with a row cap, a cancel through `killOp` and the query time out on a slow `$where`,
and a read-only session's refusals (a commit's plan, console writes, write calls) leaving the
documents as they were. TLS with a CA file, a client certificate and an encrypted key, SRV, X.509,
PLAIN and OIDC sign-in, mongos and the `$currentOp` privileges of Atlas need environments of their
own. Not run yet: no server was available when the lane landed (2026-11-01).

### CockroachDB, Greenplum and Amazon Redshift (`pg-flavors`)

| Suite                 | Variable                    | Server                                                                                                                                                                                                                                                |
| --------------------- | --------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `tests/pg_flavors.rs` | `TABLER_TEST_CRDB_URL`      | CockroachDB (`docker run -p 26257:26257 cockroachdb/cockroach:latest start-single-node --insecure`): `postgres://root@127.0.0.1:26257/defaultdb`                                                                                                      |
|                       | `TABLER_TEST_GREENPLUM_URL` | Greenplum 6 or 7, or Cloudberry, with a coordinator on 5432 (a Docker image of a single-node cluster): `postgres://gpadmin:<password>@127.0.0.1:5432/postgres`                                                                                        |
|                       | `TABLER_TEST_REDSHIFT_URL`  | A Redshift Serverless workgroup (the free trial) or a provisioned cluster, reachable from this machine: `postgres://admin:<password>@<workgroup>.<account>.<region>.redshift-serverless.amazonaws.com:5439/dev?sslmode=require`; it bills the queries |

```sh
node dev/scripts/lane.mjs pg-flavors it
```

The variables are PostgreSQL URLs; the test names the engine as the connection form does, so the
session's sniffed flavor must agree with it. Each test works in a table of its own
(`public.tabler_it_<pid>`) and drops it again. Per engine it covers connect and the version, the
sidebar, browse with paging, an exact count, a cell, a commit with one failing change, the process
list, Show DDL, and creating, listing, showing and dropping a function; also CockroachDB's hidden
`rowid` key and type modifiers, Greenplum's roles through the PostgreSQL driver's SQL, Redshift's
edits in Redshift SQL on a SUPER column and a stored procedure's `SHOW PROCEDURE`, and a cancel and
a statement time out on CockroachDB and Greenplum. What only a real server can confirm (Redshift's
`pg_proc_info` and `SET TIME ZONE`, CockroachDB's `CANCEL` with a bound id, Greenplum 6's EXPLAIN
options and role SQL under ORCA) is listed in the brief. Not run yet: no server was available when
the lane landed (2026-11-01).

### ClickHouse (`clickhouse`)

| Suite                 | Variable                     | Server                                                                                                                                                                                                                                                                                   |
| --------------------- | ---------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `tests/clickhouse.rs` | `TABLER_TEST_CLICKHOUSE_URL` | ClickHouse 23.3+ (`docker run -p 8123:8123 -e CLICKHOUSE_PASSWORD=<password> clickhouse/clickhouse-server`): `clickhouse://default:<password>@127.0.0.1:8123/default`; or ClickHouse Cloud over HTTPS as `clickhouses://default:<password>@<host>:8443/default`, which bills the queries |

```sh
node dev/scripts/lane.mjs clickhouse it
```

`clickhouse://` is plain HTTP and `clickhouses://` HTTPS with the certificate checked. The user
needs to create and drop a table in the database; the test works in one of its own
(`tabler_it_<pid>`) and drops it again. It covers connect and the version, the sidebar, the
structure view, browse with paging and exact values, an exact count, a cell, a commit with one
failing change, the editor's HTTP session and row cap, a cancel and a time out by `KILL QUERY`, the
process list with Kill Query, and read-only: refused by the session, and `INSERT`, `CREATE`,
`ALTER … UPDATE` and a statement's own `SETTINGS readonly = 0` refused by the server under
`readonly=2`. What only a real server can confirm (`KILL QUERY` under `readonly=2`, the code a
refused password's 403 carries, a non-ASCII password over HTTP Basic, an X.509 user's client
certificate, the session time out's limits) is listed in the brief. Not run yet: no server was
available when the lane landed (2026-11-01).

### Cassandra and ScyllaDB (`cassandra`)

| Suite                | Variable                    | Server                                                                                                                                                                                                                                                                                                                                                                            |
| -------------------- | --------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `tests/cassandra.rs` | `TABLER_TEST_CASSANDRA_URL` | Cassandra 3.0+ (`docker run -p 9042:9042 cassandra:4.1`) or ScyllaDB (`docker run -p 9042:9042 scylladb/scylla --smp 1`), with a keyspace made first (`CREATE KEYSPACE tabler_dev WITH replication = {'class': 'SimpleStrategy', 'replication_factor': 1}` in cqlsh): `cassandra://127.0.0.1:9042/tabler_dev`, or `cassandra://<user>:<password>@…` with PasswordAuthenticator on |

```sh
node dev/scripts/lane.mjs cassandra it
```

The keyspace must exist; each test works in a table of its own (`tabler_it_<uuid>`) and drops it
again. It covers connect and the version, the keyspaces, the structure view and exact values
(a `bigint` past 2^53, decimals, collections), browse page after page with the driver's paging
state, an exact count, a cell, a commit with one change that does not apply
(a lightweight transaction's `[applied]`), the editor's row cap, a cancel and a time out, and
read-only refusals. What only a real server can confirm (prepared markers' types, schema agreement
after DDL, TLS against a node's certificate through the IP mapping, single-node pinning over SSH,
a Command password on reconnect, ScyllaDB's version and the server's `DESCRIBE`) is listed in the
brief. Not run yet: no server was available when the lane landed (2026-11-01).

### BigQuery (`bigquery`)

| Suite               | Variable                   | Server                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| ------------------- | -------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `tests/bigquery.rs` | `TABLER_TEST_BIGQUERY_URL` | A Google Cloud project with billing (the sandbox refuses DML) and a dataset made first (`bq mk tabler_dev`), with a service-account key whose account has BigQuery Job User and Data Editor: `bigquery://<project>?keyFile=<path-to-key.json>&dataset=tabler_dev`, or an access token as `bigquery://:<token>@<project>?dataset=tabler_dev` (`gcloud auth print-access-token`); `location=` for a dataset outside the US and EU multi-regions |

```sh
node dev/scripts/lane.mjs bigquery it
```

The suite works in a table of its own (`tabler_it_<pid>`) and drops it again; it reads a few MB,
which BigQuery bills. It covers connect, the datasets and the structure view with the declared key,
free pages of `tabledata.list` with their cursor and a sorted page from one job, exact values (an
INT64 past 2^53, NUMERIC, a TIMESTAMP, an array as JSON), a count and a cell, a commit with one
failing change rolled back, the editor's row cap and its BigQuery session (a temp table kept between
runs), a cancel on the server (`jobs.cancel`) and a time out, and read-only refusals.
`the_readonly_scope_refuses_dml` (with a `keyFile=` URL) is the recorded test the brief asks for
before read-only may be enforced by the server: a token with the `bigquery.readonly` scope runs a
SELECT through `jobs.query` and must be refused a DELETE.
`endpoint=http://localhost:9050/bigquery/v2` runs the suite on the emulator (`docker run -p
9050:9050 ghcr.io/goccy/bigquery-emulator --project=<project>`), a beta that lacks some of BigQuery
(sessions, scopes and job statistics may differ), so a failure there is not yet a driver bug. What
only a real server can confirm (where `jobs.insert` puts a new session's id, the message of an
expired session, the `reason` of a job stopped by `jobTimeoutMs`, `useInt64Timestamp` on
`tabledata.list`, the NULL query parameter) is listed in the brief. Not run yet: no project was
available when the lane landed (2026-11-01).

### Vertica (`vertica`)

| Suite              | Variable                  | Server                                                                                                                                                                                                                                                                                                                                  |
| ------------------ | ------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `tests/vertica.rs` | `TABLER_TEST_VERTICA_URL` | Vertica 12 or later; the Community Edition container (`docker run -p 5433:5433 opentext/vertica-ce`, the user `dbadmin` with no password and the database `VMart`) is enough: `vertica://dbadmin@127.0.0.1:5433/VMart`, or `vertica://<user>:<password>@<host>:5433/<database>` for a user that may create tables in its default schema |

```sh
node dev/scripts/lane.mjs vertica it
```

Each test works in a table of its own (`tabler_it_<uuid>`) and drops it again. The suite covers
connect and the version, the schemas, Running Queries and completion, browse with paging, a count
and a cell, exact values (an INT past 2^53), a commit with one failing change rolled back (the
count of DML's `OUTPUT` row), the editor's transaction and row cap, a cancel on the server
(`SELECT SLEEP(60)` stopped by a CancelRequest) and a time out, and a read-only profile refused
by the session and by the server (`SHOW TRANSACTION_READ_ONLY`). The URL takes no query, so TLS,
OAuth and client certificates are checked by hand: SSL mode `require` and `verify-full` against a
server with TLS on, an OAuth token as the password, and a cancel reaching a server that requires
TLS. What else only a real server can confirm (the catalog queries' columns and types,
EXPORT_OBJECTS with quoted names and for procedures, whether the READ ONLY mode itself refuses
`NEXTVAL`, the grid's NULL ordering and null-safe comparisons, complex-type cell text) is listed in
the brief. Not run yet: no server was available when the lane landed (2026-11-01).

### DuckDB (`duckdb`)

| Suite             | Variable                  | Server                                                                                                                                                              |
| ----------------- | ------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `tests/duckdb.rs` | `TABLER_TEST_DUCKDB_PATH` | none: DuckDB runs inside Tabler. A file holding the lane's sample (`tests/fixtures/duckdb/sample.sql`); `pnpm dev:duckdb:build` writes `dev/fixtures/sample.duckdb` |

```sh
pnpm dev:duckdb:build
node dev/scripts/lane.mjs duckdb it
```

The suite works on a copy of the file in a temp folder, so the file itself is never changed. It
covers connect and the version, the databases, schemas, tables and structure, browse with paging
(exact values), a count and a cell, a commit with one failing change rolled back (the error naming
the statement), the editor's transaction and row cap, a cancel and a time out interrupting the
engine, and a read-only profile refused by the session and by the engine (writes and files named
where a table goes). What only another program can confirm, checked by hand: a file the DuckDB CLI
or Python holds read-write fails with the lock error and its hint (close it there or open the
connection read-only), and a file written by a newer DuckDB is refused by the bundled 1.5.6.
Everything else (hardened and unattended sessions, encrypted files, the logger guard) is covered by
the lane's unit tests on temp files. Not run yet: the sample file was not built when the lane
landed (2026-11-01).

### Redis and Valkey (`redis`)

| Suite            | Variable                | Server                                                                                                                                                                                                                                                                                                                                |
| ---------------- | ----------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `tests/redis.rs` | `TABLER_TEST_REDIS_URL` | Redis or Valkey (`docker run -p 6379:6379 redis:8 redis-server --enable-debug-command yes`, or `valkey/valkey:8`): `redis://127.0.0.1:6379/0`, with an ACL user `redis://<user>:<password>@…`; `rediss://…` for TLS without a certificate check, `rediss://…?verify=full` to check it (`&ca=<path to the CA's PEM>` for a private CA) |

```sh
node dev/scripts/lane.mjs redis it
```

The tests write only keys under a prefix of their own (`tabler:it:<uuid>:`) in the URL's database
and delete them at the end. They cover connect and the product and version, the databases in the
sidebar, the key browser page by page by SCAN cursor, counts, whole cells, a commit with one failing
change and the inserts the grid sends, the console with `SELECT` and `MULTI`, verified TLS (and,
with a CA, the refusal without it), and a read-only profile's refusals on every write path (console
writes, the module writes Redis Stack flags `readonly`, a commit's plan and apply, the write calls,
a kill, a writing bootstrap command), checking nothing was written. The cancel and the query time
out need `DEBUG SLEEP`, so they run only where the server enables `DEBUG` (Redis 7+:
`enable-debug-command`) and are skipped otherwise. ACL users without `@transaction` or `CONFIG`,
Cluster refusals, managed services (ElastiCache, Upstash, Azure's TLS on 6380), Redis Stack's
modules and other servers (KeyDB, Dragonfly, Garnet, Memurai) need environments of their own (the
brief's "What only a real server can confirm"). Not run yet: no server was available when the lane
landed (2026-11-02).
