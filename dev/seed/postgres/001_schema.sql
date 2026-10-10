-- Tabler dev seed / PostgreSQL / 001: core schema
--
-- Applied to an empty `tabler_dev` by dev/scripts/db-seed.sh (or by the postgres
-- container's /docker-entrypoint-initdb.d). Every object is dropped before it is
-- created, so re-applying the file to the same database is safe.
--
-- Object names are shared with the MySQL and SQLite seeds; the app's tests rely on them.

SET client_encoding = 'UTF8';
SET client_min_messages = warning;

-- ---------------------------------------------------------------------------
-- Reset (dependency order)
-- ---------------------------------------------------------------------------
DROP MATERIALIZED VIEW IF EXISTS post_counts_mv;
DROP VIEW IF EXISTS post_counts;
DROP VIEW IF EXISTS active_users;
DROP TABLE IF EXISTS post_tags;
DROP TABLE IF EXISTS posts;
DROP TABLE IF EXISTS tags;
DROP TABLE IF EXISTS audit_log;
DROP TABLE IF EXISTS users;
DROP SCHEMA IF EXISTS inventory CASCADE;
DROP FUNCTION IF EXISTS set_updated_at();
DROP TYPE IF EXISTS user_role;
DROP TYPE IF EXISTS post_status;

-- ---------------------------------------------------------------------------
-- Enum types
-- ---------------------------------------------------------------------------
CREATE TYPE user_role   AS ENUM ('admin', 'editor', 'viewer');
CREATE TYPE post_status AS ENUM ('draft', 'published', 'archived');

-- ---------------------------------------------------------------------------
-- users
-- ---------------------------------------------------------------------------
CREATE TABLE users (
  id          bigserial PRIMARY KEY,
  email       text NOT NULL UNIQUE,
  name        text NOT NULL,
  nickname    text,
  role        user_role NOT NULL DEFAULT 'viewer',
  is_active   boolean NOT NULL DEFAULT true,
  balance     numeric(20,6) NOT NULL DEFAULT 0,
  big_id      bigint,
  uid         uuid NOT NULL DEFAULT gen_random_uuid(),
  settings    jsonb,
  tags        text[],
  avatar      bytea,
  bio         text,
  score       double precision,
  created_at  timestamptz NOT NULL DEFAULT now(),
  last_login  timestamp
);

COMMENT ON TABLE  users          IS 'Application accounts (dev seed)';
COMMENT ON COLUMN users.big_id   IS 'Exercises 64-bit values beyond JS Number.MAX_SAFE_INTEGER';
COMMENT ON COLUMN users.settings IS 'Free-form per-user preferences';

-- ---------------------------------------------------------------------------
-- posts (+ updated_at trigger)
-- ---------------------------------------------------------------------------
CREATE TABLE posts (
  id            bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  user_id       bigint NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  title         text NOT NULL,
  body          text,
  status        post_status NOT NULL DEFAULT 'draft',
  published_at  timestamptz,
  views         integer NOT NULL DEFAULT 0,
  created_at    timestamptz NOT NULL DEFAULT now(),
  updated_at    timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX posts_user_id_idx             ON posts (user_id);
CREATE INDEX posts_status_published_at_idx ON posts (status, published_at);

CREATE FUNCTION set_updated_at() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
  NEW.updated_at := now();
  RETURN NEW;
END;
$$;

CREATE TRIGGER posts_set_updated_at
  BEFORE UPDATE ON posts
  FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ---------------------------------------------------------------------------
-- tags / post_tags
-- ---------------------------------------------------------------------------
CREATE TABLE tags (
  id    serial PRIMARY KEY,
  name  text NOT NULL UNIQUE
);

CREATE TABLE post_tags (
  post_id  bigint  NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
  tag_id   integer NOT NULL REFERENCES tags(id)  ON DELETE CASCADE,
  PRIMARY KEY (post_id, tag_id)
);

-- ---------------------------------------------------------------------------
-- audit_log: deliberately has NO primary key
-- ---------------------------------------------------------------------------
CREATE TABLE audit_log (
  at       timestamp NOT NULL DEFAULT now(),
  actor    text,
  action   text NOT NULL,
  payload  jsonb
);

-- ---------------------------------------------------------------------------
-- inventory schema (second schema in the same database)
-- ---------------------------------------------------------------------------
CREATE SCHEMA inventory;

CREATE TABLE inventory.items (
  id     serial PRIMARY KEY,
  sku    text NOT NULL UNIQUE,
  name   text NOT NULL,
  stock  integer NOT NULL DEFAULT 0
);

CREATE TABLE inventory.stock_movements (
  id       bigserial PRIMARY KEY,
  item_id  integer NOT NULL REFERENCES inventory.items(id) ON DELETE CASCADE,
  delta    integer NOT NULL,
  at       timestamptz NOT NULL DEFAULT now()
);

-- ---------------------------------------------------------------------------
-- Views
-- ---------------------------------------------------------------------------
CREATE VIEW active_users AS
  SELECT * FROM users WHERE is_active;

CREATE VIEW post_counts AS
  SELECT u.id AS user_id, u.name, count(p.id) AS post_count
  FROM users u
  LEFT JOIN posts p ON p.user_id = u.id
  GROUP BY u.id, u.name;

-- Populated with REFRESH MATERIALIZED VIEW at the end of 002_data.sql and 004_edge_cases.sql.
CREATE MATERIALIZED VIEW post_counts_mv AS
  SELECT * FROM post_counts
WITH NO DATA;
