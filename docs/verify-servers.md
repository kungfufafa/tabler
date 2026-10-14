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
