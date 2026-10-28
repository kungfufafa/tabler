//! Generated cases of DuckDB SQL (`dialect::duckdb`, fixture
//! `tests/fixtures/classify-generated/duckdb.json`).
//!
//! DuckDB reads are `SELECT`, `WITH`, `VALUES`, `TABLE`, FROM-first queries, `PIVOT` / `UNPIVOT`
//! (and `PIVOT_WIDER` / `PIVOT_LONGER`), `SHOW`, `DESCRIBE`, `SUMMARIZE`, `EXPLAIN` (which runs
//! its statement only with ANALYZE) and `USE`. Everything else writes, and so does a read that
//! calls a file scanner, a scanner of another server, dynamic SQL or a function that changes
//! state (`write_calls`), or reads a table function outside the allow-list. Strings are
//! standard (backslashes plain) except `E'…'`, `$tag$…$tag$` bodies are strings, block comments
//! nest. Files named where a table goes are the engine's to refuse (`drivers::duckdb::engine`).

use super::Suite;
use crate::sqlgen::dialect::duckdb::SPEC;

/// Ordinary reads of a DuckDB user: friendly SQL (FROM-first, `EXCLUDE` / `REPLACE` /
/// `COLUMNS`, `GROUP BY ALL`, `QUALIFY`, `UNION BY NAME`), PIVOT and UNPIVOT in every form,
/// ASOF / POSITIONAL / ANTI / SEMI joins, samples, nested values and lambdas, JSON paths, the
/// catalog functions, the allowed table functions, `SHOW` / `DESCRIBE` / `SUMMARIZE` / `EXPLAIN`
/// / `USE`, and the rules' words where they are no keywords.
const READS: &[&str] = &[
    "SELECT 1",
    "SELECT * FROM orders WHERE id = 42",
    "SELECT * EXCLUDE (password_hash) FROM users",
    "SELECT * REPLACE (lower(email) AS email) FROM users",
    "SELECT COLUMNS('price_.*') FROM products",
    "SELECT min(COLUMNS(* EXCLUDE (id))) FROM measurements",
    "SELECT customer_id, sum(total) AS spent FROM orders GROUP BY ALL ORDER BY ALL",
    "SELECT region, product, row_number() OVER (PARTITION BY region ORDER BY revenue DESC) AS \
     rk FROM sales QUALIFY rk <= 3",
    "FROM orders",
    "FROM orders SELECT customer_id, total WHERE total > 100 ORDER BY total DESC LIMIT 10",
    "FROM orders o JOIN customers c USING (customer_id) SELECT c.name, o.total",
    "WITH recent AS (FROM orders WHERE created_at > now() - INTERVAL 30 DAY) SELECT count(*) \
     FROM recent",
    "WITH RECURSIVE r(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM r WHERE n < 10) SELECT * \
     FROM r",
    "WITH m AS MATERIALIZED (SELECT * FROM t) SELECT * FROM m",
    "WITH v AS (VALUES (1), (2)) FROM v SELECT *",
    "VALUES (1, 'a'), (2, 'b')",
    "TABLE orders",
    "PIVOT sales ON year USING sum(amount) GROUP BY region",
    "PIVOT_WIDER sales ON year IN (2023, 2024) USING sum(amount)",
    "UNPIVOT monthly ON jan, feb, mar INTO NAME month VALUE amount",
    "UNPIVOT monthly ON COLUMNS(* EXCLUDE (id))",
    "PIVOT_LONGER monthly ON COLUMNS(* EXCLUDE (id)) INTO NAME month VALUE amount",
    "FROM monthly UNPIVOT (amount FOR month IN (jan, feb, mar))",
    "SELECT * FROM (PIVOT sales ON year USING sum(amount)) p",
    "SELECT * FROM trades t ASOF JOIN quotes q ON t.symbol = q.symbol AND t.ts >= q.ts",
    "SELECT * FROM a POSITIONAL JOIN b",
    "SELECT * FROM t ANTI JOIN u USING (id)",
    "SELECT * FROM t SEMI JOIN u ON t.id = u.id",
    "SELECT * FROM t USING SAMPLE 10%",
    "SELECT * FROM t TABLESAMPLE reservoir(100 ROWS) REPEATABLE (42)",
    "SELECT [1, 2, 3] AS l, {'a': 1, 'b': 'x'} AS s, MAP {'k': 1} AS m, l[1], s.a, s['b']",
    "SELECT list_transform([1, 2, 3], x -> x * 2), list_filter(l, lambda x: x > 1) FROM t",
    "SELECT [x * 2 FOR x IN l IF x > 1] FROM t",
    "SELECT unnest([1, 2, 3]), unnest({'a': 1, 'b': 2})",
    "SELECT * FROM unnest([1, 2, 3]) AS u(x)",
    "SELECT * FROM range(10) r(i), generate_series(1, 3) g(j)",
    "SELECT * FROM generate_series(TIMESTAMP '2024-01-01', TIMESTAMP '2024-01-02', INTERVAL 1 \
     HOUR)",
    "SELECT j->>'$.name' AS name, j->'tags' AS tags, json_extract(j, '$.a') FROM events",
    "SELECT * FROM json_each('{\"a\": 1}')",
    "SELECT x::JSON, to_json(s), json_structure(j), json_keys(j) FROM t",
    "SELECT id::VARCHAR, CAST(total AS DECIMAL(18, 2)), TRY_CAST(x AS INTEGER), amount::DOUBLE \
     FROM orders",
    "SELECT * FROM t WHERE name ILIKE 'a%' AND code NOT LIKE '%x' ESCAPE '!' AND \
     regexp_matches(email, '^[a-z]+@')",
    "SELECT * FROM t WHERE a IS DISTINCT FROM b AND c IS NOT DISTINCT FROM d",
    "SELECT * FROM t WHERE x GLOB '*.csv' AND y SIMILAR TO 'a%'",
    "SELECT date_trunc('month', ts), strftime(ts, '%Y-%m'), epoch_ms(ts), age(ts), ts + \
     INTERVAL 1 HOUR FROM events",
    "SELECT current_database(), current_schema(), version(), currval('order_seq'), \
     current_setting('threads'), getvariable('row_limit')",
    "SELECT approx_count_distinct(user_id), quantile_cont(latency, 0.9), arg_max(name, score), \
     list(name ORDER BY score), histogram(status) FROM requests",
    "SELECT count(*) FILTER (WHERE total > 0) FROM orders",
    "SELECT a, b, count(*) FROM t GROUP BY GROUPING SETS ((a), (b), ())",
    "SELECT a, b, sum(c) FROM t GROUP BY CUBE (a, b)",
    "SELECT * FROM t1 WHERE id IN (SELECT id FROM t2) AND EXISTS (SELECT 1 FROM t3 WHERE \
     t3.id = t1.id)",
    "SELECT * FROM t LEFT JOIN LATERAL (SELECT * FROM u WHERE u.id = t.id) l ON true",
    "SELECT a.* FROM t AS a NATURAL JOIN u",
    "SELECT * FROM (FROM orders SELECT id) s",
    "SELECT id FROM a UNION ALL SELECT id FROM b EXCEPT SELECT id FROM c INTERSECT SELECT id \
     FROM d",
    "SELECT * FROM a UNION BY NAME SELECT * FROM b",
    "(SELECT 1) UNION (SELECT 2) ORDER BY 1",
    "SELECT * FROM t ORDER BY a NULLS LAST LIMIT 50 OFFSET 100",
    "SELECT CASE WHEN total > 100 THEN 'big' ELSE 'small' END, coalesce(note, ''), \
     nullif(d, 0), if(a, 1, 2) FROM orders",
    "SELECT struct_insert(s, c := 1), list_prepend(0, l), map_extract(m, 'k') FROM t",
    "SELECT format('{} {}', a, b), printf('%d', n), md5(x), sha256(x), from_hex('ff'), \
     encode('a') FROM t",
    "SELECT enum_range(NULL::mood), typeof(x), 'a' COLLATE nocase = 'A' FROM t",
    "SELECT uuid(), gen_random_uuid(), now(), current_timestamp, random()",
    "SELECT * FROM orders AT (VERSION => 3)",
    "SELECT * FROM main.orders o, other.main.customers c WHERE o.cid = c.id",
    "SELECT * FROM t WHERE x = ? AND y = $name AND z = $1",
    "SELECT \"delete\", \"insert\".\"update\", \"read_csv\" FROM \"insert\"",
    "SELECT 'DELETE FROM t; DROP TABLE x' AS s, $$INSERT INTO t$$ AS d, E'it\\'s' AS e",
    "SELECT 'SELECT * FROM read_csv(''x.csv'')' AS sql_text, $q$COPY t TO 'x'$q$",
    "-- DELETE FROM t\nSELECT 1 /* DROP TABLE t; ATTACH 'x'; */",
    "SELECT created_by, insert_count, copy_of, load_time, set_name, call_id, read_at FROM audit",
    "SELECT 1.5, .5, 1e3, 1.5e-3, 0x1F, 0b101, 1_000_000 FROM t",
    "SELECT\n  o.id,\n  o.total\nFROM\n  orders AS o\nWHERE\n  o.total > 0",
    "SELECT 1; SELECT 2;",
    "SELECT * FROM duckdb_tables() WHERE schema_name = 'main'",
    "SELECT column_name, data_type FROM duckdb_columns() WHERE table_name = $1 ORDER BY \
     column_index",
    "SELECT * FROM information_schema.tables WHERE table_schema = 'main'",
    "SELECT * FROM pragma_table_info('orders')",
    "SELECT * FROM pragma_storage_info('orders')",
    "SELECT * FROM test_all_types()",
    "SELECT * FROM repeat('x', 3)",
    "SELECT json_serialize_sql('SELECT * FROM read_csv(''x.csv'')'), json_deserialize_sql(j) FROM t",
    "SHOW TABLES",
    "SHOW ALL TABLES",
    "SHOW DATABASES",
    "SHOW orders",
    "DESCRIBE orders",
    "DESCRIBE SELECT * FROM orders",
    "DESC orders",
    "SUMMARIZE orders",
    "SUMMARIZE SELECT total FROM orders WHERE total > 0",
    "EXPLAIN SELECT * FROM orders WHERE id = 1",
    "EXPLAIN ANALYZE SELECT count(*) FROM orders",
    "EXPLAIN (FORMAT json) SELECT 1",
    "EXPLAIN DELETE FROM orders WHERE id = 1",
    "USE other",
    "USE other.main",
];

/// Writes beyond the rules' words: DML of every form, DDL of DuckDB's objects and secrets,
/// COPY / EXPORT / IMPORT, ATTACH and extensions, settings, variables and pragmas, transactions
/// and prepared statements, maintenance, EXPLAIN ANALYZE of a write, the scanners and state
/// changes of the sweep in every position and spelling (also inside inspections, which run
/// their query), table functions outside the allow-list, and text a comment rule or a number
/// may turn into a second statement.
const WRITES: &[&str] = &[
    "INSERT INTO t VALUES (1, 'a')",
    "INSERT OR REPLACE INTO t VALUES (1, 'a')",
    "INSERT INTO t BY NAME SELECT * FROM s",
    "INSERT INTO t VALUES (1) ON CONFLICT (id) DO UPDATE SET a = excluded.a",
    "UPDATE t SET a = 1 WHERE id = 2",
    "DELETE FROM t WHERE id = 1",
    "MERGE INTO t USING s ON t.id = s.id WHEN MATCHED THEN UPDATE SET a = s.a WHEN NOT \
     MATCHED THEN INSERT VALUES (s.id, s.a)",
    "WITH s AS (SELECT 1 AS id) INSERT INTO t SELECT * FROM s",
    "TRUNCATE t",
    "CREATE TABLE t (id INTEGER PRIMARY KEY, name VARCHAR)",
    "CREATE OR REPLACE TABLE t AS FROM read_parquet('s3://b/*.parquet')",
    "CREATE TEMP TABLE scratch AS SELECT 1",
    "CREATE VIEW v AS SELECT * FROM read_csv_auto('x.csv')",
    "CREATE MACRO m(a) AS a + 1",
    "CREATE MACRO r(n) AS TABLE SELECT * FROM range(n)",
    "CREATE SEQUENCE s START 1",
    "CREATE TYPE mood AS ENUM ('sad', 'ok')",
    "CREATE SCHEMA s",
    "CREATE INDEX i ON t (a)",
    "CREATE SECRET (TYPE s3, KEY_ID 'k', SECRET 's')",
    "CREATE PERSISTENT SECRET s (TYPE http, BEARER_TOKEN 't')",
    "DROP SECRET s",
    "ALTER TABLE t ADD COLUMN c INTEGER DEFAULT 0",
    "ALTER TABLE t RENAME TO u",
    "DROP TABLE t",
    "COMMENT ON TABLE t IS 'orders'",
    "COPY t TO 'out.parquet' (FORMAT parquet)",
    "COPY t FROM 'in.csv' (HEADER)",
    "COPY (SELECT * FROM t) TO 'out.csv'",
    "COPY FROM DATABASE a TO b",
    "EXPORT DATABASE 'dir' (FORMAT parquet)",
    "IMPORT DATABASE 'dir'",
    "ATTACH 'other.duckdb' AS other (READ_ONLY)",
    "ATTACH 'host=h dbname=d' AS pg (TYPE postgres)",
    "ATTACH IF NOT EXISTS 'md:'",
    "DETACH other",
    "INSTALL httpfs",
    "FORCE INSTALL spatial FROM core_nightly",
    "INSTALL h3 FROM community",
    "LOAD spatial",
    "LOAD 'C:/x/ext.duckdb_extension'",
    "UPDATE EXTENSIONS",
    "SET threads = 4",
    "SET GLOBAL enable_external_access = true",
    "SET lock_configuration = false",
    "SET VARIABLE row_limit = 10",
    "SET search_path = 'other'",
    "RESET threads",
    "RESET VARIABLE row_limit",
    "PRAGMA enable_profiling",
    "PRAGMA table_info('t')",
    "PRAGMA create_fts_index('docs', 'id', 'body')",
    "CALL dbgen(sf = 0.1)",
    "CALL pragma_table_info('t')",
    "CALL start_ui()",
    "CHECKPOINT",
    "FORCE CHECKPOINT",
    "VACUUM ANALYZE t",
    "ANALYZE",
    "BEGIN TRANSACTION",
    "COMMIT",
    "ROLLBACK",
    "ABORT",
    "PREPARE p AS DELETE FROM t WHERE id = $1",
    "EXECUTE p(1)",
    "DEALLOCATE p",
    "EXPLAIN ANALYZE DELETE FROM t",
    "EXPLAIN ANALYZE UPDATE t SET a = 1",
    "EXPLAIN ANALYZE SELECT * FROM read_csv('x.csv')",
    "DESCRIBE SELECT * FROM read_parquet('x.parquet')",
    "SUMMARIZE SELECT * FROM read_json('x.json')",
    "SUMMARIZE read_csv('x.csv')",
    "SHOW SELECT * FROM glob('*')",
    "SELECT * FROM read_csv('C:/Windows/win.ini')",
    "SELECT content FROM read_text('C:/Windows/win.ini')",
    "SELECT * FROM read_blob('*.bin')",
    "SELECT * FROM read_json_auto('https://example.com/x.json')",
    "SELECT * FROM read_ndjson_objects('x.ndjson')",
    "SELECT * FROM read_parquet(['a.parquet', 'b.parquet'], hive_partitioning = true)",
    "SELECT * FROM read_duckdb('other.duckdb', table_name = 't')",
    "SELECT * FROM read_xlsx('x.xlsx')",
    "SELECT * FROM parquet_scan('x.parquet')",
    "SELECT * FROM parquet_metadata('x.parquet')",
    "SELECT * FROM sniff_csv('x.csv')",
    "SELECT * FROM glob('C:/Users/*')",
    "SELECT * FROM st_read('x.shp')",
    "SELECT * FROM st_drivers()",
    "SELECT * FROM arrow_scan(1, 2, 3)",
    "SELECT * FROM postgres_scan('dbname=x', 'public', 't')",
    "SELECT * FROM postgres_query('pg', 'SELECT 1')",
    "CALL postgres_execute('pg', 'DROP TABLE t')",
    "SELECT * FROM mysql_query('my', 'SELECT 1')",
    "SELECT * FROM sqlite_scan('x.db', 't')",
    "SELECT * FROM iceberg_scan('s3://b/t')",
    "SELECT * FROM delta_scan('s3://b/t')",
    "SELECT * FROM ducklake_snapshots('lake')",
    "SELECT * FROM odbc_query(1, 'SELECT 1')",
    "SELECT * FROM query('SELECT * FROM t')",
    "SELECT * FROM query_table('t')",
    "SELECT * FROM json_execute_serialized_sql(json_serialize_sql('SELECT 1'))",
    "SELECT json_serialize_plan('SELECT * FROM read_duckdb(''other.duckdb'')')",
    "SELECT json_serialize_plan('FROM ''C:/x.csv''', skip_null := true, optimize := true)",
    "SELECT * FROM t WHERE json_serialize_plan('FROM ''x.csv''') IS NOT NULL",
    "SELECT * FROM duckdb_secrets()",
    "SELECT * FROM which_secret('s3://bucket/x.parquet', 's3')",
    "SELECT nextval('order_seq')",
    "SELECT setseed(0.42)",
    "SELECT write_log('x', level := 'warning')",
    "SELECT * FROM enable_logging()",
    "CALL disable_logging()",
    "SELECT * FROM truncate_duckdb_logs()",
    "SELECT * FROM force_checkpoint()",
    "SELECT * FROM checkpoint()",
    "SELECT getenv('HOME')",
    "SELECT * FROM duckdb_extensions()",
    "SELECT * FROM duckdb_table_sample('t')",
    "SELECT http_get('https://example.com')",
    "SELECT * FROM load_aws_credentials()",
    "SELECT * FROM my_table_macro(3)",
    "SELECT * FROM histogram('t', a)",
    "SELECT * FROM t, st_read('x.shp')",
    "FROM read_csv('x.csv')",
    "FROM st_read('x.shp') SELECT *",
    "PIVOT read_csv('x.csv') ON a USING count(*)",
    "UNPIVOT st_read('x.shp') ON COLUMNS(*)",
    "SELECT * FROM t POSITIONAL JOIN st_read('x.shp')",
    "SELECT * FROM t ASOF JOIN read_csv('x.csv') r ON t.ts >= r.ts",
    "SELECT * FROM t JOIN LATERAL st_read('x.shp') ON true",
    "SELECT * FROM \"read_csv\"('x.csv')",
    "SELECT * FROM main.\"READ_CSV\" ('x.csv')",
    "SELECT * FROM system.main.read_csv('x.csv')",
    "SELECT * FROM U&\"\\0072ead_csv\"('x.csv')",
    "SELECT \"nextval\"('s')",
    "SELECT main.nextval ('s')",
    "SELECT (SELECT count(*) FROM read_csv('x.csv'))",
    "SELECT * FROM t WHERE a IN (SELECT * FROM query('SELECT 1'))",
    "WITH f AS (FROM read_parquet('x.parquet')) SELECT * FROM f",
    "SELECT list_transform([1], x -> (SELECT count(*) FROM read_text('C:/x')))",
    "SELECT 1 UNION ALL FROM read_csv('x.csv')",
    "SELECT 1; DELETE FROM t",
    "SHOW TABLES; SET threads = 1",
    "EXPLAIN SELECT 1; DROP TABLE t",
    "SELECT * FROM t WHERE id = 1 /* /* */ DELETE FROM t */",
    "SELECT 1 -- c\rDELETE FROM t",
    // Standard strings: the backslash is plain, so the string ends there.
    "SELECT 'a\\'; DELETE FROM t; --'",
    "SELECT $$ $$; DELETE FROM t",
    "SELECT $q$ $$ $q$; DELETE FROM t",
    "SELECT 1DELETE FROM t",
    "SELECT 1e0read_csv('x.csv')",
];

fn suite() -> Suite {
    Suite::new("duckdb", &SPEC)
        .reads(READS)
        .writes(WRITES)
        .active("in a FROM-first query", "FROM t SELECT ", " t")
        .active("in a FROM-first WHERE", "FROM t WHERE ", " t")
        .active("in a PIVOT", "PIVOT t ON ", " USING sum(x)")
        .active("in a PIVOT_WIDER", "PIVOT_WIDER t ON a USING ", " t")
        .active("in an UNPIVOT", "UNPIVOT t ON ", " INTO NAME n VALUE v")
        .active("in a PIVOT_LONGER", "PIVOT_LONGER t ON ", " t")
        .active(
            "after UNION BY NAME",
            "SELECT 1 UNION BY NAME SELECT ",
            " t",
        )
        .active("in a list literal", "SELECT [1, ", "] FROM t")
        .active("in a list comprehension", "SELECT [x FOR x IN ", "] FROM t")
        .active("in a struct literal", "SELECT {'a': ", "} FROM t")
        .active("in a map literal", "SELECT MAP {'k': ", "} FROM t")
        .active(
            "in an arrow lambda",
            "SELECT list_transform(l, x -> ",
            ") FROM t",
        )
        .active(
            "in a lambda",
            "SELECT list_transform(l, lambda x: ",
            ") FROM t",
        )
        .active("after QUALIFY", "SELECT a FROM t QUALIFY ", " t")
        .active("after GROUP BY ALL", "SELECT a FROM t GROUP BY ALL ", " t")
        .active("in EXCLUDE", "SELECT * EXCLUDE (", ") FROM t")
        .active("in REPLACE", "SELECT * REPLACE (", " AS a) FROM t")
        .active("in COLUMNS", "SELECT COLUMNS(", ") FROM t")
        .active("as a cast's type", "SELECT a::", " t")
        .active("after ->>", "SELECT j->>", " t")
        .active("after a named argument", "SELECT f(a := ", ") FROM t")
        .active(
            "after ASOF JOIN … ON",
            "SELECT * FROM t ASOF JOIN u ON ",
            " t",
        )
        .active(
            "after POSITIONAL JOIN",
            "SELECT * FROM t POSITIONAL JOIN ",
            " t",
        )
        .active(
            "after USING SAMPLE",
            "SELECT * FROM t USING SAMPLE 10% ",
            " t",
        )
        .active("in an AT clause", "SELECT * FROM t AT (VERSION => ", ")")
        .active("after a ? parameter", "SELECT ? ", " t")
        .active("after a $name parameter", "SELECT $name ", " t")
        .active("after a $1 parameter", "SELECT $1 ", " t")
        .active(
            "after a string ending in a backslash",
            "SELECT 'a\\' ",
            " t",
        )
        .active("after an E-string ending in \\\\", "SELECT E'a\\\\' ", " t")
        .active("after a $$ body holding a quote", "SELECT $$ ' $$ ", " t")
        .active("after a string holding $$", "SELECT '$$' ", " t $$")
        .active("after a quoted name holding $$", "SELECT \"$$\" ", " t $$")
        .active("after a $tag$ body holding $$", "SELECT $a$ $$ $a$ ", " t")
        .active("after SHOW and ;", "SHOW TABLES; ", " t")
        .active("after SUMMARIZE and ;", "SUMMARIZE t; ", " t")
        .active("after DESCRIBE and ;", "DESCRIBE t; ", " t")
        .active("after USE and ;", "USE main; ", " t")
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
            "in a $tag$ body after a $other$",
            "SELECT $a$ $b$ ",
            " $b$ $a$ FROM x",
        )
        .inert("in an E-string past ''", "SELECT E'a'' ", " t' FROM x")
        .inert("in an E-string past \\'", "SELECT E'a\\' ", " t' FROM x")
        .inert("in a string holding $$", "SELECT '$$ ", " t' FROM x")
        .inert("in a string holding /*", "SELECT '/* ", " t' FROM x")
        .inert(
            "in a quoted name holding a quote",
            "SELECT \"a'b ",
            " t\" FROM x",
        )
        .inert("in a JSON path", "SELECT j->>'$.", "' FROM x")
        .inert("in a struct key", "SELECT {'", "': 1} FROM x")
        .inert("in a quoted struct key", "SELECT {\"", "\": 1} FROM x")
        .vocab(&[
            "FROM",
            "PIVOT",
            "UNPIVOT",
            "PIVOT_WIDER",
            "ON",
            "USING",
            "INTO",
            "NAME",
            "VALUE",
            "EXCLUDE",
            "REPLACE",
            "COLUMNS(",
            "QUALIFY",
            "GROUP BY ALL",
            "UNION BY NAME",
            "ASOF",
            "POSITIONAL",
            "SAMPLE",
            "::",
            "->>",
            "->",
            ":=",
            "LAMBDA",
            "[",
            "]",
            "{",
            "}",
            "MAP",
            "range(",
            "duckdb_tables(",
            "st_read(",
            "read_csv(",
            "nextval(",
            "currval(",
            "json_serialize_plan(",
            "json_serialize_sql(",
            "$1",
            "$name",
            "?",
            "E'a\\\\'",
            "E'a\\''",
            "$$a'b$$",
            "$q$ $$ $q$",
            "'$$'",
            "\"$$\"",
            "'a\\'",
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
