//! Generated cases of `TSQL_LIKE_TEST` (`portable::testing`, always built), which the renderer's
//! T-SQL cases run against; fixture `tests/fixtures/classify-generated/tsql_like_test.json`.
//! The foundation's, like `ansi.rs`: the sqlserver lane's own spec has its suite in `tsql.rs`.
//!
//! T-SQL needs no terminator between statements and splits batches at `GO`, so a write keyword
//! anywhere makes the text a write: the reads below use none of the rules' words, even as
//! column names.

use super::Suite;
use crate::sqlgen::portable::testing::TSQL_LIKE_TEST;

/// Ordinary reads: joins, CTEs, window functions, paging, table hints, set operations, bracketed
/// and N-strings, system views, and the rules' words where they are no keywords.
const READS: &[&str] = &[
    "SELECT 1",
    "SELECT TOP 10 * FROM dbo.Orders ORDER BY OrderDate DESC",
    "SELECT o.Id, c.Name, o.Total FROM dbo.Orders AS o INNER JOIN dbo.Customers AS c ON c.Id = \
     o.CustomerId LEFT JOIN dbo.Regions r ON r.Id = c.RegionId WHERE o.Status = N'paid'",
    "SELECT * FROM dbo.Orders WITH (NOLOCK) WHERE Id = 42",
    "SELECT CustomerId, COUNT(*) AS n, SUM(Total) AS revenue FROM dbo.Orders GROUP BY CustomerId \
     HAVING COUNT(*) > 5 ORDER BY revenue DESC",
    "WITH recent AS (SELECT * FROM dbo.Orders WHERE OrderDate > DATEADD(day, -30, GETDATE())) \
     SELECT CustomerId, SUM(Total) FROM recent GROUP BY CustomerId",
    "WITH tree (Id, ParentId, Depth) AS (SELECT Id, ParentId, 0 FROM dbo.Nodes WHERE ParentId IS \
     NULL UNION ALL SELECT n.Id, n.ParentId, t.Depth + 1 FROM dbo.Nodes n JOIN tree t ON \
     n.ParentId = t.Id) SELECT * FROM tree",
    "SELECT Id, ROW_NUMBER() OVER (PARTITION BY CustomerId ORDER BY OrderDate) AS rn, \
     LAG(Total, 1) OVER (ORDER BY Id) AS prev FROM dbo.Orders",
    "SELECT * FROM dbo.Orders ORDER BY Id OFFSET 10 ROWS FETCH NEXT 10 ROWS ONLY",
    "SELECT * FROM dbo.Products WHERE CategoryId IN (SELECT Id FROM dbo.Categories WHERE Active = \
     1) AND Price BETWEEN 10 AND 100",
    "SELECT Name FROM dbo.Customers c WHERE EXISTS (SELECT 1 FROM dbo.Orders o WHERE \
     o.CustomerId = c.Id)",
    "SELECT Id FROM a UNION ALL SELECT Id FROM b EXCEPT SELECT Id FROM c",
    "SELECT CASE WHEN Total > 100 THEN N'big' ELSE N'small' END AS size, ISNULL(Note, N'') AS \
     Note, NULLIF(Discount, 0) AS Discount FROM dbo.Orders",
    "SELECT CAST(Total AS DECIMAL(10, 2)), CONVERT(VARCHAR(10), OrderDate, 120) FROM dbo.Orders",
    "SELECT [delete], [insert].[update], \"drop\" FROM [dbo].[select]",
    "SELECT N'DELETE FROM t; DROP TABLE x' AS s, 'EXEC sp_who' AS e",
    "-- DELETE FROM t\nSELECT 1 /* DROP TABLE t; */",
    "SELECT name, object_id FROM sys.tables WHERE is_ms_shipped = 0",
    "SELECT @@VERSION, @@SERVERNAME, DB_NAME()",
    "SELECT 0x1F, 1.5, 1e3, .5 FROM dbo.Numbers",
    "SELECT Id FROM dbo.Orders FOR JSON AUTO",
    "SELECT Id, Name FROM dbo.Customers FOR XML PATH(N'row')",
    "SELECT * FROM OPENJSON(N'[1, 2]')",
    "SELECT 1\nGO\nSELECT 2",
    "SELECT 1;\nSELECT 2;",
    "USE Analytics",
];

/// Writes beyond the rules' words: statements after a `GO` that the rules do not know, procedure
/// calls with no EXEC, calls that reach other servers, text a comment rule may split, and
/// locking hints.
const WRITES: &[&str] = &[
    "SELECT 1\nGO\nsp_configure 'xp_cmdshell', 1",
    "SELECT 1\nGO\nmy_proc 1",
    "SELECT * FROM OPENQUERY(srv, 'DELETE FROM t')",
    "SELECT * FROM OPENROWSET('SQLNCLI', 'Server=x;', 'SELECT 1')",
    "SELECT * INTO #tmp FROM dbo.Orders",
    "SELECT 1 DELETE FROM t",
    "SELECT * FROM t; DROP TABLE t",
    "WAITFOR DELAY '00:00:05'",
    "EXEC('SELECT 1')",
    "EXECUTE sp_executesql N'DELETE FROM t'",
    "DECLARE @x INT",
    "SELECT 1 /* /* */ DELETE FROM t */",
    "SELECT 1 -- c\rDELETE FROM t",
    "SELECT xp_cmdshell('dir')",
    "SELECT * FROM t WITH (UPDLOCK, ROWLOCK)",
    "SELECT * FROM t (HOLDLOCK)",
];

fn suite() -> Suite {
    Suite::new("tsql_like_test", &TSQL_LIKE_TEST)
        .reads(READS)
        .writes(WRITES)
        .active("after a GO batch", "SELECT 1\nGO\n", " t")
        .active("after TOP", "SELECT TOP 1 ", " t")
        .active("after a table hint", "SELECT * FROM t WITH (NOLOCK) ", " u")
        .active("after an N-string", "SELECT N'a' ", " t")
        .active("after a variable", "SELECT @a ", " t")
        .inert("in an N-string", "SELECT N'", " t' FROM x")
        .inert("in a [name] past ]]", "SELECT [a]] ", " t] FROM x")
        .vocab(&[
            "GO",
            "N'x'",
            "[a]",
            "TOP",
            "WITH",
            "(NOLOCK)",
            "@x",
            "#tmp",
            "OPENQUERY(",
            "0x1F",
            "BEGIN",
            "END",
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
