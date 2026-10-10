-- Tabler dev seed / PostgreSQL / 003: events_big (100 000 rows)
--
-- Generated with generate_series; every value is derived from the row number with
-- modulo arithmetic (no random()), so the table is identical on every reseed.
-- kind distribution: view 53 333 / click 26 667 / buy 15 000 / refund 5 000.

SET client_encoding = 'UTF8';
SET client_min_messages = warning;

DROP TABLE IF EXISTS events_big;

CREATE TABLE events_big (
  id       bigserial PRIMARY KEY,
  ts       timestamp NOT NULL,
  kind     text NOT NULL CHECK (kind IN ('view', 'click', 'buy', 'refund')),
  value    numeric(12,2) NOT NULL,
  payload  jsonb
);

INSERT INTO events_big (ts, kind, value, payload)
SELECT
  timestamp '2024-01-01 00:00:00' + i * interval '37 seconds',
  CASE
    WHEN i % 20 = 0 THEN 'refund'
    WHEN i % 5  = 0 THEN 'buy'
    WHEN i % 3  = 0 THEN 'click'
    ELSE 'view'
  END,
  ((i * 7919) % 100000) / 100.0,
  jsonb_build_object('i', i, 'user', 1 + i % 50, 'bucket', i % 16)
FROM generate_series(1, 100000) AS i;

-- Index after the bulk load (faster than maintaining it during the insert).
CREATE INDEX events_big_kind_ts_idx ON events_big (kind, ts);

ANALYZE events_big;
