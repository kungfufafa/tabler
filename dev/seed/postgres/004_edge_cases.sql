-- Tabler dev seed / PostgreSQL / 004: edge cases and odd objects
--
--   types_zoo      one column per scalar type; row 1 all NULL, row 2 "min", row 3 "max"/typical
--   "Order Items"  table name with a space and capitals, composite PK
--   "CamelCase"    mixed-case table with reserved words as column names
--   users          extra rows with awkward names, extreme timestamps and special doubles

SET client_encoding = 'UTF8';
SET client_min_messages = warning;

DROP TABLE IF EXISTS types_zoo;
DROP TABLE IF EXISTS "Order Items";
DROP TABLE IF EXISTS "CamelCase";

-- ---------------------------------------------------------------------------
-- types_zoo
-- ---------------------------------------------------------------------------
CREATE TABLE types_zoo (
  id             integer PRIMARY KEY,
  c_smallint     smallint,
  c_integer      integer,
  c_bigint       bigint,
  c_numeric      numeric,
  c_real         real,
  c_double       double precision,
  c_money        money,
  c_boolean      boolean,
  c_char         char(5),
  c_varchar      varchar(50),
  c_text         text,
  c_bytea        bytea,
  c_date         date,
  c_time         time,
  c_timetz       timetz,
  c_timestamp    timestamp,
  c_timestamptz  timestamptz,
  c_interval     interval,
  c_uuid         uuid,
  c_json         json,
  c_jsonb        jsonb,
  c_int_array    int[],
  c_text_array   text[],
  c_inet         inet,
  c_cidr         cidr,
  c_macaddr      macaddr,
  c_point        point,
  c_xml          xml,
  c_bit          bit(4),
  c_varbit       varbit
);

-- Row 1: every nullable column NULL
INSERT INTO types_zoo (id) VALUES (1);

-- Row 2: minimum / empty values
INSERT INTO types_zoo VALUES (
  2,
  -32768,
  -2147483648,
  -9223372036854775808,
  -99999999999999999999.9999999999,
  -3.4028235e38,
  -1.7976931348623157e308,
  '-92233720368547758.08',
  false,
  '',
  '',
  '',
  '\x00',
  '0001-01-01',
  '00:00:00',
  '00:00:00+00',
  '0001-01-01 00:00:00',
  '0001-01-01 00:00:00+00',
  interval '-178000000 years',
  '00000000-0000-0000-0000-000000000000',
  'null',
  '[]',
  '{}',
  '{}',
  '0.0.0.0',
  '0.0.0.0/0',
  '00:00:00:00:00:00',
  '(0,0)',
  '<e/>',
  B'0000',
  B''
);

-- Row 3: maximum / typical values
INSERT INTO types_zoo VALUES (
  3,
  32767,
  2147483647,
  9223372036854775807,
  123456789012345678901234567890.123456789,
  3.4028235e38,
  1.7976931348623157e308,
  '92233720368547758.07',
  true,
  'abcde',
  'The quick brown fox ü 日本',
  E'Multi-line\ntext with "quotes", ''apostrophes'' and a tab\there',
  '\xdeadbeef00ff',
  '9999-12-31',
  '23:59:59.999999',
  '23:59:59.999999+14',
  '9999-12-31 23:59:59.999999',
  '9999-12-31 12:00:00+00',
  interval '1 year 2 months 3 days 04:05:06.789',
  'ffffffff-ffff-ffff-ffff-ffffffffffff',
  '{"a": 1, "b": [1, 2, {"c": null}], "d": "ü", "e": "\u00e9", "f": 1.5e300}',
  '{"a": 1, "b": [1, 2, {"c": null}], "d": "ü", "e": "\u00e9", "f": 1.5e300}',
  ARRAY[[1, 2], [3, 4]],
  ARRAY['a', 'b c', 'd,e', 'q"uote', 'back\slash', NULL, ''],
  '192.168.1.10/24',
  '10.0.0.0/8',
  '08:00:2b:01:02:03',
  '(1.5,-2.5)',
  '<root attr="1"><child>text &amp; more</child></root>',
  B'1010',
  B'101010101'
);

-- ---------------------------------------------------------------------------
-- "Order Items": space + capitals in the table name, composite PK
-- ---------------------------------------------------------------------------
CREATE TABLE "Order Items" (
  order_id  integer NOT NULL,
  item      text NOT NULL,
  qty       integer NOT NULL DEFAULT 1,
  price     numeric(10,2) NOT NULL,
  PRIMARY KEY (order_id, item)
);

INSERT INTO "Order Items" (order_id, item, qty, price) VALUES
  (1, 'Keyboard',      1,   49.99),
  (1, 'Mouse',         2,   19.50),
  (1, 'USB-C cable',   3,    7.25),
  (2, 'Monitor 27"',   1,  329.00),
  (2, 'HDMI cable',    1,   12.00),
  (3, 'Laptop stand',  1,   39.95),
  (3, 'Keyboard',      1,   49.99),
  (4, 'Webcam',        1,   89.00),
  (4, 'Headset',       1,  129.99),
  (5, 'Desk mat 90x40', 1,  24.00);

-- ---------------------------------------------------------------------------
-- "CamelCase": reserved words as column names
-- ---------------------------------------------------------------------------
CREATE TABLE "CamelCase" (
  "Id"           integer PRIMARY KEY,
  "DisplayName"  text,
  "select"       text,
  "user"         text,
  "from"         text
);

INSERT INTO "CamelCase" ("Id", "DisplayName", "select", "user", "from") VALUES
  (1, 'First',  'select *',   'root',    'origin'),
  (2, 'Second', 'select 1',   'alice',   'earth'),
  (3, 'Third',  NULL,         'bob',     NULL),
  (4, 'Fourth', 'SELECT',     'carol',   'FROM'),
  (5, 'Fifth',  '',           '',        '');

-- ---------------------------------------------------------------------------
-- users: edge rows (ids 51+)
-- ---------------------------------------------------------------------------
-- created_at extremes are stored at noon UTC so the calendar date is the same in every
-- session time zone (midnight UTC would render as year 10000 / year 0 in some zones).
INSERT INTO users (email, name, nickname, role, is_active, big_id, score, created_at) VALUES
  ('emoji@example.com',     'Zoë 🚀 Émilie 🎉',          '🙂',      'viewer', true,  1, NULL,  now()),
  ('cjk@example.com',       '山田 太郎',                 '太郎',    'editor', true,  2, NULL,  now()),
  ('rtl@example.com',       'محمد عبد الله',             NULL,      'viewer', true,  3, NULL,  now()),
  ('newline@example.com',   E'Line one\nLine two',       NULL,      'viewer', true,  4, NULL,  now()),
  ('empty@example.com',     '',                          NULL,      'viewer', true,  5, NULL,  now()),   -- empty name, NULL nickname
  ('spaces@example.com',    '  padded name  ',           '  ',      'viewer', true,  6, NULL,  now()),
  ('year0001@example.com',  'Year One',                  NULL,      'viewer', false, 7, NULL,  '0001-01-01 12:00:00+00'),
  ('year9999@example.com',  'Year Nine Thousand',        NULL,      'viewer', false, 8, NULL,  '9999-12-31 12:00:00+00'),
  ('negzero@example.com',   'Negative Zero',             NULL,      'viewer', true,  9, '-0'::double precision, now()),
  ('huge@example.com',      'Huge Score',                NULL,      'viewer', true, 10, 1e308, now()),
  ('nullscore@example.com', 'Null Score',                NULL,      'viewer', true, 11, NULL,  now());

REFRESH MATERIALIZED VIEW post_counts_mv;
