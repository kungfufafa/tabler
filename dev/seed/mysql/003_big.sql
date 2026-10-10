-- Tabler dev seed / MySQL / 003: events_big (100 000 rows)
--
-- Generated with a recursive CTE; every value is derived from the row number with
-- modulo arithmetic (no RAND()), so the table is identical on every reseed.
-- kind distribution: view 53 333 / click 26 667 / buy 15 000 / refund 5 000.
--
-- A 100 000-level recursion needs a higher limit than the default 1 000. The two
-- statements below are version-gated comments so the same file loads on both servers:
--   MySQL 8.0+  : SET SESSION cte_max_recursion_depth = 1000000   (MariaDB ignores /*!80000 */)
--   MariaDB 10.2+: SET SESSION max_recursive_iterations = 1000000 (MySQL ignores /*M! */)
/*!80000 SET SESSION cte_max_recursion_depth = 1000000 */;
/*M!100200 SET SESSION max_recursive_iterations = 1000000 */;

SET NAMES utf8mb4;
SET time_zone = '+00:00';

DROP TABLE IF EXISTS events_big;

CREATE TABLE events_big (
  id       bigint NOT NULL AUTO_INCREMENT PRIMARY KEY,
  ts       datetime NOT NULL,
  kind     varchar(10) NOT NULL,
  value    decimal(12,2) NOT NULL,
  payload  json NULL,
  KEY events_big_kind_ts_idx (kind, ts),
  CONSTRAINT events_big_kind_chk CHECK (kind IN ('view', 'click', 'buy', 'refund'))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

INSERT INTO events_big (ts, kind, value, payload)
WITH RECURSIVE seq (i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM seq WHERE i < 100000)
SELECT
  TIMESTAMP('2024-01-01 00:00:00') + INTERVAL (i * 37) SECOND,
  CASE
    WHEN i % 20 = 0 THEN 'refund'
    WHEN i % 5  = 0 THEN 'buy'
    WHEN i % 3  = 0 THEN 'click'
    ELSE 'view'
  END,
  ((i * 7919) % 100000) / 100,
  JSON_OBJECT('i', i, 'user', 1 + i % 50, 'bucket', i % 16)
FROM seq;
