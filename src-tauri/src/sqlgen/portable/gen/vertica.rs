//! Generated cases of Vertica SQL (`dialect::vertica`, fixture
//! `tests/fixtures/classify-generated/vertica.json`).
//!
//! Vertica reads are `SELECT`, `WITH`, historical queries (`AT EPOCH LATEST SELECT …`, `AT TIME
//! '…' SELECT …`), `SHOW` and `EXPLAIN` (which never runs its statement). Everything else writes,
//! and so does a read that calls a meta-function that changes the cluster or a trainer that
//! creates a model (`write_calls`). Strings read backslashes both ways (the session's
//! `StandardConformingStrings` decides), `E'…'` strings always escape, `$$…$$` bodies are
//! strings, block comments nest, and optimizer hints are block comments.

use super::Suite;
use crate::sqlgen::dialect::vertica::SPEC;

/// Ordinary reads of a Vertica user: VMart-style star joins, analytics (windows, top-k `LIMIT …
/// OVER`, `TIMESERIES` gap filling, event-series joins, pattern matching), flex-table lookups,
/// complex types, historical queries, hints, catalog and monitor queries, the read-only
/// meta-functions, `SHOW` and every `EXPLAIN` form, and the rules' words where they are no
/// keywords.
const READS: &[&str] = &[
    "SELECT 1",
    "SELECT * FROM public.customer_dimension WHERE customer_key = 42",
    "SELECT c.customer_name, SUM(f.sales_dollar_amount) AS revenue FROM \
     store.store_sales_fact f JOIN public.customer_dimension c ON c.customer_key = f.customer_key \
     JOIN public.date_dimension d ON d.date_key = f.date_key WHERE d.calendar_year = 2024 GROUP \
     BY c.customer_name ORDER BY revenue DESC LIMIT 10",
    "SELECT store_key, product_key, SUM(sales_quantity) FROM store.store_sales_fact GROUP BY \
     ROLLUP(store_key, product_key)",
    "SELECT a, b, COUNT(*) FROM t GROUP BY GROUPING SETS ((a), (b), ())",
    "SELECT a, b, SUM(c) FROM t GROUP BY CUBE(a, b) HAVING SUM(c) > 0",
    "WITH recent AS (SELECT * FROM orders WHERE order_date > CURRENT_DATE - 30), totals AS \
     (SELECT customer_id, SUM(total) AS spent FROM recent GROUP BY customer_id) SELECT * FROM \
     totals ORDER BY spent DESC",
    "WITH /*+ENABLE_WITH_CLAUSE_MATERIALIZATION*/ w AS (SELECT a FROM t) SELECT * FROM w",
    "SELECT id, total, ROW_NUMBER() OVER (PARTITION BY customer_id ORDER BY created_at) AS rn, \
     SUM(total) OVER (ORDER BY created_at ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) AS \
     running, LAG(total, 1) OVER (ORDER BY id) FROM orders",
    "SELECT store_key, product_key, sales_dollar_amount FROM store.store_sales_fact LIMIT 3 OVER \
     (PARTITION BY store_key ORDER BY sales_dollar_amount DESC)",
    "SELECT slice_time, symbol, TS_FIRST_VALUE(bid IGNORE NULLS, 'LINEAR') AS bid FROM tickstore \
     TIMESERIES slice_time AS '3 seconds' OVER (PARTITION BY symbol ORDER BY ts)",
    "SELECT * FROM hTicks h FULL OUTER JOIN aTicks a ON (h.time INTERPOLATE PREVIOUS VALUE \
     a.time)",
    "SELECT uid, sid, ts, refurl, pageurl, action, EVENT_NAME(), PATTERN_ID(), MATCH_ID() FROM \
     clickstream_log MATCH (PARTITION BY uid, sid ORDER BY ts DEFINE Entry AS refurl NOT ILIKE \
     '%website2.com%' AND pageurl ILIKE '%website2.com%', Onsite AS pageurl ILIKE \
     '%website2.com%' AND action = 'V', Purchase AS pageurl ILIKE '%website2.com%' AND action = \
     'P' PATTERN P AS (Entry Onsite* Purchase) RESULTS ALL ROWS)",
    "SELECT CONDITIONAL_TRUE_EVENT(bid > 10.6) OVER (PARTITION BY symbol ORDER BY ts) FROM \
     tickstore",
    "SELECT MAPLOOKUP(__raw__, 'name') AS name, MAPTOSTRING(__raw__) FROM flex.raw_events",
    "SELECT MAPKEYS(__raw__) OVER (PARTITION BEST) FROM flex.raw_events",
    "SELECT EXPLODE(tags) OVER (PARTITION BEST) FROM products",
    "SELECT ARRAY[1, 2, 3], ROW(1, 'a') AS r, tags[0], CONTAINS(tags, 'sale') FROM products",
    "SELECT order_id, amount::NUMERIC(12, 2), created_at::DATE, id::VARCHAR FROM orders",
    "SELECT * FROM t WHERE name ILIKE 'a%' AND code NOT ILIKE '%x' AND REGEXP_LIKE(email, \
     '^[a-z]+@example\\.com$')",
    "SELECT * FROM t WHERE a <=> b AND c IS NOT NULL",
    "SELECT TO_CHAR(order_date, 'YYYY-MM'), DATE_TRUNC('month', order_date), DATEDIFF('day', \
     a, b), TIMESTAMPADD('hour', 1, ts), NOW(), SYSDATE, CURRENT_TIMESTAMP(3) FROM orders",
    "SELECT ADD_MONTHS(order_date, 1), LAST_DAY(order_date) FROM orders",
    "SELECT * FROM events WHERE ts > NOW() - INTERVAL '1 day' AND ts <= CLOCK_TIMESTAMP()",
    "SELECT APPROXIMATE_COUNT_DISTINCT(user_id), APPROXIMATE_MEDIAN(latency), \
     PERCENTILE_CONT(0.9) WITHIN GROUP (ORDER BY latency) OVER () FROM requests",
    "SELECT HASH(a), MD5(b), SHA256(c), TO_HEX(d), HEX_TO_BINARY('00ff'), BITCOUNT(e) FROM t",
    "SELECT * FROM t TABLESAMPLE(10)",
    "SELECT * FROM t1 WHERE id IN (SELECT id FROM t2) AND EXISTS (SELECT 1 FROM t3 WHERE \
     t3.id = t1.id)",
    "SELECT id FROM a UNION ALL SELECT id FROM b EXCEPT SELECT id FROM c INTERSECT SELECT id \
     FROM d MINUS SELECT id FROM e",
    "(SELECT 1) UNION (SELECT 2) ORDER BY 1",
    "SELECT * FROM t1 JOIN t2 USING (id) LEFT OUTER JOIN t3 ON t3.id = t1.id",
    "SELECT * FROM t ORDER BY a LIMIT 50 OFFSET 100",
    "SELECT CASE WHEN total > 100 THEN 'big' ELSE 'small' END, COALESCE(note, ''), \
     NULLIF(discount, 0), DECODE(status, 1, 'open', 'closed') FROM orders",
    "SELECT /*+ label(daily_report) */ COUNT(*) FROM orders",
    "SELECT /*+DIRECT*/ COUNT(*) FROM orders",
    "SELECT /*+ PROJS('public.orders_super') */ * FROM orders",
    "SELECT \"delete\", \"insert\".\"update\", \"CLOSE_SESSION\" FROM \"insert\"",
    "SELECT 'DELETE FROM t; DROP TABLE x' AS s, $$INSERT INTO t$$ AS d, E'it\\'s' AS e, \
     U&'d\\0061t\\+000061' AS u",
    "SELECT 'SELECT CLOSE_SESSION(1)' AS sql_text, $$SET_CONFIG_PARAMETER('a', 1)$$",
    "-- DELETE FROM t\nSELECT 1 /* DROP TABLE t; SELECT CLOSE_ALL_SESSIONS(); */",
    "SELECT created_by, insert_count, copy_of, set_name, drop_date, close_time FROM audit",
    "SELECT NEXTVAL('public.order_seq'), CURRVAL('public.order_seq')",
    "SELECT 1.5, .5, 1e3, 1.5e-3, 1E+3, X'1F', B'101' FROM t",
    "SELECT\n  o.id,\n  o.total\nFROM\n  orders AS o\nWHERE\n  o.total > 0\nORDER BY\n  o.id",
    "SELECT 1; SELECT 2;",
    "SELECT * FROM v_catalog.tables WHERE table_schema = 'public'",
    "SELECT column_name, data_type FROM v_catalog.columns WHERE table_schema = ? AND table_name \
     = ? ORDER BY ordinal_position",
    "SELECT projection_name, is_super_projection FROM v_catalog.projections WHERE \
     anchor_table_name = 'orders'",
    "SELECT session_id, user_name, current_statement FROM v_monitor.sessions",
    "SELECT anchor_table_name, SUM(used_bytes) FROM v_monitor.projection_storage GROUP BY 1",
    "SELECT VERSION(), CURRENT_DATABASE(), CURRENT_USER(), CURRENT_SCHEMA(), CURRENT_SESSION()",
    "SELECT GET_COMPLIANCE_STATUS()",
    "SELECT DISPLAY_LICENSE()",
    "SELECT GET_AHM_EPOCH(), GET_CURRENT_EPOCH(), GET_LAST_GOOD_EPOCH()",
    "SELECT GET_CONFIG_PARAMETER('MaxClientSessions')",
    "SELECT GET_PROJECTION_STATUS('public.orders_super')",
    "SELECT DUMP_LOCKTABLE()",
    "SELECT PREDICT_LINEAR_REG(x1, x2 USING PARAMETERS model_name = 'm') FROM t",
    "SELECT GET_MODEL_SUMMARY(USING PARAMETERS model_name = 'm')",
    "SELECT AWS_GET_CONFIG('aws_region')",
    "SELECT v_txtindex.READ_CONFIG_FILE(USING PARAMETERS proc_oid = '45035996274126984') OVER ()",
    "SELECT ACTIVE_SCHEDULER_NODE()",
    "SELECT ST_Distance(ST_GeomFromText('POINT(1 1)'), geom) FROM places",
    "AT EPOCH LATEST SELECT * FROM orders",
    "AT EPOCH 1234 SELECT COUNT(*) FROM orders",
    "AT TIME '2024-01-01 00:00:00' SELECT * FROM orders",
    "SHOW ALL",
    "SHOW SEARCH_PATH",
    "SHOW CURRENT ALL",
    "SHOW SESSION ALL",
    "SHOW DATABASE VMart ALL",
    "EXPLAIN SELECT * FROM orders WHERE id = 1",
    "EXPLAIN LOCAL VERBOSE SELECT COUNT(*) FROM orders",
    "EXPLAIN JSON SELECT * FROM orders",
    "EXPLAIN ANNOTATED SELECT * FROM orders",
    "EXPLAIN DELETE FROM orders WHERE id = 1",
    "EXPLAIN INSERT INTO t SELECT * FROM s",
];

/// Writes beyond the rules' words: a query's write clauses, DML of every kind (also through a
/// hint or a historical prefix), COPY and EXPORT in all their forms, DDL of Vertica's objects,
/// sessions, transactions and settings, PROFILE (which runs its statement), stored procedures,
/// directed queries, cross-database connections, the meta-functions and trainers in every
/// spelling, and text a reading or a comment rule may turn into a second statement.
const WRITES: &[&str] = &[
    "SELECT * INTO TABLE archive FROM orders",
    "SELECT * INTO TEMP TABLE scratch FROM orders",
    "SELECT id FROM orders FOR UPDATE",
    "SELECT id FROM orders WHERE id = 1 FOR UPDATE OF orders",
    "INSERT INTO t (a, b) VALUES (1, 'x')",
    "INSERT /*+DIRECT*/ INTO t SELECT * FROM s",
    "INSERT INTO t DEFAULT VALUES",
    "UPDATE t SET a = 1 WHERE id = 2",
    "UPDATE /*+DIRECT*/ t SET a = a + 1",
    "DELETE FROM t WHERE id = 1",
    "DELETE /*+DIRECT*/ FROM t",
    "MERGE INTO t USING s ON t.id = s.id WHEN MATCHED THEN UPDATE SET a = s.a WHEN NOT MATCHED \
     THEN INSERT (id, a) VALUES (s.id, s.a)",
    "WITH s AS (SELECT * FROM staging) INSERT INTO t SELECT * FROM s",
    "AT EPOCH LATEST DELETE FROM t",
    "COPY t FROM LOCAL '/tmp/data.csv' DELIMITER ',' ENCLOSED BY '\"' SKIP 1",
    "COPY t FROM 's3://bucket/data/*.parquet' PARQUET",
    "COPY t FROM STDIN",
    "COPY t FROM VERTICA other_db.public.t",
    "EXPORT TO PARQUET(directory = 's3://bucket/out') AS SELECT * FROM t",
    "EXPORT TO DELIMITED(directory = '/tmp/out') OVER (PARTITION BY a) AS SELECT * FROM t",
    "EXPORT TO VERTICA other_db.public.t AS SELECT * FROM t",
    "CONNECT TO VERTICA other_db USER dbadmin PASSWORD 'pw' ON 'host2', 5433",
    "DISCONNECT other_db",
    "CREATE TABLE t (id INT PRIMARY KEY ENABLED, name VARCHAR(80)) ORDER BY id SEGMENTED BY \
     HASH(id) ALL NODES PARTITION BY YEAR(created)",
    "CREATE TABLE t2 AS SELECT * FROM t",
    "CREATE LOCAL TEMPORARY TABLE scratch (a INT) ON COMMIT PRESERVE ROWS",
    "CREATE FLEX TABLE raw_events ()",
    "CREATE EXTERNAL TABLE ext (a INT) AS COPY FROM 's3://bucket/ext/*' PARQUET",
    "CREATE PROJECTION t_p AS SELECT * FROM t ORDER BY name UNSEGMENTED ALL NODES",
    "CREATE OR REPLACE VIEW v AS SELECT * FROM t",
    "CREATE SEQUENCE s START 1",
    "CREATE OR REPLACE PROCEDURE p() LANGUAGE PLvSQL AS $$ BEGIN PERFORM DELETE FROM t; END $$",
    "CREATE FUNCTION f(x INT) RETURN INT AS BEGIN RETURN x + 1; END",
    "ALTER TABLE t ADD COLUMN c INT DEFAULT 0",
    "ALTER TABLE t ALTER CONSTRAINT pk ENABLED",
    "ALTER SESSION SET UDPARAMETER FOR awslib aws_id = 'x'",
    "TRUNCATE TABLE t",
    "DROP TABLE t CASCADE",
    "GRANT SELECT ON t TO analyst",
    "REVOKE ALL ON SCHEMA s FROM analyst",
    "COMMENT ON TABLE t IS 'orders'",
    "CREATE USER bob IDENTIFIED BY 'pw'",
    "ALTER USER bob IDENTIFIED BY 'new' REPLACE 'old'",
    "SET SESSION AUTOCOMMIT TO ON",
    "SET SESSION CHARACTERISTICS AS TRANSACTION READ WRITE",
    "SET SEARCH_PATH TO store, public",
    "SET ROLE ALL",
    "SET TIME ZONE TO 'UTC'",
    "SET STANDARD_CONFORMING_STRINGS TO OFF",
    "COMMIT",
    "ROLLBACK",
    "BEGIN",
    "START TRANSACTION ISOLATION LEVEL SERIALIZABLE",
    "SAVEPOINT a",
    "RELEASE SAVEPOINT a",
    "LOCK TABLE t IN EXCLUSIVE MODE",
    "PROFILE SELECT * FROM t",
    "PROFILE DELETE FROM t",
    "CALL refresh_all()",
    "DO $$ BEGIN PERFORM 1; END $$",
    "SAVE QUERY SELECT * FROM t",
    "ACTIVATE DIRECTED QUERY q1",
    "SELECT CLOSE_SESSION('v_vmart_node0001-31:0x1b2')",
    "SELECT CLOSE_ALL_SESSIONS()",
    "SELECT INTERRUPT_STATEMENT('s', 1)",
    "SELECT SET_CONFIG_PARAMETER('MaxClientSessions', 100)",
    "SELECT SET_CLIENT_LABEL('x')",
    "SELECT ANALYZE_STATISTICS('public.orders')",
    "SELECT DROP_PARTITIONS('public.orders', '2020', '2021')",
    "SELECT PURGE()",
    "SELECT MAKE_AHM_NOW()",
    "SELECT DO_TM_TASK('mergeout')",
    "SELECT REFRESH('public.orders')",
    "SELECT START_REFRESH()",
    "SELECT EXPORT_OBJECTS('/tmp/ddl.sql', 'public')",
    "SELECT EXPORT_CATALOG('/tmp/catalog.sql')",
    "SELECT GET_METADATA('/data/f.parquet')",
    "SELECT INFER_EXTERNAL_TABLE_DDL('/data/*.parquet' USING PARAMETERS format = 'parquet', \
     table_name = 't')",
    "SELECT LINEAR_REG('m', 'public.t', 'y', 'x1, x2')",
    "SELECT KMEANS('k', 't', '*', 5 USING PARAMETERS max_iterations = 20)",
    "SELECT RF_CLASSIFIER('rf', 't', 'label', '*')",
    "SELECT DETECT_OUTLIERS('outliers', 't', 'x', 'robust_zscore')",
    "SELECT NORMALIZE('t_norm', 't', 'x', 'minmax')",
    "SELECT IMPORT_MODELS('/tmp/models/*' USING PARAMETERS category = 'PMML')",
    "SELECT COMPUTE_FLEXTABLE_KEYS_AND_BUILD_VIEW('raw_events')",
    "SELECT STV_Create_Index(gid, geom USING PARAMETERS index = 'i') OVER () FROM polys",
    "SELECT S3EXPORT(* USING PARAMETERS url = 's3://bucket/x.csv') OVER () FROM t",
    // Red-team: Vertica Place writes shapefiles on the server (and sets where).
    "SELECT STV_Export2Shapefile(* USING PARAMETERS shapefile = 'x.shp') OVER() FROM t",
    "SELECT \"STV_Export2Shapefile\"(*) OVER () FROM t",
    "SELECT STV_SetExportShapefileDirectory(USING PARAMETERS path = '/tmp')",
    "SELECT ADD_LOCATION('/data/v', 'v_vmart_node0001', 'DATA')",
    "SELECT MERGE_PARTITIONS('public.orders', '2020', '2021')",
    "SELECT AZURE_TOKEN_CACHE_CLEAR()",
    // Red-team round 2: Vertica Place reads a shapefile from the server or an object store, the
    // Kafka library reaches the brokers the SQL names, and AWS_SET_CONFIG sets credentials.
    "SELECT STV_ShpCreateTable(USING PARAMETERS file = '/etc/x.shp') OVER()",
    "SELECT KafkaCheckBrokers(USING PARAMETERS brokers = 'h:9092') OVER ()",
    "SELECT KafkaListTopics(USING PARAMETERS brokers = 'h:9092') OVER ()",
    "SELECT KafkaListManyTopics('h:9092') OVER ()",
    "SELECT KafkaTopicDetails(USING PARAMETERS brokers = 'h:9092', topic = 't') OVER ()",
    "SELECT KafkaExport(partition, key, value USING PARAMETERS brokers = 'h:9092', topic = 't') \
     OVER (PARTITION BEST) FROM t",
    "SELECT AWS_SET_CONFIG('aws_secret', 'x')",
    // Red-team round 3: EXECUTE_TRIGGER runs a stored procedure, DO_LOGROTATE_LOCAL deletes old
    // logs, and DELETE_TOKENIZER_CONFIG_FILE deletes a file (one word, so no DELETE keyword).
    "SELECT EXECUTE_TRIGGER('vmart.management.log_user_actions')",
    "SELECT execute_trigger('revoke_log_privileges')",
    "SELECT DO_LOGROTATE_LOCAL('force;max_age=1 day')",
    "SELECT v_txtindex.DELETE_TOKENIZER_CONFIG_FILE(USING PARAMETERS proc_oid = \
     '45035996274126984', confirm = TRUE)",
    // Red-team round 4: the Voltage SecureData library saves settings to shared DFS files and
    // calls the SecureData appliance; VALIDATE_STATISTICS reads an XML file at a server path.
    "SELECT \"VoltageSecureConfigureGlobal\"(USING PARAMETERS policy_url='u') OVER ()",
    "SELECT VoltageSecureConfigure(USING PARAMETERS config_dfs_path = '/voltagesecure/conf', \
     username = 'u', store_password = true) OVER ()",
    "SELECT VoltageSecureRefreshPolicy() OVER ()",
    "SELECT VoltageSecureProtect(ssn USING PARAMETERS format = 'ssn') FROM customers",
    "SELECT VoltageSecureAccess(ssn USING PARAMETERS format = 'ssn', mask = true) FROM customers",
    "SELECT VoltageSecureProtectAllKeys('123-45-6789' USING PARAMETERS format = 'ssn_efpe') \
     OVER ()",
    "SELECT VALIDATE_STATISTICS('/home/dbadmin/stats.xml')",
    "SELECT \"CLOSE_SESSION\"('x')",
    "SELECT v_catalog.CLOSE_SESSION('x')",
    "SELECT v_catalog.\"close_session\" ('x')",
    "SELECT U&\"\\0043LOSE_SESSION\"('x')",
    "SELECT * FROM (SELECT CLOSE_ALL_SESSIONS()) s",
    "WITH k AS (SELECT CLOSE_SESSION(session_id) FROM v_monitor.sessions) SELECT * FROM k",
    "SELECT 1; DELETE FROM t",
    "SHOW ALL; SET SEARCH_PATH TO x",
    "EXPLAIN SELECT 1; DROP TABLE t",
    "SELECT * FROM t WHERE id = 1 /* /* */ DELETE FROM t */",
    "SELECT 1 -- c\rDELETE FROM t",
    // Off, StandardConformingStrings makes the backslash escape the quote; on, it ends here.
    "SELECT 'a\\'; DELETE FROM t; --'",
    "SELECT 'a\\''; DELETE FROM t",
    "SELECT $$ $$; DELETE FROM t",
    "SELECT $q$ $$ $q$; DELETE FROM t",
    "SELECT 1DELETE FROM t",
    "SELECT 1e0CLOSE_SESSION('x')",
];

fn suite() -> Suite {
    Suite::new("vertica", &SPEC)
        .reads(READS)
        .writes(WRITES)
        .active("after AT EPOCH LATEST", "AT EPOCH LATEST SELECT 1 ", " t")
        .active("after AT TIME", "AT TIME '2024-01-01' SELECT ", " t")
        .active("as a cast's type", "SELECT a::", " t")
        .active("after a cast", "SELECT a::INT ", " t")
        .active(
            "in USING PARAMETERS",
            "SELECT f(x USING PARAMETERS a = ",
            ") FROM t",
        )
        .active(
            "after a transform's OVER",
            "SELECT EXPLODE(a) OVER (PARTITION BEST) ",
            " t",
        )
        .active(
            "after a top-k LIMIT … OVER",
            "SELECT * FROM t LIMIT 3 OVER (PARTITION BY a ORDER BY b) ",
            " t",
        )
        .active(
            "after TIMESERIES",
            "SELECT ts FROM t TIMESERIES ts AS '1 minute' OVER (ORDER BY t) ",
            " t",
        )
        .active(
            "in a MATCH clause",
            "SELECT * FROM t MATCH (PARTITION BY ",
            ")",
        )
        .active("after a hint", "SELECT /*+ label(q) */ ", " t")
        .active("glued to a hint", "SELECT /*+DIRECT*/", " t")
        .active("after ILIKE", "SELECT * FROM t WHERE a ILIKE 'x' ", " t")
        .active("after a <=> comparison", "SELECT a <=> ", " t")
        .active("in an ARRAY literal", "SELECT ARRAY[1, ", "] FROM t")
        .active("in an element access", "SELECT a[", "] FROM t")
        .active("glued to a U&'' string", "SELECT U&'a'", " t")
        .active("glued to a bit string", "SELECT B'101'", " t")
        .active(
            "after a string ending in two backslashes",
            "SELECT 'a\\\\' ",
            " t",
        )
        .active("after an E-string ending in \\\\", "SELECT E'a\\\\' ", " t")
        .active("after a $$ body holding a quote", "SELECT $$ ' $$ ", " t")
        .active("after a string holding $$", "SELECT '$$' ", " t $$")
        .active("after a quoted name holding $$", "SELECT \"$$\" ", " t $$")
        .active("after a string holding --", "SELECT '--' ", " t")
        .active("after SHOW and ;", "SHOW ALL; ", " t")
        .active("after EXPLAIN and ;", "EXPLAIN SELECT 1; ", " t")
        .active("after a $tag$ body holding $$", "SELECT $a$ $$ $a$ ", " t")
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
            "in a procedure-like $$ body",
            "SELECT $$ BEGIN PERFORM ",
            "; END $$ FROM x",
        )
        .inert("in a hint", "SELECT /*+ ", " t */ 1 FROM x")
        .inert("in a U&'' string", "SELECT U&'", " t' FROM x")
        .inert("in an E-string past ''", "SELECT E'a'' ", " t' FROM x")
        .inert(
            "in an E-string past \\\\",
            "SELECT E'a\\\\\\' ",
            " t' FROM x",
        )
        .inert("in a string holding $$", "SELECT '$$ ", " t' FROM x")
        .inert("in a string holding /*", "SELECT '/* ", " t' FROM x")
        .inert(
            "in a quoted name holding a quote",
            "SELECT \"a'b ",
            " t\" FROM x",
        )
        .inert(
            "in a $tag$ body after a $other$",
            "SELECT $a$ $b$ ",
            " $b$ $a$ FROM x",
        )
        .vocab(&[
            "::",
            "INT",
            "ILIKE",
            "TIMESERIES",
            "OVER",
            "PARTITION",
            "BEST",
            "USING",
            "PARAMETERS",
            "AT",
            "EPOCH",
            "LATEST",
            "LIMIT",
            "MATCH",
            "PROFILE",
            "INTO",
            "FOR",
            "UPDATE",
            "<=>",
            "ARRAY[",
            "]",
            "EXPLODE(",
            "MAPLOOKUP(",
            "CLOSE_SESSION(",
            "GET_COMPLIANCE_STATUS(",
            "NEXTVAL(",
            "/*+DIRECT*/",
            "/*+ label(x) */",
            "E'a\\\\'",
            "U&'x'",
            "U&\"x\"",
            "B'101'",
            "$$a'b$$",
            "$q$ $$ $q$",
            "'$$'",
            "\"$$\"",
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
