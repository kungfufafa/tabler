//! Generated cases of Snowflake SQL (`dialect::snowflake`, fixture
//! `tests/fixtures/classify-generated/snowflake.json`).
//!
//! Snowflake reads are `SELECT`, `WITH`, `SHOW`, `DESCRIBE` / `DESC` and `EXPLAIN`, all judged
//! like queries because the flow operator `->>` may chain any statement after them. Everything
//! else writes: DML, DDL, `COPY`, `PUT`, `GET`, `LIST` and `REMOVE` on stages, `USE`, `SET`,
//! transactions and Snowflake Scripting, `CALL`, `EXECUTE IMMEDIATE`, and a read that calls a
//! `SYSTEM$` function, a file or stage function, the Cortex calls that read files, run agents or
//! train models, `IDENTIFIER(…)` (it may name the function), `TO_QUERY`, `NEXTVAL`, or a table
//! function outside the allow-list (a stored procedure, a user table function, `INFER_SCHEMA`).
//! Strings always read backslash escapes, `$$…$$` bodies have no tags and no escapes (`$a$` is
//! the session variable `a$`), and `//` starts a comment.

use super::Suite;
use crate::sqlgen::dialect::snowflake::SPEC;

/// Ordinary reads of a Snowflake user: TPC-H-style joins, CTEs, windows with QUALIFY,
/// semi-structured paths and FLATTEN, Time Travel and CHANGES, streams' metadata columns,
/// sampling, PIVOT / UNPIVOT, MATCH_RECOGNIZE, CONNECT BY, ASOF joins, the allowed table
/// functions (GENERATOR, RESULT_SCAN, SPLIT_TO_TABLE, the INFORMATION_SCHEMA ones), catalog and
/// ACCOUNT_USAGE queries, context functions, GET_DDL, Cortex text functions, vectors and
/// geography, every SHOW / DESCRIBE / EXPLAIN form Tabler and users run, the flow operator
/// between reads, and the rules' words where they are no keywords.
const READS: &[&str] = &[
    "SELECT 1",
    "SELECT * FROM sales.public.orders WHERE order_id = 42",
    "SELECT c.c_name, SUM(o.o_totalprice) AS revenue FROM snowflake_sample_data.tpch_sf1.customer \
     c JOIN snowflake_sample_data.tpch_sf1.orders o ON o.o_custkey = c.c_custkey WHERE \
     o.o_orderdate >= '1995-01-01' GROUP BY c.c_name ORDER BY revenue DESC NULLS LAST LIMIT 10",
    "WITH recent AS (SELECT * FROM orders WHERE created_at > DATEADD(day, -30, \
     CURRENT_TIMESTAMP())), totals AS (SELECT customer_id, SUM(total) AS spent FROM recent GROUP \
     BY customer_id) SELECT * FROM totals ORDER BY spent DESC",
    "WITH RECURSIVE nums (n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM nums WHERE n < 10) SELECT n \
     FROM nums",
    "SELECT id, customer_id, total, ROW_NUMBER() OVER (PARTITION BY customer_id ORDER BY \
     created_at DESC) AS rn FROM orders QUALIFY rn = 1",
    "SELECT id, SUM(total) OVER (ORDER BY created_at ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT \
     ROW) AS running, LAG(total) IGNORE NULLS OVER (ORDER BY id) FROM orders",
    "SELECT v:customer.name::STRING AS name, v:items[0].sku AS sku, v['price']::NUMBER(10, 2) \
     FROM raw_events WHERE v:type = 'order'",
    "SELECT e.id, f.value:sku::STRING FROM raw_events e, LATERAL FLATTEN(input => e.v:items) f",
    "SELECT f.key, f.path, f.value FROM TABLE(FLATTEN(input => PARSE_JSON('{\"a\": [1, 2]}'), \
     recursive => TRUE)) f",
    "SELECT OBJECT_CONSTRUCT('a', 1, 'b', ARRAY_CONSTRUCT(1, 2)), ARRAY_SIZE(tags), GET(v, 'k'), \
     GET_PATH(v, 'a.b'), TRY_PARSE_JSON(s), TO_JSON(v), {'k': 1}, [1, 2] FROM docs",
    "SELECT OBJECT_INSERT(o, 'k', 1), OBJECT_DELETE(o, 'j'), ARRAY_INSERT(a, 0, 'x'), \
     MAP_DELETE(m, 'k') FROM t",
    "SELECT * FROM orders AT(OFFSET => -60 * 5)",
    "SELECT * FROM orders BEFORE(STATEMENT => '01b2c3d4-0000-1111-0000-000000000001')",
    "SELECT * FROM orders AT(TIMESTAMP => '2024-06-01 00:00:00'::TIMESTAMP_LTZ)",
    "SELECT * FROM orders CHANGES(INFORMATION => DEFAULT) AT(TIMESTAMP => DATEADD(hour, -1, \
     CURRENT_TIMESTAMP()))",
    "SELECT id, METADATA$ACTION, METADATA$ISUPDATE, METADATA$ROW_ID FROM orders_stream",
    "SELECT * FROM orders SAMPLE (10)",
    "SELECT * FROM orders TABLESAMPLE BERNOULLI (100 ROWS)",
    "SELECT * FROM monthly_sales PIVOT (SUM(amount) FOR month IN ('JAN', 'FEB', 'MAR')) AS p \
     (empid, jan, feb, mar) ORDER BY empid",
    "SELECT * FROM monthly_sales UNPIVOT (sales FOR month IN (jan, feb, mar)) ORDER BY empid",
    "SELECT * FROM stock_price_history MATCH_RECOGNIZE (PARTITION BY company ORDER BY price_date \
     MEASURES MATCH_NUMBER() AS match_number, FIRST(price_date) AS start_date ONE ROW PER MATCH \
     AFTER MATCH SKIP TO LAST up PATTERN (down+ up+) DEFINE down AS price < LAG(price), up AS \
     price > LAG(price))",
    "SELECT employee_id, manager_id, title, LEVEL FROM employees START WITH title = 'President' \
     CONNECT BY manager_id = PRIOR employee_id ORDER BY employee_id",
    "SELECT t.stock_symbol, t.trade_time, q.price FROM trades t ASOF JOIN quotes q \
     MATCH_CONDITION (t.trade_time >= q.quote_time) ON t.stock_symbol = q.stock_symbol",
    "SELECT SEQ4() AS n, UNIFORM(1, 100, RANDOM()) FROM TABLE(GENERATOR(ROWCOUNT => 1000))",
    "SELECT * FROM TABLE(RESULT_SCAN(LAST_QUERY_ID())) WHERE \"rows\" > 0",
    "SELECT * FROM TABLE(SPLIT_TO_TABLE('a,b,c', ','))",
    "SELECT t.id, s.value FROM t, TABLE(STRTOK_SPLIT_TO_TABLE(t.tags, ' ')) s",
    "SELECT query_id, query_text, execution_status FROM \
     TABLE(INFORMATION_SCHEMA.QUERY_HISTORY_BY_SESSION(RESULT_LIMIT => 100)) ORDER BY start_time \
     DESC",
    "SELECT * FROM TABLE(my_db.INFORMATION_SCHEMA.COPY_HISTORY(TABLE_NAME => 'ORDERS', \
     START_TIME => DATEADD(hours, -1, CURRENT_TIMESTAMP())))",
    "SELECT * FROM TABLE(INFORMATION_SCHEMA.TASK_HISTORY()) WHERE state = 'FAILED'",
    "SELECT * FROM TABLE(INFORMATION_SCHEMA.QUERY_HISTORY_BY_WAREHOUSE(WAREHOUSE_NAME => \
     'COMPUTE_WH'))",
    "SELECT * FROM TABLE(INFORMATION_SCHEMA.TAG_REFERENCES_ALL_COLUMNS('orders', 'table'))",
    "SELECT * FROM TABLE(INFORMATION_SCHEMA.REPLICATION_GROUP_REFRESH_PROGRESS_BY_JOB('01a2'))",
    "SELECT * FROM TABLE(INFORMATION_SCHEMA.REPLICATION_GROUP_LAG_HISTORY(REPLICATION_GROUP_NAME \
     => 'fg'))",
    "SELECT * FROM TABLE(MODEL_MONITOR_DRIFT_METRIC('mm', 'JENSEN_SHANNON', 'f', '1 DAY', \
     DATEADD(DAY, -30, CURRENT_DATE()), CURRENT_DATE()))",
    "SELECT * FROM TABLE(SNOWFLAKE.LOCAL.GET_AI_OBSERVABILITY_EVENTS('DB', 'S', 'AGENT', 'CORTEX \
     AGENT'))",
    "SELECT * FROM TABLE(GET_OBJECT_REFERENCES(DATABASE_NAME => 'DB', SCHEMA_NAME => 'S', \
     OBJECT_NAME => 'V'))",
    "SELECT * FROM SEMANTIC_VIEW(tpch_analysis DIMENSIONS customer.market_segment METRICS \
     orders.order_average_value) ORDER BY 1",
    "SELECT * FROM TABLE(my_service!SPCS_GET_LOGS()) WHERE timestamp > DATEADD(hour, -1, \
     CURRENT_TIMESTAMP())",
    "SELECT table_schema, table_name, row_count FROM my_db.information_schema.tables WHERE \
     table_schema = 'PUBLIC' ORDER BY row_count DESC",
    "SELECT column_name, data_type, is_nullable FROM information_schema.columns WHERE table_name \
     = ? ORDER BY ordinal_position",
    "SELECT warehouse_name, SUM(credits_used) FROM snowflake.account_usage.\
     warehouse_metering_history WHERE start_time > DATEADD(day, -7, CURRENT_DATE()) GROUP BY 1",
    "SELECT CURRENT_VERSION(), CURRENT_ACCOUNT(), CURRENT_REGION(), CURRENT_USER(), \
     CURRENT_ROLE(), CURRENT_WAREHOUSE(), CURRENT_DATABASE(), CURRENT_SCHEMA(), CURRENT_SESSION()",
    "SELECT GET_DDL('TABLE', '\"DB\".\"PUBLIC\".\"ORDERS\"')",
    "SELECT GET_DDL('VIEW', 'my_view', TRUE)",
    "SELECT * FROM customers WHERE name ILIKE 'a%' AND email RLIKE '^[a-z]+@example\\\\.com$' \
     AND code NOT LIKE ANY ('x%', 'y%')",
    "SELECT DATE_TRUNC('month', created_at), DATEDIFF(day, a, b), TO_CHAR(created_at, \
     'YYYY-MM'), CONVERT_TIMEZONE('UTC', 'Asia/Jakarta', created_at), TIMEADD(hour, 1, ts) FROM \
     orders",
    "SELECT EXTRACT(YEAR FROM created_at), DATE_PART(month, created_at), TRIM(name), \
     SUBSTRING(s, 1, 3) FROM orders",
    "SELECT id::VARCHAR, amount::NUMBER(12, 2), TRY_CAST(s AS INT), TO_DECIMAL(x, 10, 2), \
     CAST(ts AS DATE) FROM orders",
    "SELECT IFF(total > 100, 'big', 'small'), COALESCE(note, ''), NULLIF(discount, 0), \
     DECODE(status, 1, 'open', 'closed'), ZEROIFNULL(x), NVL2(a, b, c), TRUNC(price, 2) FROM \
     orders",
    "SELECT APPROX_COUNT_DISTINCT(user_id), APPROX_PERCENTILE(latency, 0.9), MEDIAN(latency), \
     LISTAGG(DISTINCT tag, ', ') WITHIN GROUP (ORDER BY tag), ARRAY_AGG(id) WITHIN GROUP (ORDER \
     BY id), COUNT_IF(ok), HASH_AGG(*) FROM requests",
    "SELECT region, product, SUM(amount) FROM sales GROUP BY GROUPING SETS ((region), (product), \
     ())",
    "SELECT region, SUM(amount) FROM sales GROUP BY ALL",
    "SELECT id FROM a UNION ALL SELECT id FROM b EXCEPT SELECT id FROM c INTERSECT SELECT id FROM \
     d MINUS SELECT id FROM e",
    "(SELECT 1) UNION (SELECT 2) ORDER BY 1",
    "SELECT * FROM t1 WHERE id IN (SELECT id FROM t2) AND EXISTS (SELECT 1 FROM t3 WHERE t3.id = \
     t1.id)",
    "SELECT * FROM t1 JOIN t2 USING (id) LEFT OUTER JOIN t3 ON t3.id = t1.id NATURAL JOIN t4 \
     CROSS JOIN t5",
    "SELECT * FROM t ORDER BY a LIMIT 50 OFFSET 100",
    "SELECT TOP 10 * FROM t",
    "SELECT * FROM t FETCH FIRST 5 ROWS ONLY",
    "SELECT * FROM t WHERE a IS NOT DISTINCT FROM b OR EQUAL_NULL(c, d)",
    "SELECT * FROM t WHERE id = $id_var AND name = GETVARIABLE('NAME')",
    "SELECT * FROM orders WHERE id = ? AND status = :status",
    "SELECT a.* EXCLUDE (secret), b.* RENAME (id AS b_id) FROM a JOIN b ON a.id = b.a_id",
    "SELECT * ILIKE '%name%' FROM customers",
    "SELECT TRANSFORM(arr, x INT -> x * 2), FILTER(arr, x -> x > 1) FROM t",
    "SELECT ST_DISTANCE(TO_GEOGRAPHY('POINT(1 1)'), geo), ST_ASWKT(geo), H3_LATLNG_TO_CELL(lat, \
     lng, 9) FROM places",
    "SELECT VECTOR_COSINE_SIMILARITY(embedding, [1, 2, 3]::VECTOR(FLOAT, 3)) FROM docs ORDER BY \
     1 DESC LIMIT 5",
    "SELECT HASH(a), MD5(b), SHA2(c, 256), BASE64_ENCODE(d), HEX_ENCODE(e), UUID_STRING() FROM t",
    "SELECT SNOWFLAKE.CORE.NULL_COUNT(SELECT email FROM customers)",
    "SELECT SNOWFLAKE.CORTEX.SENTIMENT(review), SNOWFLAKE.CORTEX.SUMMARIZE(body) FROM reviews",
    "SELECT m!PREDICT(INPUT_DATA => OBJECT_CONSTRUCT(*)) FROM new_rows",
    "SELECT CURRENT_TIMESTAMP(3), SYSDATE(), LOCALTIMESTAMP, CURRENT_DATE",
    "SELECT $$it's a 'quote' and DELETE FROM t$$ AS s, 'INSERT INTO t' AS d, 'it\\'s' AS e",
    "SELECT 'SELECT SYSTEM$CANCEL_ALL_QUERIES(1)' AS sql_text, $$CALL p()$$",
    // Every reading takes `\'` for a quote, so the DELETE is in the string.
    "SELECT 'a\\'; DELETE FROM t; --'",
    "SELECT $$ \\ $$, 'a\\\\' AS backslash",
    "-- DELETE FROM t\nSELECT 1 /* DROP TABLE t; SELECT SYSTEM$WAIT(1); */ // CALL p()",
    "SELECT \"delete\", \"insert\".\"update\", \"SYSTEM$WAIT\" FROM \"insert\"",
    "SELECT created_by, insert_count, copy_of, set_name, drop_date, list_price, call_time, \
     use_count, commit_ts, nextval_hint FROM audit",
    "SELECT 1.5, .5, 1e3, 1.5e-3, 1E+3 FROM t",
    "SELECT\n  o.id,\n  o.total\nFROM\n  orders AS o\nWHERE\n  o.total > 0\nORDER BY\n  o.id",
    "SELECT 1; SELECT 2;",
    "SHOW DATABASES",
    "SHOW TERSE SCHEMAS IN DATABASE \"DB\"",
    "SHOW TABLES IN SCHEMA \"DB\".\"PUBLIC\" LIMIT 10000 FROM 'ORDERS'",
    "SHOW COLUMNS IN TABLE \"DB\".\"PUBLIC\".\"ORDERS\"",
    "SHOW PRIMARY KEYS IN TABLE db.public.orders",
    "SHOW IMPORTED KEYS IN SCHEMA db.public",
    "SHOW GRANTS TO ROLE analyst",
    "SHOW GRANTS ON TABLE orders",
    "SHOW PARAMETERS LIKE 'TIMEZONE' IN SESSION",
    "SHOW WAREHOUSES LIKE 'COMPUTE%'",
    "SHOW USER FUNCTIONS IN SCHEMA s",
    "SHOW TASKS STARTS WITH 'NIGHTLY'",
    "SHOW VARIABLES",
    "SHOW TRANSACTIONS",
    "SHOW TABLES ->> SELECT \"name\", \"rows\" FROM $1 WHERE \"rows\" > 1000 ORDER BY \"rows\" DESC",
    "DESCRIBE TABLE orders",
    "DESC TABLE \"DB\".\"PUBLIC\".\"ORDERS\"",
    "DESCRIBE VIEW v",
    "DESC FUNCTION my_udf(NUMBER)",
    "DESCRIBE RESULT LAST_QUERY_ID()",
    "DESC WAREHOUSE compute_wh",
    "EXPLAIN SELECT * FROM orders WHERE id = 1",
    "EXPLAIN USING TEXT SELECT * FROM orders",
    "EXPLAIN USING JSON SELECT 1",
    "EXPLAIN USING TABULAR SELECT COUNT(*) FROM orders",
];

/// Writes beyond the rules' words: DML of every form (multi-table inserts, MERGE, UPDATE …
/// FROM), stages and files, DDL of Snowflake's objects, sessions, transactions and Snowflake
/// Scripting, CALL and EXECUTE in all their forms, the calls the sweep found (SYSTEM$, files and
/// stages, Cortex, IDENTIFIER, TO_QUERY), NEXTVAL, table functions outside the allow-list, the
/// flow operator chaining a write, and text a comment rule or a reading may turn into a second
/// statement.
const WRITES: &[&str] = &[
    "INSERT INTO t (a, b) VALUES (1, 'x')",
    "INSERT OVERWRITE INTO t SELECT * FROM s",
    "INSERT ALL INTO t1 VALUES (a) INTO t2 VALUES (b) SELECT a, b FROM s",
    "INSERT FIRST WHEN a > 1 THEN INTO t1 ELSE INTO t2 SELECT * FROM s",
    "INSERT INTO t WITH s AS (SELECT 1) SELECT * FROM s",
    "UPDATE t SET a = 1 WHERE id = 2",
    "UPDATE t SET a = s.a FROM s WHERE t.id = s.id",
    "DELETE FROM t WHERE id = 1",
    "DELETE FROM t USING s WHERE t.id = s.id",
    "MERGE INTO t USING s ON t.id = s.id WHEN MATCHED THEN UPDATE SET a = s.a WHEN NOT MATCHED \
     THEN INSERT (id, a) VALUES (s.id, s.a)",
    "TRUNCATE TABLE t",
    "TRUNCATE t",
    "COPY INTO t FROM @stage/data/ FILE_FORMAT = (TYPE = CSV SKIP_HEADER = 1)",
    "COPY INTO @stage/out/ FROM (SELECT * FROM t) FILE_FORMAT = (TYPE = PARQUET)",
    "COPY INTO 's3://bucket/path/' FROM t CREDENTIALS = (AWS_KEY_ID = 'x' AWS_SECRET_KEY = 'y')",
    "PUT file:///tmp/data.csv @~/staged AUTO_COMPRESS = TRUE",
    "GET @~/staged file:///tmp/",
    "LIST @mystage",
    "LS @~",
    "REMOVE @mystage/old/",
    "RM @%t PATTERN = '.*'",
    "CREATE TABLE t (id NUMBER AUTOINCREMENT, name VARCHAR(80)) CLUSTER BY (id)",
    "CREATE OR REPLACE TRANSIENT TABLE t2 AS SELECT * FROM t",
    "CREATE TEMPORARY TABLE scratch LIKE t",
    "CREATE TABLE t3 CLONE t AT(OFFSET => -3600)",
    "CREATE OR REPLACE VIEW v AS SELECT * FROM t",
    "CREATE SECURE MATERIALIZED VIEW mv AS SELECT a, COUNT(*) FROM t GROUP BY a",
    "CREATE OR REPLACE PROCEDURE p() RETURNS VARCHAR LANGUAGE SQL AS $$ BEGIN DELETE FROM t; \
     RETURN 'ok'; END $$",
    "CREATE FUNCTION f(x NUMBER) RETURNS NUMBER AS 'x + 1'",
    "CREATE STAGE s URL = 's3://bucket/' STORAGE_INTEGRATION = si",
    "CREATE SECRET sec TYPE = PASSWORD USERNAME = 'u' PASSWORD = 'p'",
    "CREATE WAREHOUSE wh WITH WAREHOUSE_SIZE = XSMALL",
    "CREATE OR ALTER TABLE t (a INT)",
    "ALTER TABLE t ADD COLUMN c NUMBER DEFAULT 0",
    "ALTER TABLE t SWAP WITH t2",
    "ALTER WAREHOUSE wh RESUME",
    "ALTER SESSION SET QUERY_TAG = 'x'",
    "ALTER USER u SET RSA_PUBLIC_KEY = 'MIIB'",
    "ALTER TASK t RESUME",
    "DROP TABLE t",
    "DROP DATABASE IF EXISTS d CASCADE",
    "UNDROP TABLE t",
    "COMMENT ON TABLE t IS 'orders'",
    "COMMENT IF EXISTS ON COLUMN t.a IS 'x'",
    "GRANT SELECT ON TABLE t TO ROLE analyst",
    "GRANT ROLE accountadmin TO USER bob",
    "REVOKE ALL ON SCHEMA s FROM ROLE analyst",
    "USE WAREHOUSE compute_wh",
    "USE ROLE ACCOUNTADMIN",
    "USE DATABASE d",
    "USE SCHEMA d.s",
    "USE SECONDARY ROLES ALL",
    "USE d",
    "SET x = 1",
    "SET (a, b) = (1, 2)",
    "UNSET x",
    "BEGIN",
    "BEGIN TRANSACTION",
    "BEGIN WORK",
    "START TRANSACTION",
    "COMMIT",
    "ROLLBACK",
    "BEGIN DELETE FROM t; RETURN 1; END",
    "DECLARE x INT DEFAULT 0; BEGIN x := 1; END",
    "BEGIN LET n := (SELECT COUNT(*) FROM t); RETURN n; END",
    "EXECUTE IMMEDIATE 'DELETE FROM t'",
    "EXECUTE IMMEDIATE $$ BEGIN DELETE FROM t; END; $$",
    "EXECUTE IMMEDIATE :stmt USING (a, b)",
    "EXECUTE IMMEDIATE FROM @stage/scripts/setup.sql",
    "EXECUTE TASK nightly",
    "EXECUTE ALERT a",
    "EXECUTE NOTEBOOK nb()",
    "CALL p()",
    "CALL db.s.p(1, 'x')",
    "CALL SYSTEM$SEND_EMAIL('i', 'a@b.c', 's', 'b')",
    "CALL budget!SET_SPENDING_LIMIT(100)",
    "WITH proc AS PROCEDURE () RETURNS INT LANGUAGE SQL AS $$ BEGIN RETURN 1; END $$ CALL proc()",
    "SELECT SYSTEM$CANCEL_QUERY('01b2c3d4-0000-1111')",
    "SELECT SYSTEM$CANCEL_ALL_QUERIES(12345)",
    "SELECT SYSTEM$ABORT_SESSION(12345)",
    "SELECT SYSTEM$WAIT(10)",
    "SELECT SYSTEM$CLUSTERING_INFORMATION('t')",
    "SELECT SYSTEM$TYPEOF(1)",
    "SELECT system$cancel_query ('x')",
    "SELECT \"SYSTEM$CANCEL_QUERY\"('x')",
    "SELECT db.s.SYSTEM$WAIT(1)",
    "SELECT SYSTEM$WAIT\n(1)",
    "SELECT SYSTEM$WAIT/* c */(1)",
    "SELECT * FROM TABLE(SYSTEM$DATA_METRIC_SCAN(REF_ENTITY_NAME => 't', METRIC_NAME => \
     'snowflake.core.null_count'))",
    "SELECT * FROM (SELECT SYSTEM$ABORT_TRANSACTION(1)) s",
    "WITH k AS (SELECT SYSTEM$CANCEL_QUERY(query_id) FROM q) SELECT * FROM k",
    "SELECT seq.NEXTVAL",
    "SELECT order_seq.nextval, id FROM t",
    "SELECT GET_PRESIGNED_URL(@images, 'logo.png', 3600)",
    "SELECT BUILD_SCOPED_FILE_URL(@s, 'a.pdf')",
    "SELECT BUILD_STAGE_FILE_URL(@s, 'a.pdf')",
    "SELECT GET_STAGE_LOCATION(@s)",
    "SELECT GET_RELATIVE_PATH(@s, 's3://b/a')",
    "SELECT GET_ABSOLUTE_PATH(@s, 'a')",
    "SELECT TO_FILE('@s/a.png')",
    "SELECT FL_GET_SIZE(f) FROM files",
    "SELECT SNOWFLAKE.CORE.GET_PYTHON_PROFILER_OUTPUT('01b2c3d4-0000-1111')",
    "SELECT SNOWFLAKE.CORE.GET_PYTHON_PROFILER_OUTPUT(LAST_QUERY_ID())",
    "SELECT * FROM DIRECTORY(@s)",
    "SELECT * FROM TABLE(INFER_SCHEMA(LOCATION => '@s/data/', FILE_FORMAT => 'parquet_ff'))",
    "SELECT * FROM TABLE(VALIDATE(t, JOB_ID => '_last'))",
    "SELECT AI_COMPLETE('claude-3-5-sonnet', 'hi')",
    "SELECT AI_PARSE_DOCUMENT(TO_FILE('@s', 'a.pdf'), {'mode': 'LAYOUT'})",
    "SELECT AI_CLASSIFY(review, ['good', 'bad']) FROM reviews",
    "SELECT SNOWFLAKE.CORTEX.COMPLETE('mistral-large2', 'hi')",
    "SELECT SNOWFLAKE.CORTEX.TRY_COMPLETE('m', 'x')",
    "SELECT SNOWFLAKE.CORTEX.PARSE_DOCUMENT(@s, 'a.pdf')",
    "SELECT SNOWFLAKE.CORTEX.FINETUNE('CREATE', 'm', 'base', 'SELECT 1', 'SELECT 2')",
    "SELECT SNOWFLAKE.CORTEX.DATA_AGENT_RUN('agent', '{}')",
    "SELECT EXECUTE_AI_EVALUATION('START', OBJECT_CONSTRUCT('run_name', 'r'), '@s/config.yaml')",
    "SELECT TO_QUERY('SELECT 1')",
    "SELECT * FROM TABLE(TO_QUERY('SELECT * FROM t'))",
    "SELECT IDENTIFIER('SYSTEM$CANCEL_ALL_QUERIES')(1)",
    "SELECT IDENTIFIER($fn)()",
    "SELECT * FROM IDENTIFIER($tbl)",
    "SELECT SET_SYS_CONTEXT('k', 'v')",
    "SELECT GENERATE_POSTGRES_ACCESS_TOKEN_FOR_USER('u')",
    "SELECT svc!SPCS_CANCEL_JOB()",
    "CALL ASSOCIATE_SEMANTIC_CATEGORY_TAGS('t', EXTRACT_SEMANTIC_CATEGORIES('t'))",
    "SELECT GET_ANACONDA_PACKAGES_REPODATA('linux-64')",
    "SELECT SHOW_PYTHON_PACKAGES_DEPENDENCIES('3.11', ARRAY_CONSTRUCT('numpy'))",
    "SELECT * FROM TABLE(my_proc())",
    "SELECT * FROM TABLE(db.s.my_udtf(1))",
    "SELECT * FROM t, LATERAL my_udtf(t.a)",
    "SELECT * FROM t JOIN TABLE(my_udtf(t.a)) u ON true",
    "SELECT * FROM TABLE(VALIDATE_PIPE_LOAD(PIPE_NAME => 'p', START_TIME => DATEADD(hour, -1, \
     CURRENT_TIMESTAMP())))",
    // Names no built-in table function has: Snowflake runs the user's procedure or table
    // function of that name from the current schema.
    "SELECT * FROM TABLE(GET_AI_CLEANUP())",
    "SELECT * FROM TABLE(QUERY_HISTORY_PURGE())",
    "SELECT * FROM TABLE(TAG_REFERENCES_FIX())",
    "SELECT * FROM TABLE(REPLICATION_GROUP_X())",
    "SELECT * FROM TABLE(MODEL_MONITOR_RESET())",
    "SELECT * FROM TABLE(INFORMATION_SCHEMA.QUERY_HISTORY_BY_X())",
    "SELECT * FROM TABLE(SNOWFLAKE.CORTEX.THREAD_MESSAGES(1))",
    "SELECT * FROM TABLE(SNOWFLAKE.LOCAL.GET_PYTHON_UDF_PROFILER_OUTPUT('01b2c3d4'))",
    // A procedure selected from, however it is spelt or wrapped.
    "SELECT * FROM TABLE(\"my_proc\"())",
    "SELECT * FROM TABLE(my_proc /* c */ ())",
    "SELECT * FROM TABLE(my_proc\n())",
    "SELECT * FROM t, db.s.my_proc()",
    "SELECT * FROM (TABLE(my_proc()))",
    "SELECT a FROM t UNION ALL SELECT a FROM TABLE(my_proc())",
    "SELECT * FROM t WHERE a IN (SELECT a FROM TABLE(my_proc()))",
    "SELECT * FROM TABLE(FLATTEN(input => x)) f, TABLE(my_proc()) p",
    "SHOW TABLES ->> DELETE FROM t",
    "DESC TABLE t ->> DROP TABLE t",
    "SELECT 1 ->> COMMENT ON TABLE t IS 'x'",
    "SHOW WAREHOUSES ->> ALTER WAREHOUSE wh SUSPEND",
    "EXPLAIN SELECT 1 ->> CALL p()",
    "SHOW TABLES ->> SELECT * FROM $1 ->> USE ROLE ACCOUNTADMIN",
    "EXPLAIN DELETE FROM t",
    "EXPLAIN USING TEXT INSERT INTO t SELECT 1",
    "SELECT * FROM hybrid_t WHERE id = 1 FOR UPDATE",
    "SELECT COUNT(*) INTO :n FROM t",
    "SELECT 1; DELETE FROM t",
    "SHOW TABLES; DROP TABLE t",
    "DESCRIBE TABLE t; TRUNCATE TABLE t",
    "SELECT * FROM t WHERE id = 1 /* /* */ DELETE FROM t */",
    "SELECT 1 -- c\rDELETE FROM t",
    "SELECT 1 // c\rDELETE FROM t",
    "SELECT 'a\\''; DELETE FROM t",
    "SELECT $$ $$; DELETE FROM t",
    "SELECT $$a\\$$; DELETE FROM t",
    "SELECT $a$, 1; DELETE FROM t; SELECT $a$",
    "SELECT 1DELETE FROM t",
    "SELECT 1e0SYSTEM$WAIT(1)",
];

fn suite() -> Suite {
    Suite::new("snowflake", &SPEC)
        .reads(READS)
        .writes(WRITES)
        .active("after the flow operator", "SHOW TABLES ->> ", " t")
        .active(
            "after the flow operator after a query",
            "SELECT 1 ->> ",
            " t",
        )
        .active(
            "after the flow operator after DESCRIBE",
            "DESCRIBE TABLE t ->> ",
            " t",
        )
        .active("after EXPLAIN USING TEXT", "EXPLAIN USING TEXT ", " t")
        .active("after SHOW … LIKE", "SHOW TABLES LIKE 'x' ", " t")
        .active("after a session variable", "SELECT $v ", " t")
        .active("after $a$, the variable a$", "SELECT $a$ ", " t $a$")
        .active("after a word ending in $", "SELECT a$ ", " t")
        .active("after a $$ body holding a quote", "SELECT $$ ' $$ ", " t")
        .active(
            "after a $$ body ending in a backslash",
            "SELECT $$a\\$$ ",
            " t",
        )
        .active("after a string holding $$", "SELECT '$$' ", " t $$")
        .active("after a quoted name holding $$", "SELECT \"$$\" ", " t $$")
        .active(
            "after a string ending in an escaped backslash",
            "SELECT 'a\\\\' ",
            " t",
        )
        .active("after a string holding //", "SELECT '//' ", " t")
        .active("after a cast", "SELECT a::VARIANT ", " t")
        .active("after a path and a cast", "SELECT v:a.b::STRING ", " t")
        .active("after a bracket path", "SELECT v['a'] ", " t")
        .active("in an array literal", "SELECT [1, ", "] FROM t")
        .active("in an object literal", "SELECT {'a': ", "} FROM t")
        .active("in a lambda", "SELECT TRANSFORM(a, x -> ", ") FROM t")
        .active("after a named argument", "SELECT f(a => ", ") FROM t")
        .active(
            "in FLATTEN's arguments",
            "SELECT * FROM TABLE(FLATTEN(input => ",
            ")) f",
        )
        .active("after QUALIFY", "SELECT a FROM t QUALIFY ", " t")
        .active(
            "after a Time Travel clause",
            "SELECT * FROM t AT(OFFSET => -60) ",
            " t",
        )
        .active(
            "after CONNECT BY",
            "SELECT * FROM t START WITH a = 1 CONNECT BY PRIOR a = b ",
            " t",
        )
        .active("after a stage reference", "SELECT $1 FROM @s/a.csv ", " t")
        .active("after a method call", "SELECT m!PREDICT(1) ", " t")
        .active("after EXCLUDE", "SELECT * EXCLUDE (a) ", " t")
        .active("after ILIKE", "SELECT * FROM t WHERE a ILIKE 'x' ", " t")
        .active("after a // comment and CRLF", "SELECT 1 // c\r\n", " t")
        .active("after a bind name", "SELECT :a ", " t")
        .inert(
            "in a $$ body holding a quote",
            "SELECT $$ ' ",
            " t $$ FROM x",
        )
        .inert(
            "in a $$ body holding a backslash",
            "SELECT $$ \\",
            " t $$ FROM x",
        )
        .inert(
            "in a $$ body holding a line comment",
            "SELECT $$ -- c\n",
            " t $$ FROM x",
        )
        .inert(
            "in a $$ body holding a // comment",
            "SELECT $$ // c\n",
            " t $$ FROM x",
        )
        .inert(
            "in a procedure-like $$ body",
            "SELECT $$ BEGIN ",
            "; END $$ FROM x",
        )
        .inert("in a $$ body after $a$", "SELECT $a$ $$ ", " t $$ FROM x")
        .inert(
            "in a string past an escaped backslash and quote",
            "SELECT 'a\\\\\\' ",
            " t' FROM x",
        )
        .inert("in a string holding $$", "SELECT '$$ ", " t' FROM x")
        .inert("in a string holding //", "SELECT '// ", " t' FROM x")
        .inert(
            "in a quoted name holding a quote",
            "SELECT \"a'b ",
            " t\" FROM x",
        )
        .inert(
            "in a quoted name holding a backslash",
            "SELECT \"a\\",
            " t\" FROM x",
        )
        .inert(
            "in a block comment holding //",
            "SELECT 1 /* // ",
            " t */ FROM x",
        )
        .inert(
            "in a line comment holding $$",
            "SELECT 1 -- $$ ",
            " t\nFROM x",
        )
        .inert(
            "in a // comment holding a quote",
            "SELECT 1 // ' ",
            " t\nFROM x",
        )
        .vocab(&[
            "::",
            "->>",
            "=>",
            "->",
            ":",
            ":a",
            "@s",
            "@~",
            "@%t",
            "$v",
            "$1",
            "a$",
            "$a$",
            "$$a'b$$",
            "$$a\\$$",
            "v:a.b",
            "v['a']",
            "!",
            "m!PREDICT(",
            "[",
            "]",
            "{",
            "}",
            "QUALIFY",
            "ILIKE",
            "RLIKE",
            "FLATTEN(",
            "LATERAL",
            "TABLE(",
            "GENERATOR(",
            "RESULT_SCAN(",
            "INFORMATION_SCHEMA.QUERY_HISTORY(",
            "IDENTIFIER(",
            "SYSTEM$WAIT(",
            "SYSTEM$CANCEL_QUERY",
            "NEXTVAL",
            "SHOW",
            "DESCRIBE",
            "DESC",
            "EXPLAIN",
            "USING",
            "TEXT",
            "AT(",
            "BEFORE(",
            "CHANGES(",
            "CONNECT",
            "PRIOR",
            "START",
            "EXCLUDE",
            "PIVOT",
            "UNPIVOT",
            "SUM(",
            "GET(",
            "INFER_SCHEMA(",
            "TO_FILE(",
            "AI_COMPLETE(",
            "DECLARE",
            "EXCEPTION",
            "TRANSACTION",
            "INTO",
            "FOR",
            "UPDATE",
            "'a\\''",
            "'a\\\\'",
            "\"$$\"",
            "'$$'",
            "// c\r\n",
            "-- c\r\n",
            "/* // */",
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
