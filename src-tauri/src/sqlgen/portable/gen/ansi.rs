//! Generated cases of `ANSI_TEST` (`portable::testing`): the worked example of a dialect file.
//! The shared contexts (`super::active_contexts`, `super::inert_contexts`) come with the suite;
//! a dialect adds the contexts only its grammar has, a corpus of the reads its users run, the
//! write forms its rules' word lists do not spell out, and fuzz atoms of its own.
//!
//! Fixture: `tests/fixtures/classify-generated/ansi_test.json`.

use super::Suite;
use crate::sqlgen::portable::testing::ANSI_TEST;

/// Ordinary reads: joins, CTEs, window functions, subqueries, set operations, BI-style
/// aggregates, the allowed table functions, and the rules' words where they are no keywords (in
/// strings, quoted names, comments, as parts of other names).
const READS: &[&str] = &[
    "SELECT 1",
    "SELECT * FROM orders WHERE id = 42",
    "SELECT o.id, c.name, o.total FROM orders o JOIN customers c ON c.id = o.customer_id \
     WHERE o.status = 'paid' ORDER BY o.created_at DESC LIMIT 50 OFFSET 100",
    "SELECT c.name, count(*) AS n, sum(o.total) AS revenue FROM customers c LEFT JOIN orders o \
     ON o.customer_id = c.id GROUP BY c.name HAVING count(*) > 5 ORDER BY revenue DESC",
    "WITH recent AS (SELECT * FROM orders WHERE created_at > current_date - INTERVAL '30 days'), \
     totals AS (SELECT customer_id, sum(total) AS spent FROM recent GROUP BY customer_id) \
     SELECT c.name, t.spent FROM totals t JOIN customers c ON c.id = t.customer_id",
    "WITH RECURSIVE tree (id, parent_id, depth) AS (SELECT id, parent_id, 0 FROM nodes WHERE \
     parent_id IS NULL UNION ALL SELECT n.id, n.parent_id, t.depth + 1 FROM nodes n JOIN tree t \
     ON n.parent_id = t.id) SELECT * FROM tree",
    "SELECT id, total, row_number() OVER (PARTITION BY customer_id ORDER BY created_at) AS rn, \
     sum(total) OVER (ORDER BY created_at ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) \
     AS running, lag(total, 1) OVER w FROM orders WINDOW w AS (ORDER BY id)",
    "SELECT * FROM products WHERE category_id IN (SELECT id FROM categories WHERE active) AND \
     price BETWEEN 10 AND 100",
    "SELECT name FROM customers c WHERE EXISTS (SELECT 1 FROM orders o WHERE o.customer_id = \
     c.id) AND NOT EXISTS (SELECT 1 FROM refunds r WHERE r.customer_id = c.id)",
    "SELECT id FROM a UNION SELECT id FROM b EXCEPT SELECT id FROM c INTERSECT SELECT id FROM d",
    "(SELECT 1) UNION ALL (SELECT 2) ORDER BY 1",
    "SELECT CASE WHEN total > 100 THEN 'big' WHEN total > 10 THEN 'medium' ELSE 'small' END AS \
     size, coalesce(note, '') AS note, nullif(discount, 0) FROM orders",
    "SELECT date_trunc('month', created_at) AS month, count(*) FILTER (WHERE status = 'paid') \
     AS paid, avg(total) FROM orders GROUP BY 1 ORDER BY 1",
    "SELECT percentile_cont(0.5) WITHIN GROUP (ORDER BY total) FROM orders",
    "SELECT extract(YEAR FROM created_at) AS y, substring(name FROM 1 FOR 3), trim(BOTH ' ' \
     FROM code), position('x' IN name) FROM t",
    "SELECT * FROM t WHERE a IS DISTINCT FROM b AND c IS NOT DISTINCT FROM d",
    "SELECT * FROM generate_series(1, 10) AS g(n)",
    "SELECT u.x FROM t CROSS JOIN LATERAL unnest(t.tags) AS u(x)",
    "SELECT * FROM system_tables() s, generate_series(1, 3) g",
    "SELECT * FROM t1, t2, (SELECT 1 AS x) s WHERE t1.id = t2.id",
    "SELECT * FROM (orders o JOIN customers c ON c.id = o.customer_id)",
    "SELECT count(*) FROM (SELECT DISTINCT customer_id FROM orders) AS d",
    "SELECT a FROM t GROUP BY a, lower(b) ORDER BY a LIMIT 10",
    "SELECT \"delete\", \"insert\".\"update\" FROM \"insert\"",
    "SELECT 'DELETE FROM t; DROP TABLE x' AS s, $$INSERT INTO t$$ AS d, $q$ CALL p() $q$ AS e",
    "-- DELETE FROM t\nSELECT 1 /* DROP TABLE t; */",
    "SELECT id FROM orders FETCH FIRST 10 ROWS ONLY",
    "SELECT a.* FROM accounts a WHERE a.balance < 0 AND a.updated_at >= '2024-01-01'",
    "SELECT created_by, insert_count, copy_of FROM audit",
    "SELECT cast(total AS numeric(10, 2)), total::text, CAST(NULL AS int) FROM orders",
    "SELECT lower(email) LIKE '%@example.com' AS internal FROM users WHERE name ILIKE 'a%'",
    "SELECT 1.5, .5, 1e3, 1.5e-3, 1E+3, 0x1F, X'1F', 1_000 FROM t",
    "SELECT t.begin, t.end FROM t",
    "SELECT\n  o.id,\n  o.total\nFROM\n  orders AS o\nWHERE\n  o.total > 0\nORDER BY\n  o.id",
    "SELECT 1; SELECT 2;",
    "VALUES (1, 'a'), (2, 'b')",
    "TABLE orders",
    "SHOW TABLES",
    "DESCRIBE orders",
    "DESC orders",
    "EXPLAIN SELECT * FROM orders WHERE id = 1",
    "EXPLAIN DELETE FROM orders",
    "EXPLAIN ANALYZE SELECT count(*) FROM orders",
    "EXPLAIN (ANALYZE, BUFFERS) SELECT * FROM orders",
    "EXPLAIN (FORMAT JSON) SELECT * FROM orders",
    "EXPLAIN (\"analyze\") SELECT * FROM orders",
    "SELECT * FROM U&\"orders\" WHERE id = 1",
    "SELECT * FROM orders FOR READ ONLY",
    "USE analytics",
];

/// Writes beyond the rules' words: a query's own write clauses, EXPLAIN ANALYZE of a write,
/// file readers, table functions outside the allow-list (also qualified, quoted, in a FROM
/// list, a join or a parenthesized join), statements the rules do not know, and text a reading
/// or a comment rule may turn into a second statement.
const WRITES: &[&str] = &[
    "SELECT * INTO archive FROM orders",
    "SELECT id FROM orders FOR UPDATE",
    "WITH gone AS (DELETE FROM orders WHERE id = 1 RETURNING *) SELECT * FROM gone",
    "EXPLAIN ANALYZE DELETE FROM orders",
    "EXPLAIN (ANALYZE) INSERT INTO t VALUES (1)",
    "EXPLAIN ANALYZE VERBOSE UPDATE t SET a = 1",
    "EXPLAIN ANALYZE FROBNICATE SELECT 1",
    "SELECT * FROM read_csv('/etc/passwd')",
    "SELECT load_file('/etc/passwd')",
    "SELECT * FROM other_fn(1)",
    "SELECT * FROM t, other_fn(1)",
    "SELECT * FROM t JOIN other_fn(1) f ON true",
    "SELECT * FROM (other_fn(1) CROSS JOIN t)",
    "SELECT * FROM LATERAL other_fn(1)",
    "SELECT * FROM s.other_fn(1)",
    "SELECT * FROM \"other_fn\"(1)",
    "SELECT * FROM (SELECT 1) a, other_fn(1)",
    "SELECT 1 FROM t WHERE a IS DISTINCT FROM b UNION SELECT * FROM other_fn(1)",
    "COPY orders TO '/tmp/x.csv'",
    "CALL refresh_all()",
    "MERGE INTO t USING s ON t.id = s.id WHEN MATCHED THEN DELETE",
    "BEGIN; DELETE FROM t; END",
    "PREPARE p AS SELECT 1",
    "SET search_path = x",
    "VACUUM",
    "DO $$ BEGIN PERFORM 1; END $$",
    "TABLE t; DROP TABLE t",
    "SELECT 1; DELETE FROM t",
    "SELECT * FROM t WHERE id = 1 /* /* */ DELETE FROM t */",
    "SELECT 1 -- c\rDELETE FROM t",
    "SELECT 'a\\'; DELETE FROM t; --'",
    "SELECT $$ $$; DELETE FROM t",
    // A quoted EXPLAIN option may spell ANALYZE, which runs the statement.
    "EXPLAIN (\"analyze\") DELETE FROM t",
    "EXPLAIN (VERBOSE, \"analyze\" true) DELETE FROM t",
    "EXPLAIN (\"analyze\", format json) INSERT INTO t VALUES (1)",
    "EXPLAIN (\"analyze\" 1) UPDATE t SET a = 1",
    "EXPLAIN (FORMAT JSON, \"analyze\") DELETE FROM t",
    // A Unicode-escaped name may spell any function.
    "SELECT * FROM U&\"other_fn\"(1)",
    "SELECT * FROM t, U&\"other_fn\"(1)",
    "SELECT * FROM t1 JOIN U&\"other_fn\"(1) f ON true",
    "SELECT * FROM s.U&\"other_fn\"(1)",
    "SELECT U&\"\\0072ead_csv\"('/etc/passwd')",
    "SELECT * FROM U&\"\\0072ead_csv\"('/etc/passwd')",
    "SELECT U&\"!0072ead_csv\" UESCAPE '!' ('/etc/passwd')",
    // Every row-locking clause.
    "SELECT * FROM t FOR SHARE",
    "SELECT * FROM t FOR KEY SHARE",
    "SELECT * FROM t FOR NO KEY UPDATE",
    // A FROM the scanner keeps in a number.
    "SELECT 1FROM other_fn(1)",
    "SELECT 1e0FROM other_fn(1)",
    // A bare `begin` keeps the `;` in one statement for the splitter.
    "DESCRIBE SELECT begin FROM t; DELETE FROM t",
    "SHOW begin x; DELETE FROM t",
];

fn suite() -> Suite {
    Suite::new("ansi_test", &ANSI_TEST)
        .reads(READS)
        .writes(WRITES)
        .active("after TABLE", "TABLE t ", "")
        .active("after VALUES", "VALUES (1) ", " t")
        .active("in a BEGIN … END block", "BEGIN SELECT 1; ", " t; END")
        .active(
            "after a BEGIN … END block and ;",
            "BEGIN SELECT 1; END; ",
            " t",
        )
        .active(
            "in a window",
            "SELECT row_number() OVER (PARTITION BY ",
            ") FROM t",
        )
        .active("in a CASE", "SELECT CASE WHEN ", " THEN 1 END FROM t")
        .active(
            "in a LATERAL subquery",
            "SELECT * FROM t CROSS JOIN LATERAL (SELECT ",
            ") l",
        )
        .active("after a $tag$ body holding $$", "SELECT $a$ $$ $a$ ", " t")
        .active("after EXPLAIN (ANALYZE)", "EXPLAIN (ANALYZE) ", " t")
        .inert(
            "in a string after a doubled quote",
            "SELECT 'it''s ",
            " t' FROM x",
        )
        .inert(
            "in a $tag$ body after a $other$",
            "SELECT $a$ $b$ ",
            " $b$ $a$ FROM x",
        )
        .vocab(&[
            "RETURNING",
            "FOR",
            "UPDATE",
            "INTERVAL",
            "OVER",
            "PARTITION",
            "generate_series(",
            "unnest(",
            "system_x(",
            "other_fn(",
            "$a$",
            "$a$x;$a$",
            "E'x'",
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
