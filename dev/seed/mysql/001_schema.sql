-- Tabler dev seed / MySQL / 001: core schema
--
-- Applied with the `tabler_dev` database selected (db-seed.sh runs `mysql ... tabler_dev < file`;
-- the docker image runs it with MYSQL_DATABASE=tabler_dev). Also creates the second database
-- `tabler_inv`. Every object is dropped before it is created, so re-applying is safe.
-- Written for MySQL 8.0+ and kept compatible with MariaDB 10.6+ / 11.
--
-- Object names are shared with the PostgreSQL and SQLite seeds; the app's tests rely on them.

SET NAMES utf8mb4;
SET time_zone = '+00:00';

-- ---------------------------------------------------------------------------
-- Reset
-- ---------------------------------------------------------------------------
SET FOREIGN_KEY_CHECKS = 0;
DROP VIEW  IF EXISTS post_counts;
DROP VIEW  IF EXISTS active_users;
DROP TABLE IF EXISTS post_tags;
DROP TABLE IF EXISTS posts;
DROP TABLE IF EXISTS tags;
DROP TABLE IF EXISTS audit_log;
DROP TABLE IF EXISTS users;
SET FOREIGN_KEY_CHECKS = 1;

-- ---------------------------------------------------------------------------
-- users
-- ---------------------------------------------------------------------------
-- email is varchar(255) rather than text: MySQL cannot put a UNIQUE index on an
-- unbounded TEXT column without a prefix length.
CREATE TABLE users (
  id          bigint NOT NULL AUTO_INCREMENT PRIMARY KEY,
  email       varchar(255) NOT NULL UNIQUE,
  name        text NOT NULL,
  nickname    text NULL,
  role        enum('admin', 'editor', 'viewer') NOT NULL DEFAULT 'viewer',
  is_active   boolean NOT NULL DEFAULT true,
  balance     decimal(20,6) NOT NULL DEFAULT 0,
  big_id      bigint NULL COMMENT 'Exercises 64-bit values beyond JS Number.MAX_SAFE_INTEGER',
  uid         char(36) NOT NULL DEFAULT (uuid()),
  settings    json NULL COMMENT 'Free-form per-user preferences',
  tags        set('a', 'b', 'c', 'd') NULL,
  avatar      blob NULL,
  bio         text NULL,
  score       double NULL,
  created_at  datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  last_login  timestamp NULL DEFAULT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COMMENT='Application accounts (dev seed)';

-- ---------------------------------------------------------------------------
-- posts (+ updated_at trigger)
-- ---------------------------------------------------------------------------
CREATE TABLE posts (
  id            bigint NOT NULL AUTO_INCREMENT PRIMARY KEY,
  user_id       bigint NOT NULL,
  title         text NOT NULL,
  body          text NULL,
  status        enum('draft', 'published', 'archived') NOT NULL DEFAULT 'draft',
  published_at  datetime(6) NULL,
  views         int NOT NULL DEFAULT 0,
  created_at    datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  updated_at    datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  KEY posts_user_id_idx (user_id),
  KEY posts_status_published_at_idx (status, published_at),
  CONSTRAINT posts_user_id_fkey FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

-- Single-statement body, so no DELIMITER juggling is needed.
CREATE TRIGGER posts_set_updated_at
  BEFORE UPDATE ON posts
  FOR EACH ROW SET NEW.updated_at = CURRENT_TIMESTAMP(6);

-- ---------------------------------------------------------------------------
-- tags / post_tags
-- ---------------------------------------------------------------------------
CREATE TABLE tags (
  id    int NOT NULL AUTO_INCREMENT PRIMARY KEY,
  name  varchar(100) NOT NULL UNIQUE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE post_tags (
  post_id  bigint NOT NULL,
  tag_id   int NOT NULL,
  PRIMARY KEY (post_id, tag_id),
  CONSTRAINT post_tags_post_id_fkey FOREIGN KEY (post_id) REFERENCES posts (id) ON DELETE CASCADE,
  CONSTRAINT post_tags_tag_id_fkey  FOREIGN KEY (tag_id)  REFERENCES tags (id)  ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

-- ---------------------------------------------------------------------------
-- audit_log: deliberately has NO primary key
-- ---------------------------------------------------------------------------
CREATE TABLE audit_log (
  at       datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  actor    varchar(100) NULL,
  action   varchar(100) NOT NULL,
  payload  json NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

-- ---------------------------------------------------------------------------
-- tabler_inv: second database (the MySQL counterpart of the PG `inventory` schema)
-- ---------------------------------------------------------------------------
CREATE DATABASE IF NOT EXISTS tabler_inv CHARACTER SET utf8mb4;

DROP TABLE IF EXISTS tabler_inv.stock_movements;
DROP TABLE IF EXISTS tabler_inv.items;

CREATE TABLE tabler_inv.items (
  id     int NOT NULL AUTO_INCREMENT PRIMARY KEY,
  sku    varchar(32) NOT NULL UNIQUE,
  name   text NOT NULL,
  stock  int NOT NULL DEFAULT 0
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE tabler_inv.stock_movements (
  id       bigint NOT NULL AUTO_INCREMENT PRIMARY KEY,
  item_id  int NOT NULL,
  delta    int NOT NULL,
  at       datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  CONSTRAINT stock_movements_item_id_fkey FOREIGN KEY (item_id) REFERENCES tabler_inv.items (id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

-- ---------------------------------------------------------------------------
-- Views
-- ---------------------------------------------------------------------------
CREATE VIEW active_users AS
  SELECT * FROM users WHERE is_active;

CREATE VIEW post_counts AS
  SELECT u.id AS user_id, u.name, COUNT(p.id) AS post_count
  FROM users u
  LEFT JOIN posts p ON p.user_id = u.id
  GROUP BY u.id, u.name;
