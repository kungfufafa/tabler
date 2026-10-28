//! Generated cases of Transact-SQL (`dialect::tsql`, fixture
//! `tests/fixtures/classify-generated/tsql.json`). `TSQL_LIKE_TEST`, the always-built spec the
//! renderer's T-SQL cases run against, has its suite in `tsql_like.rs`.
//!
//! T-SQL needs no terminator between statements and splits batches at `GO` lines, so a write
//! keyword anywhere makes the text a write, and a read of the corpus uses none of the rules'
//! words outside strings, quoted names and comments, even as a column name (`[close]`, not
//! `close`). The contexts below are the ones only T-SQL has: batches, `N'…'` and `[…]`, variables
//! and temporary tables, table hints, APPLY, PIVOT, `FOR XML` / `FOR JSON`, query hints, ODBC
//! escapes, CLR method calls and money literals.

use super::Suite;
use crate::sqlgen::dialect::tsql::SPEC;

/// Ordinary reads of SQL Server: paging, joins, APPLY, CTEs, window functions, set operations,
/// table and query hints, temporal and graph queries, XML and JSON, CLR types, the catalog views,
/// and the rules' words where they are no keywords (strings, `[names]`, comments, parts of other
/// names).
const READS: &[&str] = &[
    "SELECT 1",
    "SELECT TOP (10) * FROM dbo.Orders ORDER BY OrderDate DESC",
    "SELECT TOP 10 PERCENT WITH TIES Id, Total FROM dbo.Orders ORDER BY Total DESC",
    "SELECT o.Id, c.Name, o.Total FROM dbo.Orders AS o INNER JOIN dbo.Customers AS c ON c.Id = \
     o.CustomerId LEFT OUTER JOIN dbo.Regions r ON r.Id = c.RegionId WHERE o.Status = N'paid'",
    "SELECT * FROM dbo.Orders WITH (NOLOCK) WHERE Id = 42",
    "SELECT o.* FROM dbo.Orders o WITH (READPAST, INDEX(IX_Orders_Date)) WHERE o.OrderDate >= \
     '20240101'",
    "SELECT CustomerId, COUNT(*) AS n, SUM(Total) AS revenue FROM dbo.Orders GROUP BY CustomerId \
     HAVING COUNT(*) > 5 ORDER BY revenue DESC",
    "SELECT Region, Product, SUM(Amount) FROM dbo.Sales GROUP BY ROLLUP (Region, Product)",
    "SELECT Region, SUM(Amount) FROM dbo.Sales GROUP BY GROUPING SETS ((Region), ())",
    "WITH recent AS (SELECT * FROM dbo.Orders WHERE OrderDate > DATEADD(day, -30, GETDATE())) \
     SELECT CustomerId, SUM(Total) FROM recent GROUP BY CustomerId",
    "WITH tree (Id, ParentId, Depth) AS (SELECT Id, ParentId, 0 FROM dbo.Nodes WHERE ParentId IS \
     NULL UNION ALL SELECT n.Id, n.ParentId, t.Depth + 1 FROM dbo.Nodes n JOIN tree t ON \
     n.ParentId = t.Id) SELECT * FROM tree OPTION (MAXRECURSION 100)",
    ";WITH c AS (SELECT 1 AS a) SELECT a FROM c",
    "WITH XMLNAMESPACES (N'urn:x' AS x) SELECT Doc.query(N'/x:a') FROM dbo.Docs",
    "SELECT Id, ROW_NUMBER() OVER (PARTITION BY CustomerId ORDER BY OrderDate) AS rn, \
     LAG(Total, 1) OVER (ORDER BY Id) AS prev, LEAD(Total) OVER (ORDER BY Id) AS next_total, \
     SUM(Total) OVER (ORDER BY Id ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) AS running, \
     NTILE(4) OVER (ORDER BY Total) AS quartile FROM dbo.Orders",
    "SELECT DISTINCT PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY Total) OVER (PARTITION BY \
     CustomerId) FROM dbo.Orders",
    "SELECT STRING_AGG(Name, N', ') WITHIN GROUP (ORDER BY Name) FROM dbo.Customers",
    "SELECT * FROM dbo.Orders ORDER BY Id OFFSET 10 ROWS FETCH NEXT 10 ROWS ONLY",
    "SELECT * FROM dbo.Orders ORDER BY Id OFFSET 0 ROWS",
    "SELECT * FROM dbo.Products WHERE CategoryId IN (SELECT Id FROM dbo.Categories WHERE Active = \
     1) AND Price BETWEEN 10 AND 100",
    "SELECT Name FROM dbo.Customers c WHERE EXISTS (SELECT 1 FROM dbo.Orders o WHERE \
     o.CustomerId = c.Id) AND NOT EXISTS (SELECT 1 FROM dbo.Refunds r WHERE r.CustomerId = c.Id)",
    "SELECT * FROM dbo.Orders WHERE Total > ALL (SELECT Total FROM dbo.Refunds)",
    "SELECT Id FROM a UNION ALL SELECT Id FROM b EXCEPT SELECT Id FROM c INTERSECT SELECT Id FROM \
     d",
    "(SELECT 1 AS a) UNION (SELECT 2) ORDER BY a",
    "SELECT CASE WHEN Total > 100 THEN N'big' ELSE N'small' END AS size, ISNULL(Note, N'') AS \
     Note, NULLIF(Discount, 0) AS Discount, IIF(Paid = 1, N'yes', N'no') AS paid, CHOOSE(2, \
     N'a', N'b') AS pick, COALESCE(Note, Memo, N'') AS memo FROM dbo.Orders",
    "SELECT CAST(Total AS DECIMAL(10, 2)), CONVERT(VARCHAR(10), OrderDate, 120), TRY_CAST(N'1' \
     AS INT), TRY_CONVERT(DATE, N'x'), PARSE(N'1/2/2024' AS DATE USING 'en-US'), FORMAT(Total, \
     'N2') FROM dbo.Orders",
    "SELECT DATEADD(day, -7, SYSDATETIME()), DATEDIFF_BIG(ms, '2000-01-01', GETUTCDATE()), \
     EOMONTH(GETDATE()), DATEFROMPARTS(2024, 1, 2), DATETRUNC(month, GETDATE()), \
     SYSDATETIMEOFFSET() AT TIME ZONE 'UTC'",
    "SELECT GREATEST(1, 2), LEAST(3, 4), ABS(-1), ROUND(1.25, 1), CEILING(1.2), FLOOR(1.8)",
    "SELECT [delete], [insert].[update], \"drop\", [exec] FROM [dbo].[select] AS [into]",
    "SELECT [close], [open], [set], [get], [add], [return], [kill] FROM dbo.Prices",
    "SELECT N'DELETE FROM t; DROP TABLE x' AS s, 'EXEC sp_who' AS e, N'GO' AS g, 'it''s' AS q",
    "-- DELETE FROM t\nSELECT 1 /* DROP TABLE t; EXEC xp_cmdshell 'dir' */",
    "SELECT name, object_id, create_date FROM sys.tables WHERE is_ms_shipped = 0 ORDER BY name",
    "SELECT s.name, o.name, c.name FROM sys.columns c JOIN sys.objects o ON o.object_id = \
     c.object_id JOIN sys.schemas s ON s.schema_id = o.schema_id WHERE o.type IN ('U', 'V')",
    "SELECT * FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_SCHEMA = N'dbo'",
    "SELECT r.session_id, t.text FROM sys.dm_exec_requests r CROSS APPLY \
     sys.dm_exec_sql_text(r.sql_handle) t",
    "SELECT OBJECT_DEFINITION(OBJECT_ID(N'dbo.v')), OBJECT_SCHEMA_NAME(object_id), \
     COL_NAME(object_id, 1) FROM sys.views",
    "SELECT @@VERSION, @@SERVERNAME, @@SPID, @@TRANCOUNT, @@ROWCOUNT, DB_NAME(), SUSER_SNAME(), \
     SERVERPROPERTY('ProductVersion'), DATABASEPROPERTYEX(DB_NAME(), 'Collation')",
    "SELECT 0x1F, 0x, 1.5, 1e3, .5, 1., $12.50, -1, 1E-5 FROM dbo.Numbers",
    "SELECT Id FROM dbo.Orders FOR JSON AUTO",
    "SELECT Id, Name FROM dbo.Customers FOR JSON PATH, ROOT('customers'), INCLUDE_NULL_VALUES",
    "SELECT Id, Name FROM dbo.Customers FOR XML PATH(N'row'), ROOT('rows'), TYPE",
    "SELECT Id FROM dbo.Customers FOR XML RAW, ELEMENTS",
    "SELECT * FROM OPENJSON(N'[{\"id\": 1}]') WITH (id INT '$.id', name NVARCHAR(50) '$.name')",
    "SELECT JSON_VALUE(Doc, '$.a'), JSON_QUERY(Doc, '$.b'), ISJSON(Doc), JSON_MODIFY(Doc, \
     '$.c', 1) FROM dbo.Docs",
    "SELECT value FROM STRING_SPLIT(N'a,b,c', N',')",
    "SELECT * FROM GENERATE_SERIES(1, 10)",
    "SELECT c.Name, o.Total FROM dbo.Customers c CROSS APPLY (SELECT TOP (1) Total FROM \
     dbo.Orders WHERE CustomerId = c.Id ORDER BY OrderDate DESC) o",
    "SELECT c.Name, o.n FROM dbo.Customers c OUTER APPLY (SELECT COUNT(*) AS n FROM dbo.Orders \
     WHERE CustomerId = c.Id) o",
    "SELECT * FROM (SELECT Year, Quarter, Amount FROM dbo.Sales) s PIVOT (SUM(Amount) FOR \
     Quarter IN ([Q1], [Q2], [Q3], [Q4])) p",
    "SELECT Id, Attribute, Value FROM dbo.Wide UNPIVOT (Value FOR Attribute IN (A, B, C)) u",
    "SELECT * FROM dbo.Prices FOR SYSTEM_TIME AS OF '2024-01-01T00:00:00' WHERE Id = 1",
    "SELECT * FROM dbo.Prices FOR SYSTEM_TIME ALL",
    "SELECT x.value('(/a/@id)[1]', 'int'), x.query('/a'), x.exist('/a') FROM dbo.Docs CROSS \
     APPLY Doc.nodes('/root/a') AS t(x)",
    "SELECT geography::Point(47.6, -122.3, 4326).STDistance(Location), Location.STAsText() FROM \
     dbo.Places",
    "SELECT Node.ToString(), Node.GetLevel() FROM dbo.Org WHERE \
     Node.IsDescendantOf(hierarchyid::GetRoot()) = 1",
    "SELECT * FROM #tmp t JOIN ##shared s ON s.Id = t.Id",
    "SELECT @x AS x, @@ROWCOUNT AS n",
    "SELECT * FROM sys.fn_my_permissions(NULL, 'SERVER')",
    "SELECT COUNT_BIG(*), APPROX_COUNT_DISTINCT(CustomerId) FROM dbo.Orders",
    "SELECT {fn CURDATE()}, {d '2024-01-01'}, {ts '2024-01-01 10:00:00'}",
    "SELECT Id FROM dbo.Orders WHERE Name LIKE N'[a-c]%' ESCAPE N'!' COLLATE \
     Latin1_General_100_CI_AS",
    "SELECT * FROM dbo.Orders TABLESAMPLE (10 PERCENT) REPEATABLE (42)",
    "SELECT CONVERT(NVARCHAR(MAX), Data, 1), HASHBYTES('SHA2_256', Data), CHECKSUM(*) FROM \
     dbo.Blobs",
    "SELECT $IDENTITY, $ROWGUID FROM dbo.Orders",
    "SELECT $PARTITION.pfDate(OrderDate) AS p, COUNT(*) FROM dbo.Orders GROUP BY \
     $PARTITION.pfDate(OrderDate)",
    "SELECT p.Name FROM dbo.Person p, dbo.Likes l, dbo.Restaurant r WHERE MATCH(p-(l)->r)",
    "SELECT * FROM dbo.Docs WHERE CONTAINS(Body, N'\"blue\" NEAR \"car\"') OR FREETEXT(Body, \
     N'car')",
    "SELECT * FROM OPENXML(@h, N'/root/a', 1) WITH (id INT)",
    "SELECT SESSION_CONTEXT(N'user_id'), CONTEXT_INFO(), ORIGINAL_LOGIN(), APP_NAME()",
    "SELECT * FROM [linked].[db].[dbo].[t]",
    "SELECT * FROM dbo.Orders OPTION (RECOMPILE, MAXDOP 1, USE HINT ('DISABLE_PARAMETER_SNIFFING'))",
    "SELECT * FROM dbo.Orders o WITH (FORCESEEK) WHERE o.Id = @id OPTION (OPTIMIZE FOR (@id = 1))",
    "SELECT Größe, 名前 FROM dbo.Artikel",
    "SELECT CreatedBy, InsertCount, IsDeleted, UpdatedAt, Executed, Settings FROM dbo.Audit",
    "SELECT\n  o.Id,\n  o.Total\nFROM\n  dbo.Orders AS o\nWHERE\n  o.Total > 0\nORDER BY\n  o.Id",
    "SELECT 1\nGO\nSELECT 2",
    "SELECT 1\n  go  -- next batch\nSELECT 2\ngo",
    "SELECT 1;\nSELECT 2;",
    "SELECT 1 SELECT 2",
    "USE Analytics",
    "USE [Sales DB]",
    "USE master\nGO\nSELECT name FROM sys.databases",
];

/// Writes beyond the rules' words: procedure calls with no EXEC at the start of a batch, the
/// calls that reach other servers and files, the statements a terminator need not end, what a
/// comment rule may split, and SELECT … INTO.
const WRITES: &[&str] = &[
    "SELECT * INTO #tmp FROM dbo.Orders",
    "SELECT Id INTO dbo.Archive FROM dbo.Orders WHERE 1 = 0",
    "SELECT 1\nGO\nsp_configure 'xp_cmdshell', 1",
    "SELECT 1\nGO\nmy_proc 1",
    "GO\nmy_proc 1",
    "GO\ndbo.my_proc @a = 1",
    "GO\n[dbo].[my_proc] 1",
    "GO\nsp_who",
    "GO\nxp_cmdshell 'dir'",
    "GO\n{call dbo.my_proc(1)}",
    "EXEC('DELETE FROM t')",
    "EXEC (N'SELECT 1') AT linked",
    "EXECUTE sp_executesql N'DELETE FROM t'",
    "EXEC @proc",
    "EXECUTE AS USER = 'dbo'",
    "SELECT * FROM OPENQUERY(srv, 'DELETE FROM t')",
    "SELECT BulkColumn FROM OPENROWSET(BULK 'C:\\x.txt', SINGLE_CLOB) AS x",
    "SELECT a.* FROM OPENROWSET('SQLNCLI', 'Server=x;Trusted_Connection=yes;', 'EXEC sp_who') AS a",
    "SELECT * FROM OPENDATASOURCE('SQLNCLI', 'Data Source=x').db.dbo.t",
    "SELECT xp_cmdshell('dir')",
    "SELECT [xp_cmdshell](1)",
    "SELECT dbo.sp_x (1)",
    "SELECT 1 DELETE FROM t",
    "SELECT 1 UPDATE t SET a = 1",
    "SELECT 1 INSERT t VALUES (1)",
    "SELECT 1 TRUNCATE TABLE t",
    "SELECT * FROM t WAITFOR DELAY '00:00:01'",
    "SELECT * FROM t; DROP TABLE t",
    "SELECT * FROM t OPTION (RECOMPILE) DELETE FROM t",
    // Locking hints, also in the old form without WITH.
    "SELECT * FROM dbo.Orders WITH (UPDLOCK, ROWLOCK) WHERE Id = 1",
    "SELECT * FROM dbo.Orders o WITH (NOLOCK) JOIN dbo.Stock s WITH (XLOCK) ON s.Id = o.Id",
    "SELECT * FROM dbo.Orders (HOLDLOCK)",
    "SELECT * FROM sys.dm_exec_sessions\nKILL 52",
    "WITH c AS (SELECT * FROM t) DELETE FROM c",
    ";WITH c AS (SELECT 1 AS a) UPDATE c SET a = 2",
    "WITH c AS (SELECT 1 AS a) INSERT INTO t SELECT * FROM c",
    "WITH c AS (SELECT 1 AS a) MERGE t USING c ON 1 = 0 WHEN NOT MATCHED THEN INSERT VALUES (1);",
    "USE master DROP DATABASE x",
    "USE [master]; EXEC sp_who",
    "DECLARE @x INT = 1",
    "SET NOCOUNT ON",
    "SET IDENTITY_INSERT dbo.t ON",
    "SET SHOWPLAN_XML ON",
    "BEGIN TRAN",
    "BEGIN TRANSACTION; DELETE FROM t; COMMIT",
    "COMMIT",
    "ROLLBACK",
    "SAVE TRAN s",
    "BEGIN TRY DELETE FROM t END TRY BEGIN CATCH END CATCH",
    "IF 1 = 1 SELECT 1",
    "WHILE 1 = 1 BREAK",
    "PRINT 'x'",
    "RAISERROR('x', 16, 1)",
    "THROW 50000, 'x', 1",
    "DBCC CHECKDB",
    "KILL 52",
    "SHUTDOWN WITH NOWAIT",
    "CHECKPOINT",
    "RECONFIGURE",
    "BACKUP DATABASE x TO DISK = 'x.bak'",
    "RESTORE DATABASE x FROM DISK = 'x.bak'",
    "BULK INSERT t FROM 'f.csv'",
    "INSERT BULK t (a INT)",
    "GRANT SELECT ON t TO u",
    "DENY SELECT ON t TO u",
    "REVOKE SELECT ON t FROM u",
    "ALTER DATABASE x SET SINGLE_USER",
    "CREATE PROCEDURE p AS SELECT 1",
    "CREATE OR ALTER VIEW v AS SELECT 1 AS a",
    "DROP TABLE IF EXISTS t",
    "TRUNCATE TABLE t",
    "MERGE t USING s ON t.id = s.id WHEN MATCHED THEN DELETE;",
    "UPDATE STATISTICS t",
    "ENABLE TRIGGER tr ON t",
    "DISABLE TRIGGER ALL ON DATABASE",
    "OPEN SYMMETRIC KEY k DECRYPTION BY PASSWORD = 'x'",
    "CLOSE ALL SYMMETRIC KEYS",
    "DECLARE c CURSOR FOR SELECT 1",
    "OPEN c",
    "GO\nFETCH NEXT FROM c",
    "DEALLOCATE c",
    "REVERT",
    "SETUSER",
    "WAITFOR (RECEIVE TOP (1) * FROM q)",
    "RECEIVE * FROM q",
    "SEND ON CONVERSATION @h (N'x')",
    "END CONVERSATION @h",
    "BEGIN DIALOG @h FROM SERVICE s TO SERVICE 't'",
    "GET CONVERSATION GROUP @g FROM q",
    "MOVE CONVERSATION @h TO @g",
    "READTEXT t.c @p 0 0",
    "WRITETEXT t.c @p 'x'",
    "UPDATETEXT t.c @p 0 NULL 'x'",
    "RENAME OBJECT t TO u",
    "ADD SENSITIVITY CLASSIFICATION TO dbo.t.c WITH (LABEL = 'x')",
    "COPY INTO t FROM 'https://x/y.csv'",
    "CREATE TABLE #t (a INT)",
    "GOTO done",
    "GO\ndone: SELECT 1",
    "RETURN",
    // Glued to numbers, money and binary literals, which end before the keyword.
    "SELECT 1INTO t2 FROM t",
    "SELECT 0x1FINTO t2 FROM t",
    "SELECT $1.5DELETE FROM t",
    "SELECT 1.e5DELETE FROM t",
    "SELECT £1DELETE FROM t",
    // Comments a server may end elsewhere.
    "SELECT 1 /* /* */ DELETE FROM t */",
    "SELECT 1 -- c\rDELETE FROM t",
    "SELECT 1 -- c\u{2028}DELETE FROM t",
    "/* /* */ DELETE FROM t --*/ SELECT 1",
    // A `GO` the splitter takes ends the batch; one it does not take leaves the write in it.
    "SELECT 1\r\nGO\r\nDELETE FROM t",
    "SELECT 1\rGO\rDELETE FROM t",
    "SELECT 1 GO DELETE FROM t",
    "SELECT 1\nGO 3\nUPDATE t SET a = 1",
];

fn suite() -> Suite {
    Suite::new("tsql", &SPEC)
        .reads(READS)
        .writes(WRITES)
        // Batches: after a GO line, with a count, a comment, CRLF, blanks, lower case.
        .active("after a GO batch", "SELECT 1\nGO\n", " t")
        .active("after a GO batch with a count", "SELECT 1\nGO 2\n", " t")
        .active(
            "after a GO batch with a comment",
            "SELECT 1\n  go -- next\n",
            " t",
        )
        .active("after a GO batch and CRLF", "SELECT 1\r\nGO\r\n", " t")
        .active("before a GO batch", "SELECT 1 ", " t\nGO\nSELECT 2")
        .active("after a GO that is no separator", "SELECT 1 GO ", " t")
        .active("after a GO in a comment line", "SELECT 1\n-- GO\n", " t")
        .active(
            "after ;WITH",
            ";WITH c AS (SELECT 1 AS a) SELECT a FROM c ",
            " t",
        )
        // Literals and names of T-SQL.
        .active("after an N-string", "SELECT N'a' ", " t")
        .active("glued to an N-string", "SELECT N'a'", " t")
        .active("after a [name] past ]]", "SELECT [a]]b] ", " t")
        .active("after a variable", "SELECT @a ", " t")
        .active("after a system variable", "SELECT @@ROWCOUNT ", " t")
        .active("after a temporary table", "SELECT * FROM #tmp ", " t")
        .active(
            "after a global temporary table",
            "SELECT * FROM ##tmp ",
            " t",
        )
        .active("glued to money", "SELECT $1.5", " t")
        .active("glued to pounds", "SELECT £1", " t")
        .active("after $IDENTITY", "SELECT $IDENTITY ", " t")
        .active("after an ODBC escape", "SELECT {fn CURDATE()} ", " t")
        .active("in an ODBC escape", "SELECT {fn ", "(1)} FROM t")
        // Clauses only T-SQL has.
        .active("after TOP", "SELECT TOP 1 ", " t")
        .active(
            "after TOP (n) PERCENT WITH TIES",
            "SELECT TOP (1) PERCENT WITH TIES ",
            " t",
        )
        .active("after a table hint", "SELECT * FROM t WITH (NOLOCK) ", " u")
        .active("in a table hint", "SELECT * FROM t WITH (", ") u")
        .active(
            "after a query hint",
            "SELECT * FROM t OPTION (RECOMPILE) ",
            " u",
        )
        .active("in a query hint", "SELECT * FROM t OPTION (", ")")
        .active(
            "after USE HINT",
            "SELECT * FROM t OPTION (USE HINT ('X')) ",
            " u",
        )
        .active("after FOR XML", "SELECT a FROM t FOR XML PATH ", " u")
        .active("after FOR JSON", "SELECT a FROM t FOR JSON AUTO ", " u")
        .active(
            "after FOR SYSTEM_TIME",
            "SELECT * FROM t FOR SYSTEM_TIME ALL ",
            " u",
        )
        .active(
            "in a CROSS APPLY",
            "SELECT * FROM t CROSS APPLY (SELECT ",
            " u) a",
        )
        .active(
            "after an OUTER APPLY",
            "SELECT * FROM t OUTER APPLY f(t.a) a ",
            " u",
        )
        .active(
            "in a PIVOT",
            "SELECT * FROM t PIVOT (SUM(a) FOR b IN (",
            ")) p",
        )
        .active(
            "after a PIVOT",
            "SELECT * FROM t PIVOT (SUM(a) FOR b IN ([x])) p ",
            " u",
        )
        .active(
            "in an OPENJSON WITH",
            "SELECT * FROM OPENJSON(@j) WITH (a INT ",
            " u)",
        )
        .active(
            "after WITH XMLNAMESPACES",
            "WITH XMLNAMESPACES (N'u' AS x) SELECT 1 ",
            " t",
        )
        .active(
            "after a CLR method call",
            "SELECT x.value('a', 'int') ",
            " t",
        )
        .active(
            "after a static method call",
            "SELECT geography::Point(1, 2, 4326) ",
            " t",
        )
        .active("in a CASE", "SELECT CASE WHEN ", " THEN 1 END FROM t")
        .active(
            "in a window",
            "SELECT ROW_NUMBER() OVER (PARTITION BY ",
            ") FROM t",
        )
        .active(
            "after OFFSET … FETCH",
            "SELECT * FROM t ORDER BY a OFFSET 1 ROWS FETCH NEXT 1 ROWS ONLY ",
            " u",
        )
        .active(
            "after COLLATE",
            "SELECT a COLLATE Latin1_General_BIN ",
            " t",
        )
        .active(
            "after AT TIME ZONE",
            "SELECT SYSDATETIME() AT TIME ZONE 'UTC' ",
            " t",
        )
        // Literals and names in which nothing counts.
        .inert("in an N-string", "SELECT N'", " t' FROM x")
        .inert("in an N-string past ''", "SELECT N'a'' ", " t' FROM x")
        .inert("in a [name] past ]]", "SELECT [a]] ", " t] FROM x")
        .inert("in a [name] holding [", "SELECT [a[b ", " t] FROM x")
        .inert(
            "in a \"name\" holding ] and [",
            "SELECT \"a] [",
            " t\" FROM x",
        )
        .inert(
            "in a string holding a GO line",
            "SELECT 'a\nGO\n",
            " t' FROM x",
        )
        .inert(
            "in a [name] holding a GO line",
            "SELECT [a\nGO\n",
            " t] FROM x",
        )
        .inert(
            "in a block comment holding a GO line",
            "SELECT 1 /*\nGO\n",
            " t */ FROM x",
        )
        .inert("in a string holding --", "SELECT 'a -- ", " t' FROM x")
        .inert("in a string holding /*", "SELECT 'a /* ", " t' FROM x")
        .inert(
            "in a line comment holding a quote",
            "SELECT 1 -- it's ",
            " t\nFROM x",
        )
        .inert(
            "in a string continued past a backslash",
            "SELECT 'a\\\n",
            " t' FROM x",
        )
        .inert("in an XML string", "SELECT CAST(N'<a>", "</a>' AS XML)")
        .inert("in a JSON path", "SELECT JSON_VALUE(@j, '$.", "')")
        .vocab(&[
            "GO",
            "\nGO\n",
            "\r\nGO 2\r\n",
            "\nGO -- c\n",
            "N'x'",
            "N'a''b'",
            "[a]",
            "[a]]b]",
            "[a\nGO\nb]",
            "'a\nGO\nb'",
            "/*\nGO\n*/",
            "TOP",
            "(1)",
            "PERCENT",
            "TIES",
            "WITH",
            "(NOLOCK)",
            "OPTION",
            "(RECOMPILE)",
            "HINT",
            "@x",
            "@@ROWCOUNT",
            "#tmp",
            "##g",
            "$1",
            "$1.5",
            "£1",
            "$IDENTITY",
            "0x1F",
            "0x",
            "OPENQUERY(",
            "OPENJSON(",
            "BEGIN",
            "END",
            "TRY",
            "CATCH",
            "OUTPUT",
            "INSERTED",
            "APPLY",
            "CROSS",
            "PIVOT",
            "FOR",
            "XML",
            "JSON",
            "SYSTEM_TIME",
            "OFFSET",
            "FETCH",
            "NEXT",
            "ROWS",
            "ONLY",
            "::",
            "{fn",
            "}",
            "COLLATE",
            "IIF(",
            ".value(",
            "USE",
            ";WITH",
            "\u{3000}",
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

/// A procedure runs without EXEC only as the first statement of a batch (so do a cursor FETCH
/// and a label): later in a batch its name is the alias of the column before it (`SELECT 1
/// sp_who`) or a syntax error. So the corpus puts these forms after a GO line, and here they
/// start the text.
#[test]
fn procedure_calls_start_a_batch() {
    use crate::sqlgen::classify::StatementClass::{Read, Write};
    for sql in [
        "my_proc 1",
        "dbo.my_proc @a = 1",
        "[dbo].[my_proc] 1",
        "\"my_proc\" 1",
        "sp_who",
        "xp_cmdshell 'dir'",
        "{call dbo.my_proc(1)}",
        "FETCH NEXT FROM c",
        "done: SELECT 1",
        "-- c\nmy_proc",
        "/* c */ my_proc",
    ] {
        assert_eq!((SPEC.classify)(sql), Write, "{sql:?}");
    }
    assert_eq!((SPEC.classify)("SELECT 1 sp_who"), Read);
}

/// Red-team finding in the shared scanner: `ſelect` (long s), `wıth` (dotless i) and `uſe`
/// upper-case to SELECT, WITH and USE by Unicode rules, while SQL Server takes such a word as the
/// name of a procedure and runs it (at the start of a batch). Read keywords match in ASCII only.
#[test]
fn read_keywords_match_in_ascii_only() {
    use crate::sqlgen::classify::StatementClass::Write;
    for sql in [
        "ſelect 1",
        "ſELECT * FROM t",
        "wıth 1",
        "WıTH x AS (SELECT 1) SELECT 1",
        "uſe x",
        "Uſe shop",
    ] {
        assert_eq!((SPEC.classify)(sql), Write, "{sql:?}");
    }
}
