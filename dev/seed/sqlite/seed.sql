-- Tabler dev seed / SQLite
--
-- Builds dev/fixtures/sample.sqlite (see dev/scripts/build-sqlite.sh, which always starts
-- from an empty file: `sqlite3 dev/fixtures/sample.sqlite < dev/seed/sqlite/seed.sql`).
-- Needs SQLite >= 3.38 (STRICT tables, built-in JSON functions).
--
-- Object names are shared with the PostgreSQL and MySQL seeds; the app's tests rely on them.
-- Deterministic: no random(), so the file is byte-for-byte comparable between builds.
--   users      : 10 hand-written rows (ids 1-10) + 40 generated (ids 11-50) + 11 edge rows
--   tags       : 15        posts: 200        post_tags: 420        audit_log: 30
--   events_big : 100 000   types_zoo: 3      types_zoo_affinity: 3
--   "Order Items": 10      "CamelCase": 5
-- (no inventory schema: SQLite has no schemas / second database in one file)

PRAGMA foreign_keys = ON;
PRAGMA encoding = 'UTF-8';

BEGIN;

-- ---------------------------------------------------------------------------
-- Reset (the build script starts from an empty file; kept for manual re-runs)
-- ---------------------------------------------------------------------------
DROP VIEW  IF EXISTS post_counts;
DROP VIEW  IF EXISTS active_users;
DROP TABLE IF EXISTS post_tags;
DROP TABLE IF EXISTS posts;
DROP TABLE IF EXISTS tags;
DROP TABLE IF EXISTS audit_log;
DROP TABLE IF EXISTS events_big;
DROP TABLE IF EXISTS types_zoo;
DROP TABLE IF EXISTS types_zoo_affinity;
DROP TABLE IF EXISTS "Order Items";
DROP TABLE IF EXISTS "CamelCase";
DROP TABLE IF EXISTS users;

-- ---------------------------------------------------------------------------
-- users
-- ---------------------------------------------------------------------------
CREATE TABLE users (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  email       TEXT NOT NULL UNIQUE,
  name        TEXT NOT NULL,
  nickname    TEXT,
  role        TEXT NOT NULL DEFAULT 'viewer' CHECK (role IN ('admin', 'editor', 'viewer')),
  is_active   BOOLEAN NOT NULL DEFAULT 1 CHECK (is_active IN (0, 1)),
  balance     NUMERIC(20,6) NOT NULL DEFAULT 0,
  big_id      INTEGER,
  uid         TEXT,                -- uuid as text
  settings    TEXT,                -- JSON document
  tags        TEXT,                -- comma-separated subset of a,b,c,d
  avatar      BLOB,
  bio         TEXT,
  score       REAL,
  created_at  TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  last_login  TEXT
);

-- ---------------------------------------------------------------------------
-- posts (+ updated_at trigger)
-- ---------------------------------------------------------------------------
CREATE TABLE posts (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  user_id       INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  title         TEXT NOT NULL,
  body          TEXT,
  status        TEXT NOT NULL DEFAULT 'draft' CHECK (status IN ('draft', 'published', 'archived')),
  published_at  TEXT,
  views         INTEGER NOT NULL DEFAULT 0,
  created_at    TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at    TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX posts_user_id_idx             ON posts (user_id);
CREATE INDEX posts_status_published_at_idx ON posts (status, published_at);

-- SQLite has no BEFORE-UPDATE "SET NEW.x"; the idiom is an AFTER UPDATE trigger that
-- updates the same row (recursive_triggers is off by default, so it does not re-fire).
CREATE TRIGGER posts_set_updated_at
  AFTER UPDATE ON posts
  FOR EACH ROW
BEGIN
  UPDATE posts SET updated_at = CURRENT_TIMESTAMP WHERE id = NEW.id;
END;

-- ---------------------------------------------------------------------------
-- tags / post_tags
-- ---------------------------------------------------------------------------
CREATE TABLE tags (
  id    INTEGER PRIMARY KEY AUTOINCREMENT,
  name  TEXT NOT NULL UNIQUE
);

CREATE TABLE post_tags (
  post_id  INTEGER NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
  tag_id   INTEGER NOT NULL REFERENCES tags(id)  ON DELETE CASCADE,
  PRIMARY KEY (post_id, tag_id)
) WITHOUT ROWID;

-- ---------------------------------------------------------------------------
-- audit_log: deliberately has NO primary key
-- ---------------------------------------------------------------------------
CREATE TABLE audit_log (
  at       TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  actor    TEXT,
  action   TEXT NOT NULL,
  payload  TEXT                    -- JSON document
);

-- ---------------------------------------------------------------------------
-- events_big
-- ---------------------------------------------------------------------------
CREATE TABLE events_big (
  id       INTEGER PRIMARY KEY AUTOINCREMENT,
  ts       TEXT NOT NULL,
  kind     TEXT NOT NULL CHECK (kind IN ('view', 'click', 'buy', 'refund')),
  value    NUMERIC(12,2) NOT NULL,
  payload  TEXT                    -- JSON document
);

-- ---------------------------------------------------------------------------
-- types_zoo (STRICT) and types_zoo_affinity (classic affinity rules)
-- ---------------------------------------------------------------------------
CREATE TABLE types_zoo (
  id         INTEGER PRIMARY KEY,
  c_int      INT,
  c_integer  INTEGER,
  c_real     REAL,
  c_text     TEXT,
  c_blob     BLOB,
  c_any      ANY
) STRICT;

CREATE TABLE types_zoo_affinity (
  id          INTEGER PRIMARY KEY,
  c_date      DATE,
  c_datetime  DATETIME,
  c_boolean   BOOLEAN,
  c_numeric   NUMERIC,
  c_json      JSON
);

-- ---------------------------------------------------------------------------
-- "Order Items": space + capitals in the table name, composite PK
-- ---------------------------------------------------------------------------
CREATE TABLE "Order Items" (
  order_id  INTEGER NOT NULL,
  item      TEXT NOT NULL,
  qty       INTEGER NOT NULL DEFAULT 1,
  price     NUMERIC(10,2) NOT NULL,
  PRIMARY KEY (order_id, item)
);

-- ---------------------------------------------------------------------------
-- "CamelCase": reserved words as column names
-- ---------------------------------------------------------------------------
CREATE TABLE "CamelCase" (
  "Id"           INTEGER PRIMARY KEY,
  "DisplayName"  TEXT,
  "select"       TEXT,
  "user"         TEXT,
  "from"         TEXT
);

-- ---------------------------------------------------------------------------
-- Views
-- ---------------------------------------------------------------------------
CREATE VIEW active_users AS
  SELECT * FROM users WHERE is_active = 1;

CREATE VIEW post_counts AS
  SELECT u.id AS user_id, u.name, count(p.id) AS post_count
  FROM users u
  LEFT JOIN posts p ON p.user_id = u.id
  GROUP BY u.id, u.name;

-- ===========================================================================
-- DATA
-- ===========================================================================

-- ---------------------------------------------------------------------------
-- users: hand-written rows exercising every column
-- ---------------------------------------------------------------------------
INSERT INTO users
  (email, name, nickname, role, is_active, balance, big_id, uid, settings, tags, avatar, bio, score, created_at, last_login)
VALUES
  ('alice@example.com', 'Alice Admin', 'ali', 'admin', 1, 1500.25,
   9007199254740993,                      -- 2^53 + 1: not representable as a JS double
   '11111111-1111-4111-8111-111111111111',
   '{"theme": "dark", "notifications": {"email": true, "push": false}, "limits": [10, 20, 30]}',
   'a,b',
   X'89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4890000000d4944415478da63f8cfc0f01f00050001ff56c72f0d0000000049454e44ae426082',
   NULL, 98.5, '2024-01-05 09:30:00', '2024-09-01 12:00:00'),

  ('bob@example.com', 'Bob Builder', NULL, 'editor', 1, -42.000001,
   -9223372036854775808,                  -- 64-bit minimum
   '22222222-2222-4222-8222-222222222222',
   '{"theme": "light"}',
   'c',
   NULL,
   -- exactly 10 000 chars (SQLite has no repeat(); hex(zeroblob(n)) gives 2n '0' chars)
   substr(replace(hex(zeroblob(200)), '00', 'Lorem ipsum dolor sit amet, consectetur adipiscing elit. '), 1, 10000),
   0, '2024-01-06 10:00:00', NULL),

  ('carol@example.com', 'Carol Chen', 'cc', 'viewer', 0, 0,
   9223372036854775807,                   -- 64-bit maximum
   '33333333-3333-4333-8333-333333333333',
   NULL, NULL,
   X'89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4890000000d4944415478da63f8cfc0f01f00050001ff56c72f0d0000000049454e44ae426082',
   NULL, NULL, '2024-01-07 11:15:00', '2024-03-15 08:45:30'),

  ('dave@example.com', 'Dave Davis', 'dd', 'editor', 1, 12.5, 0,
   '44444444-4444-4444-8444-444444444444',
   '[1, 2, 3]', '', NULL, NULL, 1.5, '2024-01-08 12:00:00', '2024-08-20 23:59:59'),

  ('eve@example.com', 'Eve Evans', NULL, 'viewer', 1, 0.000001, 42,
   '55555555-5555-4555-8555-555555555555',
   '"just a string"', 'a,b,c,d', NULL, 'Short bio.', -273.15, '2024-01-09 13:00:00', NULL),

  ('frank@example.com', 'Frank Furter', 'ff', 'admin', 1, 99999999999999.999999, 1234567890123,
   '66666666-6666-4666-8666-666666666666',
   '{"nested": {"deep": {"deeper": {"deepest": true}}}}', 'd', NULL, NULL, 3.14159, '2024-01-10 14:00:00', '2024-01-10 14:05:00'),

  ('grace@example.com', 'Grace Hopper', 'amazing', 'editor', 1, -99999999999999.999999, -1,
   '77777777-7777-4777-8777-777777777777',
   '{"theme": "dark", "lang": "en-US"}', 'b,c', NULL, 'COBOL pioneer.', 100, '2024-01-11 15:00:00', '2024-07-04 00:00:00'),

  ('heidi@example.com', 'Heidi Klum', NULL, 'viewer', 0, 7.5, 4294967296,
   '88888888-8888-4888-8888-888888888888',
   '{}', NULL, NULL, NULL, 55.5, '2024-01-12 16:00:00', NULL),

  ('ivan@example.com', 'Ivan Ivanov', 'vanya', 'viewer', 1, 1000, -4294967297,
   '99999999-9999-4999-8999-999999999999',
   '{"emoji": "🚀", "unicode": "üñ"}', 'a', NULL, NULL, 0.1, '2024-01-13 17:00:00', '2024-02-29 12:00:00'),

  ('judy@example.com', 'Judy Jetson', 'jj', 'editor', 1, 250.75, 2147483648,
   'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
   '{"a": null, "b": false, "c": 1e100}', 'c,d', NULL, NULL, 42, '2024-01-14 18:00:00', '2024-05-05 05:05:05');

-- users: generated rows 11-50 (uid is a deterministic, v4-shaped UUID)
INSERT INTO users
  (email, name, nickname, role, is_active, balance, big_id, uid, settings, tags, score, created_at, last_login)
WITH RECURSIVE seq(i) AS (SELECT 11 UNION ALL SELECT i + 1 FROM seq WHERE i < 50)
SELECT
  'user' || i || '@example.com',
  'User ' || i,
  CASE WHEN i % 3 = 0 THEN NULL ELSE 'u' || i END,
  CASE i % 3 WHEN 0 THEN 'admin' WHEN 1 THEN 'editor' ELSE 'viewer' END,
  i % 7 <> 0,
  round(i * 1234.5678, 6),
  i * 1000000007,
  printf('00000000-0000-4000-8000-%012d', i),
  json_object('theme', CASE WHEN i % 2 = 0 THEN 'dark' ELSE 'light' END, 'level', i),
  CASE i % 4 WHEN 0 THEN 'a' WHEN 1 THEN 'a,b' WHEN 2 THEN 'b,c,d' ELSE 'd' END,
  CASE WHEN i % 9 = 0 THEN NULL ELSE ((i * 37) % 101) / 10.0 END,
  datetime('2024-02-01 00:00:00', '+' || i || ' days'),
  CASE WHEN i % 5 = 0 THEN NULL ELSE datetime('2024-06-01 00:00:00', '+' || i || ' hours') END
FROM seq;

-- ---------------------------------------------------------------------------
-- tags
-- ---------------------------------------------------------------------------
INSERT INTO tags (name) VALUES
  ('rust'), ('sql'), ('postgres'), ('mysql'), ('sqlite'),
  ('tauri'), ('react'), ('typescript'), ('devops'), ('docker'),
  ('testing'), ('ui'), ('performance'), ('security'), ('docs');

-- ---------------------------------------------------------------------------
-- posts: 200 rows; status cycles draft/published/archived
-- ---------------------------------------------------------------------------
INSERT INTO posts (user_id, title, body, status, published_at, views, created_at, updated_at)
WITH RECURSIVE seq(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM seq WHERE i < 200)
SELECT
  1 + i % 50,
  'Post #' || i || ': ' ||
    CASE i % 5 WHEN 0 THEN 'Getting started' WHEN 1 THEN 'Deep dive' WHEN 2 THEN 'Release notes'
               WHEN 3 THEN 'Retrospective' ELSE 'FAQ' END,
  CASE WHEN i % 10 = 0 THEN NULL
       ELSE 'Body of post ' || i || '. ' ||
            substr(replace(hex(zeroblob(5)), '00', 'Lorem ipsum dolor sit amet, consectetur adipiscing elit. '), 1, 56 * (1 + i % 5))
  END,
  CASE i % 3 WHEN 0 THEN 'draft' WHEN 1 THEN 'published' ELSE 'archived' END,
  CASE WHEN i % 3 <> 0 THEN datetime('2024-02-01 00:00:00', '+' || i || ' days') END,
  (i * 37) % 1000,
  datetime('2024-01-01 00:00:00', '+' || (i * 6) || ' hours'),
  datetime('2024-01-01 00:00:00', '+' || (i * 6) || ' hours')
FROM seq;

-- ---------------------------------------------------------------------------
-- post_tags: 2 tags per post + a 3rd for every 10th post = 420 rows
-- ---------------------------------------------------------------------------
INSERT INTO post_tags (post_id, tag_id)
SELECT id, 1 + id % 15 FROM posts
UNION ALL
SELECT id, 1 + (id + 1) % 15 FROM posts
UNION ALL
SELECT id, 1 + (id + 7) % 15 FROM posts WHERE id % 10 = 0;

-- ---------------------------------------------------------------------------
-- audit_log: 30 rows, no PK
-- ---------------------------------------------------------------------------
INSERT INTO audit_log (at, actor, action, payload)
WITH RECURSIVE seq(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM seq WHERE i < 30)
SELECT
  datetime('2024-03-01 08:00:00', '+' || i || ' hours'),
  CASE i % 5 WHEN 0 THEN 'alice' WHEN 1 THEN 'bob' WHEN 2 THEN 'carol' WHEN 3 THEN 'system' ELSE NULL END,
  CASE i % 4 WHEN 0 THEN 'login' WHEN 1 THEN 'update' WHEN 2 THEN 'delete' ELSE 'export' END,
  json_object('seq', i, 'ip', '10.0.0.' || i, 'ok', json(CASE WHEN i % 4 <> 0 THEN 'true' ELSE 'false' END))
FROM seq;

-- ---------------------------------------------------------------------------
-- events_big: 100 000 rows via recursive CTE; modulo arithmetic only (no random()).
-- kind distribution: view 53 333 / click 26 667 / buy 15 000 / refund 5 000.
-- ---------------------------------------------------------------------------
INSERT INTO events_big (ts, kind, value, payload)
WITH RECURSIVE seq(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM seq WHERE i < 100000)
SELECT
  datetime('2024-01-01 00:00:00', '+' || (i * 37) || ' seconds'),
  CASE
    WHEN i % 20 = 0 THEN 'refund'
    WHEN i % 5  = 0 THEN 'buy'
    WHEN i % 3  = 0 THEN 'click'
    ELSE 'view'
  END,
  ((i * 7919) % 100000) / 100.0,
  json_object('i', i, 'user', 1 + i % 50, 'bucket', i % 16)
FROM seq;

-- Index after the bulk load.
CREATE INDEX events_big_kind_ts_idx ON events_big (kind, ts);

-- ---------------------------------------------------------------------------
-- types_zoo: row 1 all NULL, row 2 "min", row 3 "max"/typical
-- (STRICT: values must match the declared type; ANY keeps whatever it is given)
-- ---------------------------------------------------------------------------
INSERT INTO types_zoo (id) VALUES (1);
INSERT INTO types_zoo VALUES (2, -2147483648, -9223372036854775808, -1.7976931348623157e308, '', X'', '42');
INSERT INTO types_zoo VALUES (3, 2147483647, 9223372036854775807, 1.7976931348623157e308,
  'The quick brown fox ü 日本' || char(10) || 'line two with "quotes" and ''apostrophes''',
  X'DEADBEEF00FF', 3.14);

-- Non-strict: declared types only pick an affinity (e.g. the NUMERIC text is converted,
-- the big NUMERIC loses precision as a REAL, BOOLEAN is stored as INTEGER 0/1).
INSERT INTO types_zoo_affinity (id) VALUES (1);
INSERT INTO types_zoo_affinity VALUES (2, '0001-01-01', '0001-01-01 00:00:00', 0, '-9223372036854775808', 'null');
INSERT INTO types_zoo_affinity VALUES (3, '9999-12-31', '9999-12-31 23:59:59.999', 1, '12345678901234567890.123',
  '{"a": 1, "b": [1, 2, {"c": null}], "d": "ü", "e": "\u00e9", "f": 1.5e300}');

-- ---------------------------------------------------------------------------
-- "Order Items"
-- ---------------------------------------------------------------------------
INSERT INTO "Order Items" (order_id, item, qty, price) VALUES
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
-- "CamelCase"
-- ---------------------------------------------------------------------------
INSERT INTO "CamelCase" ("Id", "DisplayName", "select", "user", "from") VALUES
  (1, 'First',  'select *',  'root',   'origin'),
  (2, 'Second', 'select 1',  'alice',  'earth'),
  (3, 'Third',  NULL,        'bob',    NULL),
  (4, 'Fourth', 'SELECT',    'carol',  'FROM'),
  (5, 'Fifth',  '',          '',       '');

-- ---------------------------------------------------------------------------
-- users: edge rows (ids 51+)
-- ---------------------------------------------------------------------------
-- Note: SQLite stores integral REALs as integers on disk, so -0.0 comes back as 0.0.
INSERT INTO users (email, name, nickname, role, is_active, big_id, score, created_at) VALUES
  ('emoji@example.com',     'Zoë 🚀 Émilie 🎉',                   '🙂',   'viewer', 1,  1, NULL,  CURRENT_TIMESTAMP),
  ('cjk@example.com',       '山田 太郎',                          '太郎', 'editor', 1,  2, NULL,  CURRENT_TIMESTAMP),
  ('rtl@example.com',       'محمد عبد الله',                      NULL,   'viewer', 1,  3, NULL,  CURRENT_TIMESTAMP),
  ('newline@example.com',   'Line one' || char(10) || 'Line two', NULL,   'viewer', 1,  4, NULL,  CURRENT_TIMESTAMP),
  ('empty@example.com',     '',                                   NULL,   'viewer', 1,  5, NULL,  CURRENT_TIMESTAMP),   -- empty name, NULL nickname
  ('spaces@example.com',    '  padded name  ',                    '  ',   'viewer', 1,  6, NULL,  CURRENT_TIMESTAMP),
  ('year0001@example.com',  'Year One',                           NULL,   'viewer', 0,  7, NULL,  '0001-01-01 00:00:00'),
  ('year9999@example.com',  'Year Nine Thousand',                 NULL,   'viewer', 0,  8, NULL,  '9999-12-31 23:59:59'),
  ('negzero@example.com',   'Negative Zero',                      NULL,   'viewer', 1,  9, -0.0,  CURRENT_TIMESTAMP),
  ('huge@example.com',      'Huge Score',                         NULL,   'viewer', 1, 10, 1e308, CURRENT_TIMESTAMP),
  ('nullscore@example.com', 'Null Score',                         NULL,   'viewer', 1, 11, NULL,  CURRENT_TIMESTAMP);

COMMIT;

PRAGMA user_version = 1;
VACUUM;
