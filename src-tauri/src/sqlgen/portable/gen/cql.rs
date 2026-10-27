//! Generated cases of CQL (`dialect::cql`, fixture
//! `tests/fixtures/classify-generated/cql.json`).
//!
//! CQL reads are `SELECT`, server-side `DESCRIBE` / `DESC` and `LIST`, `USE`, and the cqlsh
//! commands the session handles itself (`CONSISTENCY`, `SERIAL CONSISTENCY`, `TRACING`, `PAGING`,
//! `EXPAND`). Every other statement writes: DML with lightweight transactions and `USING TTL` /
//! `TIMESTAMP`, batches, DDL, roles and permissions, and cqlsh's file commands. Strings take no
//! backslash escapes, `$$…$$` bodies have no tags, and `//` starts a comment.

use super::Suite;
use crate::sqlgen::dialect::cql::SPEC;

/// Ordinary reads of a Cassandra or ScyllaDB user: key and range lookups, paging by token,
/// `ALLOW FILTERING`, collection filters, aggregates, `SELECT JSON`, the CQL functions,
/// `system_schema` introspection, and the rules' words where they are no keywords.
const READS: &[&str] = &[
    "SELECT * FROM users",
    "SELECT * FROM shop.orders WHERE customer_id = 42",
    "SELECT id, name, email FROM users WHERE id = 6ab09bec-e68e-48d9-a5f8-97e6fb4c9b47",
    "SELECT * FROM events WHERE user_id = ? AND bucket = ? AND ts > '2024-01-01' AND ts <= \
     '2024-02-01 00:00:00+0000' ORDER BY ts DESC LIMIT 100",
    "SELECT * FROM events WHERE user_id IN (1, 2, 3) AND bucket = 0 PER PARTITION LIMIT 10",
    "SELECT * FROM sensor_data WHERE sensor_id = 7 AND (day, seq) > ('2024-01-01', 10) AND \
     (day, seq) <= ('2024-01-31', 0)",
    "SELECT JSON * FROM users WHERE id = :id",
    "SELECT JSON id, name AS full_name FROM users LIMIT 10",
    "SELECT DISTINCT user_id, bucket FROM events",
    "SELECT count(*) FROM events WHERE user_id = 1 AND bucket = 2",
    "SELECT user_id, bucket, max(ts), min(ts), avg(amount), sum(amount), count(1) FROM events \
     WHERE user_id = 1 GROUP BY user_id, bucket",
    "SELECT k, ttl(v), writetime(v), token(k) FROM kv WHERE token(k) > -9223372036854775808 AND \
     token(k) <= 0 LIMIT 5000",
    "SELECT * FROM users WHERE email = 'a@example.com' ALLOW FILTERING",
    "SELECT * FROM users WHERE country = 'ID' LIMIT 300 ALLOW FILTERING",
    "SELECT * FROM products WHERE tags CONTAINS 'sale' AND attrs CONTAINS KEY 'color' AND \
     attrs['size'] = 'L' ALLOW FILTERING",
    "SELECT * FROM products WHERE name LIKE 'cof%'",
    "SELECT * FROM t WHERE v IN ('a', 'b') AND w != 3 ALLOW FILTERING",
    "SELECT CAST(amount AS text), toJson(addr), toDate(ts), toUnixTimestamp(ts), dateOf(id), \
     unixTimestampOf(id) FROM payments",
    "SELECT blobAsText(data), textAsBlob('x'), bigintAsBlob(1), uuid(), now() FROM blobs",
    "SELECT * FROM events WHERE id > maxTimeuuid('2013-01-01 00:05+0000') AND id < \
     minTimeuuid('2013-02-02 10:00+0000') ALLOW FILTERING",
    "SELECT addr.city, addr.zip, m['key'], l[0], s FROM users WHERE id = 1",
    "SELECT * FROM t WHERE k = 0x0fab AND d = 1h30m AND f = NaN AND g = -Infinity AND b = true",
    "SELECT * FROM readings WHERE id = 1 AND ts >= currentTimestamp() - 1d",
    "SELECT similarity_cosine(embedding, [0.1, 0.2, 0.3]) FROM docs ORDER BY embedding ANN OF \
     [0.1, 0.2, 0.3] LIMIT 5",
    "SELECT * FROM t WHERE m = {'a': 1, 'b': 2} AND s = {1, 2} AND tup = (1, 'x') ALLOW \
     FILTERING",
    "SELECT keyspace_name, durable_writes, replication FROM system_schema.keyspaces",
    "SELECT table_name, comment, default_time_to_live FROM system_schema.tables WHERE \
     keyspace_name = 'shop'",
    "SELECT column_name, kind, position, clustering_order, type FROM system_schema.columns \
     WHERE keyspace_name = ? AND table_name = ?",
    "SELECT release_version, cql_version, cluster_name, data_center FROM system.local WHERE key \
     = 'local'",
    "SELECT peer, data_center, rack, release_version, rpc_address FROM system.peers",
    "SELECT range_start, range_end, partitions_count FROM system.size_estimates WHERE \
     keyspace_name = 'shop' AND table_name = 'orders'",
    "SELECT * FROM system_views.settings WHERE name = 'cluster_name'",
    "SELECT \"Mixed\", \"select\", \"delete\", \"insert\".\"update\" FROM \"Ks\".\"Table\"",
    "SELECT 'DELETE FROM t; DROP TABLE x' AS s, $$INSERT INTO t$$ AS d FROM t",
    "SELECT 'C:\\temp\\' AS dir, 'it''s' AS q FROM t",
    "-- DELETE FROM t\nSELECT k /* DROP TABLE t; */ FROM t // TRUNCATE t",
    "// UPDATE t SET v = 1\nSELECT * FROM t",
    "SELECT created_by, insert_count, update_time, delete_flag, applied FROM audit",
    "SELECT\n  id,\n  total\nFROM\n  shop.orders\nWHERE\n  id = 1",
    "SELECT * FROM t; SELECT * FROM u;",
    "DESCRIBE KEYSPACES",
    "DESCRIBE KEYSPACE shop",
    "DESCRIBE ONLY KEYSPACE shop",
    "DESCRIBE TABLE shop.orders",
    "DESCRIBE shop.orders WITH INTERNALS",
    "DESCRIBE FULL SCHEMA",
    "DESCRIBE CLUSTER",
    "DESCRIBE TYPE shop.address",
    "DESCRIBE MATERIALIZED VIEW shop.orders_by_day",
    "DESCRIBE INDEX shop.orders_by_email",
    "DESC TABLES",
    "DESC FUNCTIONS",
    "LIST ROLES",
    "LIST USERS",
    "LIST ALL PERMISSIONS OF alice",
    "LIST ALL PERMISSIONS ON KEYSPACE shop NORECURSIVE",
    "USE shop",
    "USE \"MixedKs\"",
    "CONSISTENCY",
    "CONSISTENCY LOCAL_QUORUM",
    "SERIAL CONSISTENCY LOCAL_SERIAL",
    "TRACING ON",
    "TRACING OFF",
    "PAGING 100",
    "PAGING OFF",
    "EXPAND ON",
];

/// Writes beyond the rules' words: batches of every kind, lightweight transactions, `USING TTL`
/// and `TIMESTAMP`, collection updates, `INSERT … JSON`, DDL of every object, roles and
/// permissions, Cassandra 5's identities, ScyllaDB's view pruning, cqlsh's file commands, and
/// text a comment rule or a backslash may turn into a second statement.
const WRITES: &[&str] = &[
    "BEGIN BATCH INSERT INTO t (k, v) VALUES (1, 'a'); UPDATE t SET v = 'b' WHERE k = 2; DELETE \
     FROM t WHERE k = 3; APPLY BATCH",
    "BEGIN UNLOGGED BATCH USING TIMESTAMP 1700000000000 INSERT INTO t (k) VALUES (1) APPLY BATCH",
    "BEGIN COUNTER BATCH UPDATE page_views SET views = views + 1 WHERE page = 'home'; APPLY BATCH",
    "APPLY BATCH",
    "INSERT INTO users (id, name) VALUES (uuid(), 'a') IF NOT EXISTS",
    "INSERT INTO users JSON '{\"id\": 1, \"name\": \"a\"}' DEFAULT UNSET",
    "INSERT INTO t (k, v) VALUES (1, 'x') USING TTL 86400 AND TIMESTAMP 123",
    "UPDATE users USING TTL 3600 SET name = 'b' WHERE id = 1 IF name = 'a'",
    "UPDATE t SET tags = tags + {'x'}, m['k'] = 'v', l = l - ['y'] WHERE k = 1 IF EXISTS",
    "UPDATE page_views SET views = views - 1 WHERE page = 'home'",
    "UPDATE system_views.settings SET value = 'x' WHERE name = 'y'",
    "DELETE name FROM users USING TIMESTAMP 1 WHERE id = 1 IF EXISTS",
    "DELETE m['k'], l[0] FROM t WHERE k = 1",
    "DELETE FROM events WHERE user_id = 1 AND ts < '2024-01-01'",
    "TRUNCATE TABLE shop.orders",
    "TRUNCATE orders",
    "CREATE KEYSPACE IF NOT EXISTS shop WITH replication = {'class': 'NetworkTopologyStrategy', \
     'dc1': 3} AND durable_writes = true",
    "ALTER KEYSPACE shop WITH durable_writes = false",
    "DROP KEYSPACE shop",
    "CREATE TABLE IF NOT EXISTS t (k int, c timestamp, v text, PRIMARY KEY ((k), c)) WITH \
     CLUSTERING ORDER BY (c DESC) AND comment = 'x'",
    "ALTER TABLE t ADD v2 text",
    "ALTER TABLE t RENAME k TO k2",
    "ALTER TABLE t WITH gc_grace_seconds = 0",
    "DROP TABLE IF EXISTS t",
    "CREATE INDEX ON t (v)",
    "CREATE CUSTOM INDEX t_v ON t (v) USING 'StorageAttachedIndex'",
    "DROP INDEX shop.t_v",
    "CREATE MATERIALIZED VIEW v AS SELECT * FROM t WHERE k IS NOT NULL AND c IS NOT NULL \
     PRIMARY KEY (c, k)",
    "DROP MATERIALIZED VIEW v",
    "CREATE TYPE address (street text, city text)",
    "ALTER TYPE address ADD zip text",
    "CREATE OR REPLACE FUNCTION f (x int) CALLED ON NULL INPUT RETURNS int LANGUAGE java AS $$ \
     return x; $$",
    "CREATE AGGREGATE a (int) SFUNC f STYPE int INITCOND 0",
    "DROP FUNCTION f",
    "CREATE TRIGGER tr ON t USING 'org.example.Trigger'",
    "CREATE ROLE alice WITH PASSWORD = 'secret' AND LOGIN = true",
    "ALTER ROLE alice WITH PASSWORD = 'x'",
    "CREATE USER bob WITH PASSWORD 'pw' SUPERUSER",
    "DROP ROLE alice",
    "GRANT SELECT ON KEYSPACE shop TO alice",
    "REVOKE MODIFY ON ALL KEYSPACES FROM alice",
    "GRANT alice TO bob",
    "ADD IDENTITY 'spiffe://example/x' TO ROLE 'alice'",
    "DROP IDENTITY 'spiffe://example/x'",
    "PRUNE MATERIALIZED VIEW shop.v WHERE v = 1",
    "LOGIN alice 'secret'",
    "SOURCE '/tmp/x.cql'",
    "COPY shop.orders TO '/tmp/orders.csv'",
    "COPY t FROM 'x.csv' WITH HEADER = true",
    "CAPTURE '/tmp/out.txt'",
    "SELECT * FROM t; DELETE FROM t WHERE k = 1",
    "DESCRIBE TABLE t; DROP TABLE t",
    "USE shop; TRUNCATE t",
    "CONSISTENCY ALL; INSERT INTO t (k) VALUES (1)",
    "LIST ROLES; GRANT alice TO bob",
    "SELECT * FROM t WHERE k = 1 /* /* */ DELETE FROM t */",
    "SELECT * FROM t -- c\rDELETE FROM t",
    "SELECT * FROM t // c\rDELETE FROM t",
    // A backslash escapes nothing: the string ends at the quote after it.
    "SELECT 'a\\'; DELETE FROM t WHERE k = 1; --'",
    "SELECT $$ $$; DELETE FROM t WHERE k = 1",
    "SELECT 1DELETE FROM t",
];

fn suite() -> Suite {
    Suite::new("cql", &SPEC)
        .reads(READS)
        .writes(WRITES)
        .active(
            "after ALLOW FILTERING",
            "SELECT * FROM t WHERE a = 1 ALLOW FILTERING ",
            " t",
        )
        .active("after SELECT JSON", "SELECT JSON ", " t")
        .active("after SELECT DISTINCT", "SELECT DISTINCT ", " t")
        .active(
            "after PER PARTITION LIMIT",
            "SELECT * FROM t PER PARTITION LIMIT 1 ",
            " t",
        )
        .active(
            "in a map literal",
            "SELECT * FROM t WHERE m = {'a': ",
            " t}",
        )
        .active("in a set literal", "SELECT * FROM t WHERE s = {1, ", "}")
        .active("in a list literal", "SELECT * FROM t WHERE l = [1, ", "]")
        .active(
            "in a tuple relation",
            "SELECT * FROM t WHERE (a, b) > (1, ",
            " t)",
        )
        .active("in an element access", "SELECT m[", "] FROM t")
        .active(
            "after a uuid literal",
            "SELECT * FROM t WHERE id = 6ab09bec-e68e-48d9-a5f8-97e6fb4c9b47 ",
            " t",
        )
        .active("glued to a blob literal", "SELECT 0xCAFE", " t")
        .active(
            "glued to a duration",
            "SELECT * FROM t WHERE d = 1h30m",
            " t",
        )
        .active("glued to a ? marker", "SELECT * FROM t WHERE k = ?", " t")
        .active(
            "after a named bind marker",
            "SELECT * FROM t WHERE k = :k ",
            " t",
        )
        .active("in a token() call", "SELECT * FROM t WHERE token(", ") > 0")
        .active("after DESCRIBE and ;", "DESCRIBE TABLE t; ", " t")
        .active("after LIST ROLES and ;", "LIST ROLES; ", " t")
        .active("after USE and ;", "USE shop; ", " t")
        .active("after a cqlsh command and ;", "CONSISTENCY ONE; ", " t")
        .active("after a $$ body holding //", "SELECT $$ // $$ ", " t")
        .active("after a $$ body holding a quote", "SELECT $$ ' $$ ", " t")
        .active("after a string holding --", "SELECT '--' ", " t")
        .active("after a string holding $$", "SELECT '$$' ", " t $$")
        .active("after a quoted name holding $$", "SELECT \"$$\" ", " t $$")
        .active("after a // comment and CRLF", "SELECT 1 // c\r\n", " t")
        .active(
            "after a block comment holding //",
            "SELECT 1 /* // */ ",
            " t",
        )
        .inert(
            "in a $$ body holding a quote",
            "SELECT $$ ' ",
            " t $$ FROM x",
        )
        .inert(
            "in a $$ body holding a line comment",
            "SELECT $$ -- c\n",
            " t $$ FROM x",
        )
        .inert(
            "in a UDF-like $$ body",
            "SELECT $$ return x; ",
            "; $$ FROM x",
        )
        .inert("in a string holding $$", "SELECT '$$ ", " t' FROM x")
        .inert("in a string holding //", "SELECT '// ", " t' FROM x")
        .inert("in a string holding /*", "SELECT '/* ", " t' FROM x")
        .inert(
            "in a quoted name holding a quote",
            "SELECT \"a'b ",
            " t\" FROM x",
        )
        .inert(
            "in a // comment after a //",
            "SELECT 1 // a // ",
            " t\nFROM x",
        )
        .inert(
            "in a block comment holding //",
            "SELECT 1 /* // ",
            " t */ FROM x",
        )
        .vocab(&[
            "ALLOW",
            "FILTERING",
            "JSON",
            "PER",
            "PARTITION",
            "token(",
            "ttl(",
            "writetime(",
            "CONTAINS",
            "KEY",
            "IF",
            "EXISTS",
            "USING",
            "TTL",
            "TIMESTAMP",
            "UNLOGGED",
            "COUNTER",
            "LOGGED",
            "0xCAFE",
            "1h30m",
            "6ab09bec-e68e-48d9-a5f8-97e6fb4c9b47",
            ":k",
            "{'a': 1}",
            "[1, 2]",
            "NaN",
            "-Infinity",
            "$$a'b$$",
            "$$ // $$",
            "'$$'",
            "\"$$\"",
            "frozen<list<text>>",
            "-- c\r\n",
        ])
}

#[test]
fn generated_cases() {
    suite().check();
}

#[test]
fn fuzz() {
    suite().fuzz(3000);
}
