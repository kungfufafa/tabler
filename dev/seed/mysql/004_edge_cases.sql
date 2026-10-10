-- Tabler dev seed / MySQL / 004: edge cases and odd objects
--
--   types_zoo      one column per scalar type; row 1 all NULL, row 2 "min", row 3 "max"/typical
--   `Order Items`  table name with a space and capitals, composite PK
--   `CamelCase`    mixed-case table with reserved words as column names
--   users          extra rows with awkward names, extreme datetimes and special doubles
--
-- time_zone is pinned to UTC so the TIMESTAMP extremes are valid regardless of the
-- server's zone.

SET NAMES utf8mb4;
SET time_zone = '+00:00';

DROP TABLE IF EXISTS types_zoo;
DROP TABLE IF EXISTS `Order Items`;
DROP TABLE IF EXISTS `CamelCase`;

-- ---------------------------------------------------------------------------
-- types_zoo
-- ---------------------------------------------------------------------------
CREATE TABLE types_zoo (
  id                 int NOT NULL PRIMARY KEY,
  c_tinyint          tinyint,
  c_smallint         smallint,
  c_mediumint        mediumint,
  c_int              int,
  c_bigint_unsigned  bigint unsigned,
  c_decimal          decimal(10,3),
  c_float            float,
  c_double           double,
  c_bit1             bit(1),
  c_bit8             bit(8),
  c_char             char(5),
  c_varchar          varchar(50),
  c_tinytext         tinytext,
  c_text             text,
  c_longtext         longtext,
  c_binary           binary(4),
  c_varbinary        varbinary(8),
  c_tinyblob         tinyblob,
  c_blob             blob,
  c_date             date,
  c_time             time,
  c_datetime         datetime(6),
  c_timestamp        timestamp NULL DEFAULT NULL,
  c_year             year,
  c_enum             enum('x', 'y'),
  c_set              set('a', 'b'),
  c_json             json,
  c_geometry         geometry
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

-- Row 1: every nullable column NULL
INSERT INTO types_zoo (id) VALUES (1);

-- Row 2: minimum / empty values
INSERT INTO types_zoo VALUES (
  2,
  -128,
  -32768,
  -8388608,
  -2147483648,
  0,
  -9999999.999,
  -3.402823466e38,
  -1.7976931348623157e308,
  b'0',
  b'00000000',
  '',
  '',
  '',
  '',
  '',
  X'00000000',
  X'',
  X'',
  X'',
  '1000-01-01',
  '-838:59:59',
  '1000-01-01 00:00:00.000000',
  '1970-01-01 00:00:01',
  1901,
  'x',
  '',
  'null',
  ST_GeomFromText('POINT(0 0)')
);

-- Row 3: maximum / typical values
INSERT INTO types_zoo VALUES (
  3,
  127,
  32767,
  8388607,
  2147483647,
  18446744073709551615,
  9999999.999,
  3.402823466e38,
  1.7976931348623157e308,
  b'1',
  b'11111111',
  'abcde',
  'The quick brown fox ü 日本',
  REPEAT('t', 255),
  'Multi-line\ntext with "quotes", \'apostrophes\' and a tab\there',
  REPEAT('L', 1000),
  X'DEADBEEF',
  X'0102030405060708',
  X'FF00',
  X'89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4890000000d4944415478da63f8cfc0f01f00050001ff56c72f0d0000000049454e44ae426082',
  '9999-12-31',
  '838:59:59',
  '9999-12-31 23:59:59.999999',
  '2038-01-19 03:14:07',
  2155,
  'y',
  'a,b',
  '{"a": 1, "b": [1, 2, {"c": null}], "d": "ü", "e": "\\u00e9", "f": 1.5e300}',
  ST_GeomFromText('POINT(106.8456 -6.2088)')
);

-- ---------------------------------------------------------------------------
-- `Order Items`: space + capitals in the table name, composite PK
-- (item is varchar(100): TEXT cannot be part of a PK without a prefix length)
-- ---------------------------------------------------------------------------
CREATE TABLE `Order Items` (
  order_id  int NOT NULL,
  item      varchar(100) NOT NULL,
  qty       int NOT NULL DEFAULT 1,
  price     decimal(10,2) NOT NULL,
  PRIMARY KEY (order_id, item)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

INSERT INTO `Order Items` (order_id, item, qty, price) VALUES
  (1, 'Keyboard',       1,   49.99),
  (1, 'Mouse',          2,   19.50),
  (1, 'USB-C cable',    3,    7.25),
  (2, 'Monitor 27"',    1,  329.00),
  (2, 'HDMI cable',     1,   12.00),
  (3, 'Laptop stand',   1,   39.95),
  (3, 'Keyboard',       1,   49.99),
  (4, 'Webcam',         1,   89.00),
  (4, 'Headset',        1,  129.99),
  (5, 'Desk mat 90x40', 1,   24.00);

-- ---------------------------------------------------------------------------
-- `CamelCase`: reserved words as column names
-- ---------------------------------------------------------------------------
CREATE TABLE `CamelCase` (
  `Id`           int NOT NULL PRIMARY KEY,
  `DisplayName`  text,
  `select`       text,
  `user`         text,
  `from`         text
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

INSERT INTO `CamelCase` (`Id`, `DisplayName`, `select`, `user`, `from`) VALUES
  (1, 'First',  'select *',  'root',   'origin'),
  (2, 'Second', 'select 1',  'alice',  'earth'),
  (3, 'Third',  NULL,        'bob',    NULL),
  (4, 'Fourth', 'SELECT',    'carol',  'FROM'),
  (5, 'Fifth',  '',          '',       '');

-- ---------------------------------------------------------------------------
-- users: edge rows (ids 51+)
-- ---------------------------------------------------------------------------
-- MySQL DATETIME spans 1000-01-01 .. 9999-12-31, so the "year 1" row uses 1000-01-01.
-- A literal -0.0 is stored as 0 by MySQL; CAST('-0' AS DOUBLE) keeps the sign.
INSERT INTO users (email, name, nickname, role, is_active, big_id, score, created_at) VALUES
  ('emoji@example.com',     'Zoë 🚀 Émilie 🎉',      '🙂',    'viewer', true,  1, NULL,  CURRENT_TIMESTAMP(6)),
  ('cjk@example.com',       '山田 太郎',             '太郎',  'editor', true,  2, NULL,  CURRENT_TIMESTAMP(6)),
  ('rtl@example.com',       'محمد عبد الله',         NULL,    'viewer', true,  3, NULL,  CURRENT_TIMESTAMP(6)),
  ('newline@example.com',   'Line one\nLine two',    NULL,    'viewer', true,  4, NULL,  CURRENT_TIMESTAMP(6)),
  ('empty@example.com',     '',                      NULL,    'viewer', true,  5, NULL,  CURRENT_TIMESTAMP(6)),   -- empty name, NULL nickname
  ('spaces@example.com',    '  padded name  ',       '  ',    'viewer', true,  6, NULL,  CURRENT_TIMESTAMP(6)),
  ('year0001@example.com',  'Year One',              NULL,    'viewer', false, 7, NULL,  '1000-01-01 00:00:00'),
  ('year9999@example.com',  'Year Nine Thousand',    NULL,    'viewer', false, 8, NULL,  '9999-12-31 23:59:59.999999'),
  ('negzero@example.com',   'Negative Zero',         NULL,    'viewer', true,  9, CAST('-0' AS DOUBLE), CURRENT_TIMESTAMP(6)),
  ('huge@example.com',      'Huge Score',            NULL,    'viewer', true, 10, 1e308, CURRENT_TIMESTAMP(6)),
  ('nullscore@example.com', 'Null Score',            NULL,    'viewer', true, 11, NULL,  CURRENT_TIMESTAMP(6));
