-- Tabler dev seed / PostgreSQL / 002: core data
--
-- Deterministic: no random(), so ids and values are identical on every reseed.
--   users       : 10 hand-written rows (ids 1-10) + 40 generated (ids 11-50)
--   tags        : 15
--   posts       : 200 (ids 1-200, user_id cycles over 1-50)
--   post_tags   : 420
--   audit_log   : 30
--   inventory.* : 20 items, 40 stock movements

SET client_encoding = 'UTF8';
SET client_min_messages = warning;

-- ---------------------------------------------------------------------------
-- users: hand-written rows exercising every column
-- ---------------------------------------------------------------------------
INSERT INTO users
  (email, name, nickname, role, is_active, balance, big_id, uid, settings, tags, avatar, bio, score, created_at, last_login)
VALUES
  ('alice@example.com', 'Alice Admin', 'ali', 'admin', true, 1500.250000,
   9007199254740993,                      -- 2^53 + 1: not representable as a JS double
   '11111111-1111-4111-8111-111111111111',
   '{"theme": "dark", "notifications": {"email": true, "push": false}, "limits": [10, 20, 30]}',
   '{a,b}',
   '\x89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4890000000d4944415478da63f8cfc0f01f00050001ff56c72f0d0000000049454e44ae426082'::bytea,
   NULL, 98.5, '2024-01-05 09:30:00+00', '2024-09-01 12:00:00'),

  ('bob@example.com', 'Bob Builder', NULL, 'editor', true, -42.000001,
   -9223372036854775808,                  -- bigint minimum
   '22222222-2222-4222-8222-222222222222',
   '{"theme": "light"}',
   '{c}',
   NULL,
   left(repeat('Lorem ipsum dolor sit amet, consectetur adipiscing elit. ', 200), 10000),  -- exactly 10 000 chars
   0, '2024-01-06 10:00:00+00', NULL),

  ('carol@example.com', 'Carol Chen', 'cc', 'viewer', false, 0,
   9223372036854775807,                   -- bigint maximum
   '33333333-3333-4333-8333-333333333333',
   NULL, NULL,
   '\x89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4890000000d4944415478da63f8cfc0f01f00050001ff56c72f0d0000000049454e44ae426082'::bytea,
   NULL, NULL, '2024-01-07 11:15:00+00', '2024-03-15 08:45:30'),

  ('dave@example.com', 'Dave Davis', 'dd', 'editor', true, 12.5, 0,
   '44444444-4444-4444-8444-444444444444',
   '[1, 2, 3]', '{}', NULL, NULL, 1.5, '2024-01-08 12:00:00+00', '2024-08-20 23:59:59'),

  ('eve@example.com', 'Eve Evans', NULL, 'viewer', true, 0.000001, 42,
   '55555555-5555-4555-8555-555555555555',
   '"just a string"', '{a,b,c,d}', NULL, 'Short bio.', -273.15, '2024-01-09 13:00:00+00', NULL),

  ('frank@example.com', 'Frank Furter', 'ff', 'admin', true, 99999999999999.999999, 1234567890123,
   '66666666-6666-4666-8666-666666666666',
   '{"nested": {"deep": {"deeper": {"deepest": true}}}}', '{d}', NULL, NULL, 3.14159, '2024-01-10 14:00:00+00', '2024-01-10 14:05:00'),

  ('grace@example.com', 'Grace Hopper', 'amazing', 'editor', true, -99999999999999.999999, -1,
   '77777777-7777-4777-8777-777777777777',
   '{"theme": "dark", "lang": "en-US"}', '{b,c}', NULL, 'COBOL pioneer.', 100, '2024-01-11 15:00:00+00', '2024-07-04 00:00:00'),

  ('heidi@example.com', 'Heidi Klum', NULL, 'viewer', false, 7.5, 4294967296,
   '88888888-8888-4888-8888-888888888888',
   '{}', NULL, NULL, NULL, 55.5, '2024-01-12 16:00:00+00', NULL),

  ('ivan@example.com', 'Ivan Ivanov', 'vanya', 'viewer', true, 1000, -4294967297,
   '99999999-9999-4999-8999-999999999999',
   '{"emoji": "🚀", "unicode": "üñ"}', '{a}', NULL, NULL, 0.1, '2024-01-13 17:00:00+00', '2024-02-29 12:00:00'),

  ('judy@example.com', 'Judy Jetson', 'jj', 'editor', true, 250.75, 2147483648,
   'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
   '{"a": null, "b": false, "c": 1e100}', '{c,d}', NULL, NULL, 42, '2024-01-14 18:00:00+00', '2024-05-05 05:05:05');

-- users: generated rows 11-50
INSERT INTO users
  (email, name, nickname, role, is_active, balance, big_id, settings, tags, score, created_at, last_login)
SELECT
  format('user%s@example.com', i),
  format('User %s', i),
  CASE WHEN i % 3 = 0 THEN NULL ELSE format('u%s', i) END,
  (ARRAY['admin', 'editor', 'viewer'])[1 + i % 3]::user_role,
  i % 7 <> 0,
  (i * 1234.5678)::numeric(20,6),
  i::bigint * 1000000007,
  jsonb_build_object('theme', CASE WHEN i % 2 = 0 THEN 'dark' ELSE 'light' END, 'level', i),
  (CASE i % 4 WHEN 0 THEN '{a}' WHEN 1 THEN '{a,b}' WHEN 2 THEN '{b,c,d}' ELSE '{d}' END)::text[],
  CASE WHEN i % 9 = 0 THEN NULL ELSE ((i * 37) % 101) / 10.0 END,
  timestamptz '2024-02-01 00:00:00+00' + i * interval '1 day',
  CASE WHEN i % 5 = 0 THEN NULL ELSE timestamp '2024-06-01 00:00:00' + i * interval '1 hour' END
FROM generate_series(11, 50) AS i;

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
SELECT
  1 + i % 50,
  format('Post #%s: %s', i, (ARRAY['Getting started', 'Deep dive', 'Release notes', 'Retrospective', 'FAQ'])[1 + i % 5]),
  CASE WHEN i % 10 = 0 THEN NULL
       ELSE format('Body of post %s. ', i) || repeat('Lorem ipsum dolor sit amet, consectetur adipiscing elit. ', 1 + i % 5)
  END,
  (ARRAY['draft', 'published', 'archived'])[1 + i % 3]::post_status,
  CASE WHEN i % 3 <> 0 THEN timestamptz '2024-02-01 00:00:00+00' + i * interval '1 day' END,
  (i * 37) % 1000,
  timestamptz '2024-01-01 00:00:00+00' + i * interval '6 hours',
  timestamptz '2024-01-01 00:00:00+00' + i * interval '6 hours'
FROM generate_series(1, 200) AS i;

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
SELECT
  timestamp '2024-03-01 08:00:00' + i * interval '1 hour',
  (ARRAY['alice', 'bob', 'carol', 'system', NULL])[1 + i % 5],
  (ARRAY['login', 'update', 'delete', 'export'])[1 + i % 4],
  jsonb_build_object('seq', i, 'ip', format('10.0.0.%s', i), 'ok', i % 4 <> 0)
FROM generate_series(1, 30) AS i;

-- ---------------------------------------------------------------------------
-- inventory schema data
-- ---------------------------------------------------------------------------
INSERT INTO inventory.items (sku, name, stock)
SELECT
  format('SKU-%s', lpad(i::text, 4, '0')),
  format('%s %s', (ARRAY['Widget', 'Gadget', 'Gizmo', 'Doohickey'])[1 + i % 4], i),
  (i * 17) % 100
FROM generate_series(1, 20) AS i;

INSERT INTO inventory.stock_movements (item_id, delta, at)
SELECT
  1 + i % 20,
  ((i * 13) % 21) - 10,
  timestamptz '2024-04-01 00:00:00+00' + i * interval '3 hours'
FROM generate_series(1, 40) AS i;

-- ---------------------------------------------------------------------------
-- Materialized view
-- ---------------------------------------------------------------------------
REFRESH MATERIALIZED VIEW post_counts_mv;
