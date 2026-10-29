//! Generated cases of Redshift SQL (`dialect::redshift`, fixture
//! `tests/fixtures/classify-generated/redshift.json`).
//!
//! Redshift reads are `SELECT`, `WITH`, `SHOW` (tables, views, columns, grants, settings) and
//! `EXPLAIN` (which never runs its statement; there is no EXPLAIN ANALYZE). Everything else
//! writes: `UNLOAD ('SELECT …') TO 's3://…'`, whose query is a string, `COPY … FROM 's3://…'`,
//! `CALL`, `CREATE TEMP TABLE … AS`, transactions, settings, cursors, `CANCEL`. So does a query
//! with `INTO` (`SELECT … INTO #t`) or a call of a system administration function (ending
//! sessions, settings, WLM priorities, a cluster reboot: `pg_terminate_backend`,
//! `pg_cancel_backend`, `set_config`, `change_*_priority`, `reboot_cluster`) or one that reaches
//! server files, signals the server or moves a sequence (the large object and generic file
//! access functions, `pg_reload_conf`, `nextval`), also in attribute notation
//! (`(123).pg_terminate_backend` is `pg_terminate_backend(123)`). Strings read backslashes as
//! escapes (and are also read as if they did not, so a read is one either way), `E'…'` is not a
//! string form of its own, `$$…$$` and `$tag$…$tag$` bodies are strings, and block comments nest.

use super::Suite;
use crate::sqlgen::dialect::redshift::SPEC;

/// Ordinary reads of a Redshift user: the TICKIT sample's joins and rankings, Redshift's
/// analytics (QUALIFY, LISTAGG, APPROXIMATE COUNT, MEDIAN, PIVOT), SUPER navigation with PartiQL,
/// spatial and HyperLogLog functions, cross-database and Spectrum tables, the system tables and
/// views, every SHOW and EXPLAIN form, and the rules' words where they are no keywords.
const READS: &[&str] = &[
    "SELECT 1",
    "SELECT * FROM public.sales WHERE salesid = 42",
    "SELECT sellerid, username, (firstname || ' ' || lastname) AS name, city, sum(qtysold) FROM \
     sales, date, users WHERE sales.sellerid = users.userid AND sales.dateid = date.dateid AND \
     year = 2008 AND city = 'San Diego' GROUP BY sellerid, username, name, city ORDER BY 5 DESC \
     LIMIT 5",
    "SELECT eventname, total_price FROM (SELECT eventid, total_price, ntile(1000) OVER (ORDER BY \
     total_price DESC) AS percentile FROM (SELECT eventid, sum(pricepaid) total_price FROM sales \
     GROUP BY eventid)) q, event e WHERE q.eventid = e.eventid AND percentile = 1 ORDER BY \
     total_price DESC",
    "SELECT * FROM store_sales ss WHERE ss_sold_time > '12:00:00' QUALIFY row_number() OVER \
     (PARTITION BY ss_sold_date ORDER BY ss_sales_price DESC) <= 2",
    "SELECT listagg(sellerid, ', ') WITHIN GROUP (ORDER BY sellerid) FROM sales WHERE eventid = \
     4337",
    "SELECT APPROXIMATE COUNT(DISTINCT pricepaid) FROM sales",
    "SELECT median(pricepaid), percentile_cont(0.9) WITHIN GROUP (ORDER BY pricepaid) FROM sales",
    "SELECT salesid, sum(pricepaid) OVER (PARTITION BY sellerid ORDER BY saletime ROWS UNBOUNDED \
     PRECEDING) AS running, lag(pricepaid, 1) OVER (ORDER BY saletime) FROM sales",
    "SELECT dateadd(day, 30, caldate), datediff(week, '2008-01-01', caldate), getdate(), sysdate, \
     convert_timezone('UTC', 'America/New_York', saletime), to_char(saletime, 'YYYY-MM') FROM \
     date, sales",
    "SELECT nvl(catdesc, 'none'), nvl2(catgroup, 'y', 'n'), decode(catid, 1, 'one', 'other'), \
     coalesce(catname, ''), nullif(catid, 0) FROM category",
    "SELECT c.c_name, o.o_orderkey, o.o_totalprice FROM customer_orders_lineitem c, c.c_orders o \
     WHERE o.o_orderstatus = 'F'",
    "SELECT json_extract_path_text(doc, 'a', 'b'), json_parse('{\"a\": 1}'), c.doc.items[0].price \
     FROM docs c",
    "SELECT is_valid_json(payload), json_serialize(doc), json_typeof(doc) FROM docs",
    "SELECT ST_AsText(geom), ST_Distance(geom, ST_GeomFromText('POINT(1 1)')) FROM places",
    "SELECT hll_cardinality(hll_create_sketch(userid)) FROM sales",
    "SELECT * FROM (SELECT partname, price FROM part) PIVOT (avg(price) FOR partname IN ('P1', \
     'P2'))",
    "SELECT * FROM count_by_color UNPIVOT (cnt FOR color IN (red, green, blue))",
    "WITH venue_sales AS (SELECT venuename, venuecity, sum(pricepaid) AS venuename_sales FROM \
     sales, venue, event WHERE venue.venueid = event.venueid AND event.eventid = sales.eventid \
     GROUP BY venuename, venuecity), top_venues AS (SELECT venuename FROM venue_sales WHERE \
     venuename_sales > 800000) SELECT venuename, venuecity, sum(qtysold) FROM sales, venue, event \
     WHERE venue.venueid = event.venueid AND venuename IN (SELECT venuename FROM top_venues) GROUP \
     BY 1, 2",
    "WITH RECURSIVE john_org(id, name, manager_id, level) AS (SELECT id, name, manager_id, 1 FROM \
     employee WHERE name = 'John' UNION ALL SELECT e.id, e.name, e.manager_id, level + 1 FROM \
     employee e, john_org j WHERE e.manager_id = j.id AND level < 4) SELECT * FROM john_org",
    "SELECT TOP 10 * FROM sales ORDER BY pricepaid DESC",
    "SELECT * FROM sales ORDER BY saletime DESC NULLS LAST LIMIT 50 OFFSET 100",
    "SELECT * FROM users WHERE username ILIKE 'a%' AND city SIMILAR TO '%(b|d)%' AND email ~ \
     '^[a-z]+@example[.]com$'",
    "SELECT catgroup, catname, count(*) FROM category GROUP BY ROLLUP (catgroup, catname)",
    "SELECT id FROM a UNION SELECT id FROM b EXCEPT SELECT id FROM c INTERSECT SELECT id FROM d \
     MINUS SELECT id FROM e",
    "(SELECT 1) UNION ALL (SELECT 2) ORDER BY 1",
    "SELECT * FROM t1 JOIN t2 USING (id) LEFT OUTER JOIN t3 ON t3.id = t1.id",
    "SELECT * FROM other_db.public.orders o JOIN dev.public.customers c ON c.id = o.customer_id",
    "SELECT count(*) FROM spectrum.sales WHERE eventid = 1",
    "SELECT cast(pricepaid AS decimal(8, 2)), pricepaid::varchar(20), saletime::date, CAST(NULL \
     AS integer) FROM sales",
    "SELECT 1.5, .5, 1e3, 1.5e-3, 1E+3 FROM t",
    "SELECT current_user, current_database(), version(), pg_backend_pid(), current_setting('x')",
    "SELECT has_table_privilege('alice', 'sales', 'select'), has_schema_privilege('alice', \
     'public', 'usage')",
    "SELECT \"table\", size, tbl_rows FROM svv_table_info WHERE \"schema\" = 'public'",
    "SELECT * FROM pg_table_def WHERE tablename = 'sales'",
    "SELECT query, starttime, substring(querytxt, 1, 60) FROM stl_query WHERE userid > 1 ORDER BY \
     starttime DESC LIMIT 10",
    "SELECT * FROM sys_query_history WHERE status = 'running'",
    "SELECT pid, status, query FROM stv_recents WHERE status = 'Running'",
    "SELECT process, user_name, db_name FROM stv_sessions",
    "SELECT * FROM pg_get_late_binding_view_cols() cols(view_schema name, view_name name, \
     col_name name, col_type varchar, col_num int)",
    "SELECT database_name, database_type FROM svv_redshift_databases",
    "SELECT * FROM svv_external_tables WHERE schemaname = 'spectrum'",
    "SELECT \"delete\", \"insert\".\"update\", \"unload\" FROM \"insert\"",
    "SELECT 'UNLOAD (''SELECT 1'') TO ''s3://b/x''' AS s, $$DELETE FROM t$$ AS d, $q$ CALL p() \
     $q$ AS e",
    "SELECT 'SELECT pg_terminate_backend(1)' AS sql_text, $$set_config('a', 'b', false)$$",
    "-- DELETE FROM t\nSELECT 1 /* DROP TABLE t; COPY t FROM 's3://x' */",
    "SELECT created_by, insert_count, copy_of, unload_id, call_id, vacuum_status FROM audit",
    "SELECT lo_owner, lo_size, nextval_at, stats_reset, (doc).items FROM objects",
    "SELECT '(1).pg_terminate_backend' AS s, \"pg_terminate_backend\", t.\"lo_import\" FROM t",
    "SELECT\n  s.salesid,\n  s.pricepaid\nFROM\n  sales AS s\nWHERE\n  s.pricepaid > 0\nORDER \
     BY\n  s.salesid",
    "SELECT 1; SELECT 2;",
    "SHOW TABLE public.sales",
    "SHOW VIEW public.v",
    "SHOW EXTERNAL TABLE spectrum.sales",
    "SHOW DATABASES",
    "SHOW SCHEMAS FROM DATABASE dev",
    "SHOW TABLES FROM SCHEMA dev.public",
    "SHOW COLUMNS FROM TABLE dev.public.sales",
    "SHOW GRANTS ON TABLE public.sales",
    "SHOW PROCEDURE public.p(int)",
    "SHOW MODEL ALL",
    "SHOW DATASHARES",
    "SHOW search_path",
    "SHOW ALL",
    "EXPLAIN SELECT * FROM sales WHERE salesid = 1",
    "EXPLAIN VERBOSE SELECT count(*) FROM sales",
    "EXPLAIN DELETE FROM sales WHERE salesid = 1",
    "EXPLAIN INSERT INTO t SELECT * FROM s",
];

/// Writes beyond the rules' words: a query's write clauses, DML of every kind, COPY and UNLOAD in
/// their forms (an UNLOAD's query is a string), DDL of Redshift's objects (temp tables, late
/// binding views, materialized views, procedures, external schemas, tables and functions, models,
/// row-level security, datashares), users, transactions, settings, cursors, prepared statements,
/// CANCEL, maintenance, the calls that write in every spelling, and text a reading or a comment
/// rule may turn into a second statement.
const WRITES: &[&str] = &[
    "SELECT * INTO archive FROM sales",
    "SELECT * INTO TEMP TABLE scratch FROM sales",
    "SELECT * INTO #scratch FROM sales",
    "SELECT id FROM sales FOR UPDATE",
    "WITH s AS (SELECT * FROM staging) INSERT INTO t SELECT * FROM s",
    "INSERT INTO t (a, b) VALUES (1, 'x'), (2, 'y')",
    "INSERT INTO t DEFAULT VALUES",
    "INSERT INTO t SELECT * FROM s",
    "UPDATE t SET a = 1 WHERE id = 2",
    "UPDATE t SET a = s.a FROM s WHERE t.id = s.id",
    "DELETE FROM t WHERE id = 1",
    "DELETE FROM t USING s WHERE t.id = s.id",
    "MERGE INTO t USING s ON t.id = s.id WHEN MATCHED THEN UPDATE SET a = s.a WHEN NOT MATCHED \
     THEN INSERT VALUES (s.id, s.a)",
    "COPY sales FROM 's3://bucket/tickit/sales_tab.txt' IAM_ROLE \
     'arn:aws:iam::123456789012:role/r' DELIMITER '|' TIMEFORMAT 'MM/DD/YYYY HH:MI:SS' REGION \
     'us-west-2'",
    "COPY t FROM 's3://b/x' CREDENTIALS 'aws_access_key_id=A;aws_secret_access_key=B' FORMAT AS \
     PARQUET",
    "COPY t (a, b) FROM 's3://b/manifest' IAM_ROLE default MANIFEST JSON 'auto'",
    "UNLOAD ('SELECT * FROM venue WHERE venuestate = ''NV''') TO 's3://bucket/unload/' IAM_ROLE \
     default PARALLEL OFF",
    "UNLOAD ($$SELECT * FROM t$$) TO 's3://b/x' IAM_ROLE default FORMAT PARQUET",
    "CREATE TABLE t (id INT IDENTITY(1, 1), name VARCHAR(30) ENCODE zstd) DISTSTYLE KEY DISTKEY \
     (id) SORTKEY (name)",
    "CREATE TEMP TABLE x AS SELECT * FROM t",
    "CREATE TABLE x (LIKE y INCLUDING DEFAULTS)",
    "CREATE TABLE #x (a int)",
    "CREATE OR REPLACE VIEW v AS SELECT * FROM public.t WITH NO SCHEMA BINDING",
    "CREATE MATERIALIZED VIEW mv AUTO REFRESH YES AS SELECT * FROM t",
    "REFRESH MATERIALIZED VIEW mv",
    "CREATE OR REPLACE PROCEDURE p(x int) AS $$ BEGIN DELETE FROM t WHERE id = x; END; $$ \
     LANGUAGE plpgsql",
    "CREATE FUNCTION f(int) RETURNS int STABLE AS $$ SELECT $1 + 1 $$ LANGUAGE sql",
    "CREATE EXTERNAL SCHEMA spectrum FROM DATA CATALOG DATABASE 'db' IAM_ROLE default CREATE \
     EXTERNAL DATABASE IF NOT EXISTS",
    "CREATE EXTERNAL TABLE spectrum.s (a int) STORED AS PARQUET LOCATION 's3://b/x/'",
    "CREATE EXTERNAL FUNCTION f(int) RETURNS int VOLATILE LAMBDA 'fn' IAM_ROLE default",
    "CREATE MODEL m FROM (SELECT * FROM t) TARGET y FUNCTION predict_y IAM_ROLE default SETTINGS \
     (S3_BUCKET 'b')",
    "CREATE USER bob PASSWORD 'md5abc'",
    "ALTER USER bob PASSWORD 'x'",
    "ALTER TABLE t ADD COLUMN c INT DEFAULT 0",
    "ALTER TABLE t ALTER DISTSTYLE EVEN",
    "ALTER TABLE t ALTER SORTKEY AUTO",
    "ALTER TABLE t APPEND FROM s",
    "ATTACH RLS POLICY p ON t TO ROLE r",
    "GRANT USAGE ON DATASHARE d TO NAMESPACE 'x'",
    "TRUNCATE t",
    "DROP TABLE t CASCADE",
    "GRANT SELECT ON t TO alice",
    "REVOKE ALL ON SCHEMA s FROM alice",
    "COMMENT ON TABLE t IS 'x'",
    "VACUUM DELETE ONLY t TO 75 PERCENT",
    "ANALYZE t",
    "ANALYZE COMPRESSION t",
    "CALL p(1)",
    "CALL public.p()",
    "BEGIN",
    "BEGIN READ ONLY",
    "START TRANSACTION",
    "COMMIT",
    "END",
    "ABORT",
    "ROLLBACK",
    "LOCK t",
    "SET search_path TO x, public",
    "SET query_group TO 'q'",
    "RESET ALL",
    "SET SESSION AUTHORIZATION 'bob'",
    "CANCEL 18764",
    "CANCEL 18764 'long'",
    "PREPARE p (int) AS SELECT $1",
    "EXECUTE p(1)",
    "DEALLOCATE p",
    "DECLARE c CURSOR FOR SELECT * FROM t",
    "FETCH NEXT FROM c",
    "CLOSE c",
    "VALUES (1, 'a')",
    "USE dev",
    "SELECT pg_terminate_backend(123)",
    "SELECT pg_cancel_backend(process) FROM stv_sessions",
    "SELECT set_config('search_path', 'x', false)",
    "SELECT \"pg_terminate_backend\"(1)",
    "SELECT pg_catalog.pg_terminate_backend(1)",
    "SELECT pg_catalog.\"set_config\" ('a', 'b', false)",
    "SELECT * FROM (SELECT pg_terminate_backend(1)) s",
    "WITH k AS (SELECT pg_cancel_backend(process) FROM stv_sessions) SELECT * FROM k",
    "SELECT REBOOT_CLUSTER()",
    "SELECT CHANGE_USER_PRIORITY('bob', 'lowest')",
    "SELECT CHANGE_QUERY_PRIORITY(123, 'critical')",
    "SELECT CHANGE_SESSION_PRIORITY(30311, 'lowest')",
    "SELECT change_query_priority(query, 'lowest') FROM stv_recents WHERE status = 'Running'",
    "SELECT pg_catalog.reboot_cluster()",
    "SELECT lo_import('/etc/passwd')",
    "SELECT lo_export(1, '/tmp/x')",
    "SELECT lowrite(0, 'x')",
    "SELECT pg_start_backup('x')",
    "SELECT pg_stat_reset()",
    "SELECT pg_read_file('pg_hba.conf', 0, 200)",
    "SELECT pg_read_binary_file('x')",
    "SELECT * FROM pg_ls_dir('.') d",
    "SELECT pg_stat_file('postgresql.conf')",
    "SELECT pg_reload_conf()",
    "SELECT pg_rotate_logfile()",
    "SELECT nextval('s')",
    "SELECT setval('s', 1, false)",
    // Attribute notation: `(expr).name` is `name(expr)` when no field has that name.
    "SELECT (123).pg_terminate_backend",
    "SELECT (123).pg_cancel_backend",
    "SELECT (process).pg_terminate_backend FROM stv_sessions",
    "SELECT (s.process).pg_cancel_backend FROM stv_sessions s",
    "SELECT ('/etc/passwd').lo_import",
    "SELECT (1).lo_unlink",
    "SELECT (393216).lo_creat",
    "SELECT (1).lo_create",
    "SELECT ('x').pg_start_backup",
    "SELECT (1).PG_TERMINATE_BACKEND",
    "SELECT (SELECT 1).pg_terminate_backend",
    "SELECT (1)\n.pg_terminate_backend",
    "SELECT (1). pg_terminate_backend",
    "SELECT (1)/**/.pg_terminate_backend",
    "SELECT ('pg_hba.conf').pg_read_file",
    "SELECT ('.').pg_ls_dir",
    "SELECT ('x').pg_stat_file",
    "SELECT ('s').nextval",
    "SELECT ('shared').pg_stat_reset_shared",
    "SELECT * FROM stv_sessions WHERE (process).pg_cancel_backend",
    "WITH k AS (SELECT (process).pg_terminate_backend FROM stv_sessions) SELECT * FROM k",
    "SELECT 1; DELETE FROM t",
    "SHOW ALL; SET search_path TO x",
    "EXPLAIN SELECT 1; DROP TABLE t",
    "EXPLAIN ANALYZE DELETE FROM t",
    "SELECT * FROM t WHERE id = 1 /* /* */ DELETE FROM t */",
    "SELECT 1 -- c\rDELETE FROM t",
    // Read with backslash escapes, the string goes on past `\'`; read without, it ends there.
    "SELECT 'a\\'; DELETE FROM t; --'",
    "SELECT 'a\\''; DELETE FROM t",
    "SELECT E'a\\'; DELETE FROM t; --'",
    "SELECT $$ $$; DELETE FROM t",
    "SELECT $q$ $$ $q$; DELETE FROM t",
    "SELECT 1DELETE FROM t",
    "SELECT 1e0pg_terminate_backend(1)",
];

fn suite() -> Suite {
    Suite::new("redshift", &SPEC)
        .reads(READS)
        .writes(WRITES)
        .active("as a cast's type", "SELECT a::", " t")
        .active("after a cast", "SELECT a::INT ", " t")
        .active("after TOP", "SELECT TOP 10 ", " t")
        .active("after APPROXIMATE", "SELECT APPROXIMATE ", " t")
        .active(
            "after QUALIFY",
            "SELECT * FROM t QUALIFY row_number() OVER (ORDER BY a) = 1 ",
            " t",
        )
        .active(
            "in WITHIN GROUP",
            "SELECT listagg(a, ',') WITHIN GROUP (ORDER BY ",
            ") FROM t",
        )
        .active(
            "in a PIVOT",
            "SELECT * FROM t PIVOT (sum(a) FOR b IN (",
            ")) p",
        )
        .active("in a SUPER path", "SELECT c.doc.", " FROM t c")
        .active("in a SUPER index", "SELECT c.doc[", "] FROM t c")
        .active("in a PartiQL FROM", "SELECT * FROM t c, c.", " o")
        .active("after a # temp table", "SELECT * FROM #t ", " t")
        .active(
            "in a window",
            "SELECT row_number() OVER (PARTITION BY ",
            ") FROM t",
        )
        .active("in a CASE", "SELECT CASE WHEN ", " THEN 1 END FROM t")
        .active("after an E and a string", "SELECT E'a' ", " t")
        .active(
            "after a string ending in two backslashes",
            "SELECT 'a\\\\' ",
            " t",
        )
        .active("after a $$ body holding a quote", "SELECT $$ ' $$ ", " t")
        .active("after a string holding $$", "SELECT '$$' ", " t $$")
        .active("after a quoted name holding $$", "SELECT \"$$\" ", " t $$")
        .active("after a string holding --", "SELECT '--' ", " t")
        .active("after a $tag$ body holding $$", "SELECT $a$ $$ $a$ ", " t")
        .active("after SHOW and ;", "SHOW ALL; ", " t")
        .active("after EXPLAIN and ;", "EXPLAIN SELECT 1; ", " t")
        .inert(
            "in a string after a doubled quote",
            "SELECT 'it''s ",
            " t' FROM x",
        )
        .inert(
            "in an UNLOAD-style string",
            "SELECT 'UNLOAD (''",
            "'') TO ''s3://b/x''' FROM x",
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
            "in a procedure-like $$ body",
            "SELECT $$ BEGIN ",
            "; END $$ FROM x",
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
            "varchar(65535)",
            "INT",
            "QUALIFY",
            "TOP",
            "APPROXIMATE",
            "LISTAGG(",
            "WITHIN",
            "GROUP",
            "PIVOT",
            "INTO",
            "FOR",
            "UPDATE",
            "TO",
            "IAM_ROLE",
            "'s3://b/x'",
            "#t",
            "c.doc[0]",
            "pg_terminate_backend(",
            "set_config(",
            "reboot_cluster(",
            "change_user_priority(",
            "json_serialize(",
            "lo_import(",
            "(1).pg_terminate_backend",
            ").nextval",
            "pg_read_file(",
            "$$a'b$$",
            "$q$ $$ $q$",
            "'$$'",
            "\"$$\"",
            "E'x'",
            "'a\\\\'",
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
