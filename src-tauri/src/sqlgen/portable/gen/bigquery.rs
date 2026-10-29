//! Generated cases of GoogleSQL (`dialect::bigquery`, fixture
//! `tests/fixtures/classify-generated/bigquery.json`).
//!
//! BigQuery reads are `SELECT`, `WITH` and the pipe queries that start with `FROM`. Everything
//! else writes: DML, DDL (`UNDROP` too), DCL, `EXPORT DATA` / `EXPORT MODEL`, `LOAD DATA`,
//! `EXECUTE IMMEDIATE`, `CALL` (the `BQ.*` system procedures too), scripting and transactions,
//! `ASSERT` and GQL `GRAPH` queries (unknown statements), and a query that calls a function
//! reaching another service: federated queries, BigQuery ML and AI (remote models, Gemini, the
//! Cloud AI APIs), Cloud KMS, Sensitive Data Protection, Cloud Storage object functions, Earth
//! Engine, a `VECTOR_SEARCH` that embeds its query. Strings and backticked names always take
//! backslash escapes, also raw strings (where `\'` stays in the string), triple-quoted strings
//! hold quotes and newlines, `#` starts a comment and block comments do not nest.

use super::Suite;
use crate::sqlgen::dialect::bigquery::SPEC;

/// Ordinary reads of a BigQuery user: public datasets with dashed and backticked project ids,
/// wildcard tables and partitions, UNNEST and nested data, QUALIFY and named windows, PIVOT /
/// UNPIVOT, time travel, sampling, change history (APPENDS, CHANGES), GAP_FILL,
/// MATCH_RECOGNIZE, differential privacy, GRAPH_TABLE, pipe queries, JSON, geography, AEAD
/// keysets made in the query, search, typed literals, parameters and system variables,
/// INFORMATION_SCHEMA, and the rules' words where they are no keywords (strings of every kind,
/// backticked names, comments, longer words).
const READS: &[&str] = &[
    "SELECT 1",
    "SELECT * FROM `bigquery-public-data.samples.shakespeare` WHERE word_count > 100 LIMIT 10",
    "SELECT corpus, SUM(word_count) AS words FROM `bigquery-public-data`.samples.shakespeare \
     GROUP BY corpus ORDER BY words DESC",
    "SELECT name, SUM(number) AS total FROM bigquery-public-data.usa_names.usa_1910_2013 WHERE \
     state = 'TX' GROUP BY name ORDER BY total DESC LIMIT 10",
    "SELECT * FROM `bigquery-public-data.google_trends.top_terms` WHERE refresh_date = (SELECT \
     MAX(refresh_date) FROM `bigquery-public-data.google_trends.top_terms`)",
    "WITH recent AS (SELECT * FROM shop.orders WHERE order_date > DATE_SUB(CURRENT_DATE(), \
     INTERVAL 30 DAY)), totals AS (SELECT customer_id, SUM(total) AS spent FROM recent GROUP BY \
     customer_id) SELECT * FROM totals ORDER BY spent DESC",
    "WITH RECURSIVE nums AS (SELECT 1 AS n UNION ALL SELECT n + 1 FROM nums WHERE n < 10) SELECT \
     n FROM nums",
    "WITH a AS (SELECT 1 AS x), b AS (SELECT x + 1 AS y FROM a) SELECT * FROM a CROSS JOIN b",
    "SELECT id, customer_id, total, ROW_NUMBER() OVER (PARTITION BY customer_id ORDER BY \
     created_at DESC) AS rn FROM shop.orders WHERE TRUE QUALIFY rn = 1",
    "SELECT id, SUM(total) OVER w AS running, LAG(total) OVER (ORDER BY id) FROM shop.orders \
     WINDOW w AS (ORDER BY created_at ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW)",
    "SELECT PERCENTILE_CONT(x, 0.5) OVER (), NTILE(4) OVER (ORDER BY x), FIRST_VALUE(x IGNORE \
     NULLS) OVER (PARTITION BY g ORDER BY t ROWS BETWEEN 1 PRECEDING AND 1 FOLLOWING) FROM d.t",
    "SELECT o.id, item.sku, item.qty, pos FROM shop.orders AS o, UNNEST(o.items) AS item WITH \
     OFFSET AS pos",
    "SELECT ARRAY(SELECT AS STRUCT sku, qty FROM UNNEST(items) WHERE qty > 1) AS big FROM \
     shop.orders",
    "SELECT STRUCT(1 AS a, 'x' AS b) AS s, [1, 2, 3] AS arr, ARRAY<STRING>['a', 'b'], \
     ARRAY_LENGTH(tags), tags[OFFSET(0)], tags[SAFE_ORDINAL(2)] FROM shop.products",
    "SELECT * FROM UNNEST([STRUCT(1 AS a, 'x' AS b), (2, 'y')])",
    "SELECT x FROM UNNEST(GENERATE_ARRAY(1, 3)) AS x WHERE x > 1",
    "SELECT * EXCEPT (secret) REPLACE (UPPER(name) AS name) FROM shop.customers",
    "SELECT AS VALUE STRUCT(id, name) FROM shop.customers",
    "SELECT event_name, COUNT(*) FROM `analytics_123.events_*` WHERE _TABLE_SUFFIX BETWEEN \
     '20240101' AND '20240131' GROUP BY event_name",
    "SELECT * FROM shop.orders WHERE status = 'shipped' AND _PARTITIONTIME >= \
     TIMESTAMP('2024-01-01')",
    "SELECT * FROM shop.orders FOR SYSTEM_TIME AS OF TIMESTAMP_SUB(CURRENT_TIMESTAMP(), INTERVAL \
     1 HOUR)",
    "SELECT * FROM shop.orders TABLESAMPLE SYSTEM (10 PERCENT)",
    "SELECT * FROM (SELECT product, quarter, sales FROM shop.produce) PIVOT (SUM(sales) FOR \
     quarter IN ('Q1', 'Q2', 'Q3', 'Q4'))",
    "SELECT * FROM shop.produce UNPIVOT (sales FOR quarter IN (q1, q2, q3, q4))",
    "SELECT * FROM APPENDS(TABLE shop.orders, NULL, NULL)",
    "SELECT * FROM CHANGES(TABLE shop.orders, TIMESTAMP_SUB(CURRENT_TIMESTAMP(), INTERVAL 2 \
     HOUR), NULL)",
    "SELECT * FROM GAP_FILL(TABLE sensors.readings, ts_column => 'ts', bucket_width => INTERVAL \
     1 MINUTE, value_columns => [('temp', 'linear')])",
    "SELECT * FROM shop.sales MATCH_RECOGNIZE (PARTITION BY region ORDER BY day MEASURES \
     FIRST(day) AS start_day PATTERN (down+ up+) DEFINE down AS amount < PREV(amount), up AS \
     amount > PREV(amount))",
    "SELECT WITH DIFFERENTIAL_PRIVACY OPTIONS (epsilon = 10, delta = .01, privacy_unit_column = \
     id) item, AVG(quantity) FROM shop.produce GROUP BY item",
    "SELECT * FROM GRAPH_TABLE(finance.graph MATCH (a:Account)-[t:Transfers]->(b:Account) \
     COLUMNS (a.id AS src, b.id AS dst, t.amount))",
    "SELECT JSON_VALUE(payload, '$.user.id') AS uid, JSON_QUERY(payload, '$.items'), \
     payload.user.name, LAX_INT64(payload.qty), TO_JSON_STRING(t) FROM raw.events AS t",
    "SELECT JSON_SET(j, '$.a', 1), JSON_REMOVE(j, '$.b'), JSON_ARRAY_APPEND(j, '$.c', 2), \
     JSON_STRIP_NULLS(j), j['k'] FROM raw.docs",
    "SELECT ST_DISTANCE(ST_GEOGPOINT(lng, lat), ST_GEOGFROMTEXT('POINT(106.8 -6.2)')) AS meters, \
     ST_ASTEXT(geo) FROM geo.places WHERE ST_DWITHIN(geo, ST_GEOGPOINT(106.8, -6.2), 1000)",
    "SELECT DATE_TRUNC(order_date, MONTH), DATE_DIFF(a, b, DAY), FORMAT_TIMESTAMP('%Y-%m', \
     created_at, 'Asia/Jakarta'), TIMESTAMP_ADD(ts, INTERVAL 1 HOUR), EXTRACT(YEAR FROM ts), \
     PARSE_DATE('%Y%m%d', s) FROM shop.orders",
    "SELECT TIMESTAMP_BUCKET(ts, INTERVAL 5 MINUTE), DATE_BUCKET(d, INTERVAL 7 DAY) FROM \
     d.metrics",
    "SELECT SAFE_CAST(s AS INT64), CAST(x AS NUMERIC), CAST(y AS BIGNUMERIC), SAFE_DIVIDE(a, b), \
     IF(total > 100, 'big', 'small'), IFNULL(note, ''), NULLIF(d, 0), COALESCE(a, b) FROM \
     shop.orders",
    "SELECT CASE WHEN total > 100 THEN 'big' WHEN total > 10 THEN 'mid' ELSE 'small' END AS \
     size FROM shop.orders",
    "SELECT APPROX_COUNT_DISTINCT(user_id), APPROX_QUANTILES(latency, 100)[OFFSET(90)], \
     HLL_COUNT.EXTRACT(sketch), STRING_AGG(DISTINCT tag, ', ' ORDER BY tag), ARRAY_AGG(id IGNORE \
     NULLS LIMIT 10), COUNTIF(ok), LOGICAL_AND(ok) FROM ops.requests",
    "SELECT ANY_VALUE(name HAVING MAX ts), MAX_BY(name, ts) FROM d.t GROUP BY id",
    "SELECT region, product, SUM(amount) FROM shop.sales GROUP BY ROLLUP (region, product)",
    "SELECT region, product, SUM(amount) FROM shop.sales GROUP BY GROUPING SETS ((region), \
     (product), ())",
    "SELECT id FROM a UNION ALL SELECT id FROM b EXCEPT DISTINCT SELECT id FROM c INTERSECT \
     DISTINCT SELECT id FROM d",
    "(SELECT 1) UNION ALL (SELECT 2) ORDER BY 1",
    "SELECT * FROM t1 WHERE id IN (SELECT id FROM t2) AND EXISTS (SELECT 1 FROM t3 WHERE t3.id = \
     t1.id) AND x IN UNNEST(@ids)",
    "SELECT * FROM t1 JOIN t2 USING (id) LEFT OUTER JOIN t3 ON t3.id = t1.id CROSS JOIN t4 FULL \
     JOIN t5 ON TRUE",
    "SELECT * FROM shop.orders ORDER BY created_at DESC NULLS LAST LIMIT 50 OFFSET 100",
    "SELECT * FROM t WHERE a IS NOT DISTINCT FROM b AND c IS DISTINCT FROM d",
    "SELECT * FROM shop.orders WHERE id = @id AND status IN UNNEST(@statuses) AND created_at > \
     @since",
    "SELECT @@project_id, @@dataset_id, @@location, @@session_id, @@current_job_id, @@time_zone",
    "SELECT SESSION_USER(), CURRENT_TIMESTAMP(), CURRENT_DATETIME('Asia/Jakarta'), \
     GENERATE_UUID(), RAND()",
    "SELECT GENERATE_ARRAY(1, 10), GENERATE_DATE_ARRAY('2024-01-01', '2024-01-31', INTERVAL 1 \
     DAY), GENERATE_TIMESTAMP_ARRAY(a, b, INTERVAL 1 HOUR) FROM d.t",
    "SELECT FARM_FINGERPRINT(s), MD5(s), SHA256(b), TO_HEX(b), FROM_BASE64(s), NET.HOST(url), \
     NET.REG_DOMAIN(url) FROM web.hits",
    "SELECT REGEXP_CONTAINS(email, r'^[a-z]+@example\\.com$'), REGEXP_EXTRACT(s, r'(\\d+)'), \
     LOWER(name) LIKE 'a%', name LIKE 'x\\\\_%' FROM shop.customers",
    "SELECT TRANSLATE(s, 'abc', 'xyz'), CONTAINS_SUBSTR(t, 'x'), EDIT_DISTANCE(a, b), \
     SOUNDEX(name) FROM logs.lines AS t",
    "SELECT * FROM d.t WHERE SEARCH(d.t, 'needle', analyzer => 'LOG_ANALYZER')",
    "SELECT COSINE_DISTANCE(a.embedding, b.embedding), EUCLIDEAN_DISTANCE(a.v, b.v) FROM d.docs \
     a CROSS JOIN d.docs b",
    "SELECT name, COLLATE(name, 'und:ci') = 'abc' FROM shop.customers",
    "SELECT RANGE_START(r), RANGE_CONTAINS(r, CURRENT_DATE()), GENERATE_RANGE_ARRAY(r, INTERVAL \
     1 DAY) FROM d.periods",
    "SELECT ARRAY_TO_STRING(ARRAY_REVERSE(SPLIT(s, ',')), '|'), ARRAY_CONCAT([1], [2]), \
     ARRAY_FIRST(a), ARRAY_SLICE(a, 1, 3) FROM d.t",
    "SELECT TYPEOF(x), ERROR('stop') FROM d.t WHERE FALSE",
    "SELECT KEYS.NEW_KEYSET('AEAD_AES_GCM_256') AS keyset",
    "SELECT AEAD.DECRYPT_STRING(k.keyset, c.secret, CAST(c.id AS STRING)) FROM d.ciphers AS c \
     JOIN d.keys AS k USING (id)",
    "SELECT bqutil.fn.median([1, 2, 3]), `ml-project`.shop.fn(1)",
    "SELECT table_name, table_type, creation_time FROM `my-project`.shop.INFORMATION_SCHEMA.TABLES \
     ORDER BY table_name",
    "SELECT column_name, data_type, is_nullable FROM shop.INFORMATION_SCHEMA.COLUMNS WHERE \
     table_name = @name ORDER BY ordinal_position",
    "SELECT job_id, user_email, state, total_bytes_processed FROM \
     `region-us`.INFORMATION_SCHEMA.JOBS_BY_PROJECT WHERE creation_time > \
     TIMESTAMP_SUB(CURRENT_TIMESTAMP(), INTERVAL 1 DAY) AND state != 'DONE'",
    "SELECT ddl FROM `p`.`shop`.INFORMATION_SCHEMA.TABLES WHERE table_name = 'orders'",
    "SELECT `delete`, `insert`.`update`, `ml`, `ai`, `set`, `begin` FROM `my-proj.drop.create`",
    "SELECT 'DELETE FROM t; DROP TABLE x' AS s, \"INSERT INTO t\" AS d, '''EXECUTE IMMEDIATE 'x'; \
     CALL p()''' AS tq, r'\\d+; DELETE' AS r, b'DROP' AS b",
    "SELECT \"\"\"multi\nline; DELETE FROM t\"\"\" AS doc, R\"\"\"raw \\\" ; MERGE\"\"\" AS raw, \
     r'\\'; DECLARE' AS q",
    "SELECT 'ML.PREDICT(x)', \"AI.GENERATE('y')\", `EXTERNAL_QUERY` FROM t",
    "-- DELETE FROM t\nSELECT 1 /* DROP TABLE t; CALL p(); */ # EXECUTE IMMEDIATE 'x'\n",
    "SELECT created_by, insert_count, update_time, deleted_at, merge_key, call_count, set_name, \
     load_date, export_flag, begin_ts, html, mail FROM audit.log",
    "SELECT 1.5, .5, 1e3, 1.5e-3, 1E+3, 0x1F, NUMERIC '1.23', BIGNUMERIC '1e40', DATE \
     '2024-01-01', DATETIME '2024-01-01 10:00:00', TIMESTAMP '2024-01-01 00:00:00+07', TIME \
     '10:00:00', JSON '{\"a\": 1}', RANGE<DATE> '[2024-01-01, 2024-02-01)', INTERVAL '1-2 3 \
     4:5:6' YEAR TO SECOND",
    "SELECT\n  o.id,\n  o.total\nFROM\n  shop.orders AS o\nWHERE\n  o.total > 0\nORDER BY\n  o.id",
    "SELECT 1; SELECT 2;",
    "FROM shop.orders",
    "FROM shop.orders |> WHERE status = 'paid' |> AGGREGATE SUM(total) AS revenue GROUP BY \
     customer_id |> ORDER BY revenue DESC |> LIMIT 10",
    "FROM shop.orders AS o |> JOIN shop.customers AS c ON c.id = o.customer_id |> EXTEND o.total \
     * 1.1 AS gross |> SELECT c.name, gross",
    "FROM shop.produce |> PIVOT (SUM(sales) FOR quarter IN ('Q1', 'Q2')) |> TABLESAMPLE SYSTEM \
     (50 PERCENT)",
    "SELECT * FROM shop.orders |> WHERE total > 100 |> RENAME total AS amount |> AS t |> DISTINCT",
];

/// Writes beyond the rules' words: DML, DDL and DCL of BigQuery's objects (models, remote
/// functions, connections' tables, snapshots, indexes, policies, graphs, project options),
/// export and load, every statement that runs another one, scripting blocks, transactions,
/// unknown statements that only read (`ASSERT`, GQL), the write calls in every spelling, pipe
/// operators that run a function or reuse a write word, and text a reading or a comment rule may
/// turn into a second statement.
const WRITES: &[&str] = &[
    "INSERT INTO shop.orders (id, total) VALUES (1, 9.5)",
    "INSERT shop.orders (id) VALUES (1)",
    "INSERT shop.orders SELECT * FROM staging.orders",
    "UPDATE shop.orders SET total = 0 WHERE id = 1",
    "UPDATE shop.orders o SET o.status = 'x' FROM staging.s WHERE o.id = s.id",
    "DELETE FROM shop.orders WHERE id = 1",
    "DELETE shop.orders WHERE TRUE",
    "MERGE shop.orders T USING staging.orders S ON T.id = S.id WHEN MATCHED THEN UPDATE SET total \
     = S.total WHEN NOT MATCHED THEN INSERT ROW WHEN NOT MATCHED BY SOURCE THEN DELETE",
    "TRUNCATE TABLE shop.orders",
    "CREATE TABLE d.t (id INT64 NOT NULL, name STRING) PARTITION BY DATE(_PARTITIONTIME) CLUSTER \
     BY id OPTIONS (description = 'x')",
    "CREATE OR REPLACE TABLE d.t2 AS SELECT * FROM d.t",
    "CREATE TEMP TABLE scratch AS SELECT 1 AS x",
    "CREATE TEMP FUNCTION f(x INT64) AS (x + 1); SELECT f(1)",
    "CREATE TEMP FUNCTION js(x STRING) RETURNS STRING LANGUAGE js AS r\"\"\"return x;\"\"\"; \
     SELECT js('a')",
    "CREATE OR REPLACE FUNCTION d.remote_fn(x STRING) RETURNS STRING REMOTE WITH CONNECTION \
     `us.conn` OPTIONS (endpoint = 'https://example.com/f')",
    "CREATE MODEL d.m OPTIONS (model_type = 'linear_reg') AS SELECT x, y AS label FROM d.t",
    "CREATE OR REPLACE MODEL d.gemini REMOTE WITH CONNECTION `us.conn` OPTIONS (endpoint = \
     'gemini-2.0-flash')",
    "CREATE EXTERNAL TABLE d.ext OPTIONS (format = 'CSV', uris = ['gs://b/*.csv'])",
    "CREATE SNAPSHOT TABLE d.snap CLONE d.t",
    "CREATE MATERIALIZED VIEW d.mv AS SELECT id FROM d.t",
    "CREATE SCHEMA d2 OPTIONS (location = 'US')",
    "CREATE ROW ACCESS POLICY p ON d.t GRANT TO ('user:a@example.com') FILTER USING (region = \
     'US')",
    "CREATE SEARCH INDEX i ON d.t (ALL COLUMNS)",
    "CREATE VECTOR INDEX vi ON d.docs (embedding) OPTIONS (index_type = 'IVF')",
    "CREATE PROPERTY GRAPH finance.graph NODE TABLES (finance.account) EDGE TABLES \
     (finance.transfer SOURCE KEY (src) REFERENCES account (id) DESTINATION KEY (dst) REFERENCES \
     account (id))",
    "ALTER TABLE d.t ADD COLUMN c STRING",
    "ALTER TABLE d.t SET OPTIONS (expiration_timestamp = TIMESTAMP '2030-01-01')",
    "ALTER SCHEMA d SET DEFAULT COLLATE 'und:ci'",
    "ALTER PROJECT `my-proj` SET OPTIONS (`region-us.default_time_zone` = 'Asia/Jakarta')",
    "DROP TABLE d.t",
    "DROP SCHEMA d CASCADE",
    "DROP SNAPSHOT TABLE d.snap",
    "DROP ALL ROW ACCESS POLICIES ON d.t",
    "UNDROP SCHEMA d",
    "GRANT `roles/bigquery.dataViewer` ON SCHEMA d TO 'user:a@example.com'",
    "REVOKE `roles/bigquery.dataViewer` ON TABLE d.t FROM 'group:g@example.com'",
    "EXPORT DATA OPTIONS (uri = 'gs://bucket/out/*.parquet', format = 'PARQUET', overwrite = \
     TRUE) AS SELECT * FROM d.t",
    "EXPORT DATA WITH CONNECTION `us.conn` OPTIONS (uri = 's3://b/*', format = 'JSON') AS SELECT 1",
    "EXPORT MODEL d.m OPTIONS (uri = 'gs://b/m')",
    "LOAD DATA INTO d.t FROM FILES (format = 'CSV', uris = ['gs://b/x.csv'])",
    "LOAD DATA OVERWRITE d.t FROM FILES (format = 'PARQUET', uris = ['gs://b/*.parquet']) WITH \
     CONNECTION `us.conn`",
    "DECLARE x INT64 DEFAULT 0",
    "SET x = (SELECT COUNT(*) FROM d.t)",
    "SET @@query_label = 'team:data'",
    "SET @@dataset_project_id = 'other-proj'",
    "EXECUTE IMMEDIATE 'DELETE FROM d.t WHERE TRUE'",
    "EXECUTE IMMEDIATE CONCAT('DROP TABLE ', name)",
    "EXECUTE IMMEDIATE @sql USING 1 AS x",
    "EXECUTE IMMEDIATE 'SELECT 1' INTO x",
    "CALL d.refresh_all()",
    "CALL BQ.REFRESH_MATERIALIZED_VIEW('p.d.mv')",
    "CALL BQ.JOBS.CANCEL('job_123')",
    "CALL BQ.ABORT_SESSION()",
    "BEGIN SELECT 1; END",
    "BEGIN DELETE FROM d.t WHERE TRUE; EXCEPTION WHEN ERROR THEN SELECT @@error.message; END",
    "BEGIN TRANSACTION; DELETE FROM d.t WHERE id = 1; COMMIT TRANSACTION",
    "BEGIN TRANSACTION",
    "COMMIT TRANSACTION",
    "ROLLBACK TRANSACTION",
    "IF (SELECT COUNT(*) FROM d.t) > 0 THEN SELECT 1; END IF",
    "LOOP SET i = i + 1; IF i > 3 THEN LEAVE; END IF; END LOOP",
    "WHILE i < 3 DO SET i = i + 1; END WHILE",
    "REPEAT SET i = i + 1; UNTIL i >= 3 END REPEAT",
    "FOR r IN (SELECT id FROM d.t) DO SELECT r.id; END FOR",
    "CASE WHEN x > 0 THEN SELECT 1; ELSE SELECT 2; END CASE",
    "label_1: BEGIN SELECT 1; END label_1",
    "RAISE USING MESSAGE = 'stop'",
    "RETURN",
    "BREAK",
    "ASSERT (SELECT COUNT(*) FROM d.t) > 0 AS 'empty'",
    "GRAPH finance.graph MATCH (a)-[t]->(b) RETURN a.id, b.id",
    "SELECT * FROM EXTERNAL_QUERY('my-proj.us.pg', 'SELECT * FROM users')",
    "SELECT * FROM EXTERNAL_QUERY(\"us.conn\", '''DELETE FROM users''')",
    "WITH x AS (SELECT 1) SELECT * FROM EXTERNAL_QUERY('c', 'q')",
    "SELECT * FROM ML.PREDICT(MODEL d.m, TABLE d.t)",
    "SELECT * FROM (SELECT * FROM ML.PREDICT(MODEL d.m, TABLE d.t))",
    "SELECT * FROM ML.GENERATE_TEXT(MODEL d.gemini, (SELECT 'hi' AS prompt))",
    "SELECT * FROM ML.TRANSLATE(MODEL d.tr, TABLE d.docs, STRUCT('translate_text' AS \
     translate_mode, 'en' AS target_language_code))",
    "SELECT ML.BUCKETIZE(x, [1, 2]) FROM d.t",
    "SELECT AI.GENERATE('Summarize: ' || body, connection_id => 'us.conn') FROM d.docs",
    "SELECT AI.IF(('Is this spam? ', body), connection_id => 'us.conn') FROM d.mail",
    "SELECT * FROM d.mail WHERE AI.IF(('spam?', body))",
    "SELECT AI.SCORE(('rate', review)) FROM d.reviews",
    "SELECT * FROM AI.SEARCH(TABLE d.docs, 'body', 'cats')",
    "SELECT AI.TRANSLATE(body, 'en') FROM d.docs",
    "SELECT SAFE.AI.GENERATE_BOOL(('x', y)) FROM d.t",
    "SELECT * FROM `ml`.`predict`(MODEL d.m, TABLE d.t)",
    "SELECT * FROM `ML.GENERATE_TEXT`(MODEL d.m, TABLE d.t)",
    "SELECT * FROM `ml`.generate_text(MODEL d.m, TABLE d.t)",
    "SELECT `ai`.generate('x')",
    "SELECT * FROM VECTOR_SEARCH(TABLE d.docs, 'embedding', query_value => 'cats', top_k => 5)",
    "SELECT * FROM EXTERNAL_OBJECT_TRANSFORM(TABLE d.images, ['SIGNED_URL'])",
    "SELECT ST_REGIONSTATS(geo, 'ee://COPERNICUS/S2') FROM d.areas",
    "SELECT DETERMINISTIC_DECRYPT_STRING(KEYS.KEYSET_CHAIN('gcp-kms://projects/p/locations/us/\
     keyRings/r/cryptoKeys/k', b'wrapped'), c, '') FROM d.t",
    "SELECT KEYS.NEW_WRAPPED_KEYSET('gcp-kms://k', 'AEAD_AES_GCM_256')",
    "SELECT `keys.keyset_chain`('gcp-kms://k', b'x')",
    // With the SAFE. prefix, quoted in one piece, in parts or not at all.
    "SELECT `safe.st_regionstats`(geo, 'ee://x') FROM t",
    "SELECT `safe.dlp_key_chain`('gcp-kms://k', b'x')",
    "SELECT `safe.dlp_deterministic_encrypt`(k, s, 'ctx') FROM d.t",
    "SELECT `safe.keys.keyset_chain`('gcp-kms://k', b'x')",
    "SELECT `SAFE.KEYS.NEW_WRAPPED_KEYSET`('gcp-kms://k', 'AEAD_AES_GCM_256')",
    "SELECT `safe.keys.rewrap_keyset`('gcp-kms://k', 'gcp-kms://k2', b'x')",
    "SELECT `safe.keys.rotate_wrapped_keyset`('gcp-kms://k', b'x', 'AEAD_AES_GCM_256')",
    "SELECT `safe.ai.generate`('x')",
    "SELECT `safe.ai.if`(('spam?', body)) FROM d.mail",
    "SELECT `safe.ml.generate_text`(1)",
    "SELECT * FROM `safe.ml.predict`(MODEL d.m, TABLE d.t)",
    "SELECT `safe.obj.get_access_url`(r, 'rw')",
    "SELECT `safe.obj.get_read_url`(r) FROM d.files",
    "SELECT `safe.obj.fetch_metadata`(r) FROM d.files",
    "SELECT * FROM `safe.obj.list`('gs://bucket/*', 'us.conn')",
    "SELECT * FROM `safe.external_query`('c', 'q')",
    "SELECT * FROM `safe.vector_search`(TABLE d.docs, 'embedding', query_value => 'cats')",
    "SELECT * FROM `safe.external_object_transform`(TABLE d.images, ['SIGNED_URL'])",
    "SELECT SAFE.ST_REGIONSTATS(geo, 'ee://x') FROM t",
    "SELECT SAFE.DLP_KEY_CHAIN('gcp-kms://k', b'x')",
    "SELECT safe.keys.keyset_chain('gcp-kms://k', b'x')",
    "SELECT SAFE.OBJ.GET_ACCESS_URL(r, 'rw') FROM t",
    "SELECT `safe`.`st_regionstats`(geo, 'ee://x') FROM t",
    "SELECT `safe.keys`.keyset_chain('gcp-kms://k', b'x')",
    "SELECT * FROM `safe`.`ml.predict`(MODEL d.m, TABLE d.t)",
    "SELECT safe.`ai.generate`('x')",
    "SELECT DLP_DETERMINISTIC_ENCRYPT(DLP_KEY_CHAIN('gcp-kms://k', b'x'), s, 'ctx') FROM d.t",
    "SELECT OBJ.GET_ACCESS_URL(ref, 'r') FROM d.objects",
    "SELECT * FROM OBJ.LIST('gs://bucket/*', 'us.conn')",
    "SELECT OBJ.FETCH_METADATA(OBJ.MAKE_REF(uri, 'us.conn')) FROM d.files",
    "FROM d.t |> CALL d.tvf(1)",
    // Reads that reuse a write word: false writes the rules accept (docs/drivers/bigquery.md).
    "FROM d.t |> SET a = 1",
    "FROM d.t |> DROP secret",
    "SELECT HLL_COUNT.MERGE(sketch) FROM d.sketches",
    "SELECT 1; DELETE FROM d.t WHERE TRUE",
    "SELECT 1; EXECUTE IMMEDIATE 'x'",
    "SELECT r'\\', ' ; DELETE FROM t; --'",
    "SELECT r\"\\\", \" ; DELETE FROM t; #\"",
    "SELECT rb'\\', ' ; DELETE FROM t; --'",
    "SELECT '''a''' ; DELETE FROM t; --'''",
    "SELECT 'a\\\\'; DELETE FROM t; --'",
    "SELECT `a\\\\`; DELETE FROM t",
    "SELECT 1 # c\nDELETE FROM t",
    "SELECT 1 -- c\rDELETE FROM t",
    "SELECT * FROM t WHERE id = 1 /* /* */ DELETE FROM t */",
    "SELECT 1DELETE FROM t",
    "SELECT 1e0EXTERNAL_QUERY('c', 'q')",
    // A separately quoted qualifier of an AI or ML function (`AI.*`, `ML.*` match a qualified
    // callee's last parts), and a quoted name that takes escapes (it may spell any function).
    "SELECT `ai`.if(('spam?', body)) FROM d.mail",
    "SELECT `safe.ai`.if(('spam?', body)) FROM d.mail",
    "SELECT `ai`.translate(body, 'en') FROM d.docs",
    "SELECT * FROM `ml`.translate(MODEL d.tr, TABLE d.docs, STRUCT('en' AS x))",
    "SELECT * FROM `ai`.search(TABLE d.docs, 'body', 'cats')",
    "SELECT `ai`.agg(('sum up', body)) FROM d.docs",
    "SELECT `st_regionstat\\u0073`(geo, 'ee://x') FROM t",
    "SELECT * FROM `external_quer\\u0079`('c', 'q')",
    "SELECT * FROM `\\u006dl.predict`(MODEL d.m, TABLE d.t)",
    "SELECT `keys`.`keyset_chai\\u006e`('gcp-kms://k', b'x')",
];

fn suite() -> Suite {
    Suite::new("bigquery", &SPEC)
        .reads(READS)
        .writes(WRITES)
        .active("after a pipe operator", "FROM t |> WHERE a = 1 |> ", " t")
        .active("in a pipe SELECT", "FROM t |> SELECT ", " t")
        .active("after SAFE.", "SELECT SAFE.", " t")
        .active(
            "after a raw string ending in \\\\",
            "SELECT r'a\\\\' ",
            " t",
        )
        .active("after a raw string holding \\'", "SELECT r'\\'' ", " t")
        .active(
            "after a raw \"string\" holding \\\"",
            "SELECT R\"\\\"\" ",
            " t",
        )
        .active(
            "after a raw triple-quoted string holding \\'",
            "SELECT r'''a\\''' ''' ",
            " t",
        )
        .active(
            "after a triple-quoted string holding a quote",
            "SELECT '''it's''' ",
            " t",
        )
        .active(
            "after a \"\"\"string\"\"\" holding '''",
            "SELECT \"\"\"a'''b\"\"\" ",
            " t",
        )
        .active(
            "after a byte string ending in \\\\",
            "SELECT b'\\\\' ",
            " t",
        )
        .active(
            "after a backticked path",
            "SELECT * FROM `my-proj.ds.t` ",
            " t",
        )
        .active(
            "after a dashed project name",
            "SELECT * FROM my-proj.ds.t ",
            " t",
        )
        .active("after a named parameter", "SELECT @p1 ", " t")
        .active("after a system variable", "SELECT @@dataset_id ", " t")
        .active("in an array literal", "SELECT [1, ", "] FROM t")
        .active("in a STRUCT", "SELECT STRUCT(1 AS a, ", ") FROM t")
        .active("after UNNEST", "SELECT * FROM UNNEST([1]) ", " t")
        .active(
            "after FOR SYSTEM_TIME AS OF",
            "SELECT * FROM t FOR SYSTEM_TIME AS OF CURRENT_TIMESTAMP() ",
            " t",
        )
        .active(
            "after QUALIFY",
            "SELECT a FROM t QUALIFY ROW_NUMBER() OVER () = 1 ",
            " t",
        )
        .active("after a typed literal", "SELECT JSON '{}' ", " t")
        .active("after an array subscript", "SELECT a[OFFSET(0)] ", " t")
        .active("in a named argument", "SELECT f(a => ", ") FROM t")
        .active("after a # comment and CRLF", "SELECT 1 # c\r\n", " t")
        .active("after a -- with no space", "SELECT 1 --c\n", " t")
        .active("after a CASE … END", "SELECT CASE WHEN a THEN 1 END ", " t")
        .active(
            "in a GRAPH_TABLE",
            "SELECT * FROM GRAPH_TABLE(g MATCH (a) COLUMNS (",
            "))",
        )
        .inert("in an r'string'", "SELECT r'", " t' FROM x")
        .inert("in an R\"string\"", "SELECT R\"", " t\" FROM x")
        .inert("in an rb'string'", "SELECT rb'", " t' FROM x")
        .inert("in a bR\"string\"", "SELECT bR\"", " t\" FROM x")
        .inert("in a raw string past \\'", "SELECT r'a\\' ", " t' FROM x")
        .inert(
            "in a raw \"string\" past \\\"",
            "SELECT r\"a\\\" ",
            " t\" FROM x",
        )
        .inert(
            "in a raw triple-quoted string",
            "SELECT r'''",
            " t''' FROM x",
        )
        .inert(
            "in a raw triple-quoted string past \\'''",
            "SELECT r'''a\\''' ",
            " t''' FROM x",
        )
        .inert(
            "in a triple-quoted string over lines",
            "SELECT '''it's\n",
            "\n t''' FROM x",
        )
        .inert(
            "in a \"\"\"string\"\"\" holding '''",
            "SELECT \"\"\" ''' ",
            " t\"\"\" FROM x",
        )
        .inert(
            "in a triple-quoted string past \\'''",
            "SELECT '''a\\''' ",
            " t''' FROM x",
        )
        .inert(
            "in a byte triple-quoted string",
            "SELECT b'''",
            " t''' FROM x",
        )
        .inert("in a backticked path", "SELECT 1 FROM `my-proj.", ".t`")
        .inert("in a string holding #", "SELECT '# ", " t' FROM x")
        .inert(
            "in a string holding a backtick",
            "SELECT 'a` ",
            " t' FROM x",
        )
        .inert(
            "in a backticked name holding a quote",
            "SELECT `a'b ",
            " t` FROM x",
        )
        .inert("in a -- comment with no space", "SELECT 1 --", "\nFROM x")
        .inert("in a # comment at the start", "# ", "\nSELECT 1")
        .inert(
            "in a block comment holding --",
            "SELECT 1 /* -- ",
            " */ FROM x",
        )
        .vocab(&[
            "|>",
            "@p1",
            "@@dataset_id",
            "SAFE.",
            "UNNEST(",
            "STRUCT(",
            "[",
            "]",
            "OFFSET(",
            "=>",
            "QUALIFY",
            "FOR",
            "SYSTEM_TIME",
            "AS",
            "OF",
            "EXECUTE",
            "IMMEDIATE",
            "BEGIN",
            "END",
            "IF",
            "LOOP",
            "EXCEPTION",
            "TRANSACTION",
            "MODEL",
            "TABLE",
            "IF(",
            "TRANSLATE(",
            "SEARCH(",
            "ML.PREDICT(",
            "AI.IF(",
            "`ml`.predict(",
            "EXTERNAL_QUERY(",
            "KEYS.KEYSET_CHAIN(",
            "KEYS.NEW_KEYSET(",
            "GRAPH_TABLE(",
            "`my-proj.ds.t`",
            "`a\\``",
            "my-proj.ds.t",
            "r'\\''",
            "R\"\\\"\"",
            "rb'x'",
            "b'\\x00'",
            "'''a'b'''",
            "\"\"\"a\"b\"\"\"",
            "r'''\\''''",
            "# c\n",
            "--c\n",
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
