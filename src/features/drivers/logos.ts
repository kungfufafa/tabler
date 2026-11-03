import type { DatabaseKind } from "@/lib/ipc";
import postgres from "@/assets/databases/postgres.svg";
import cockroachdb from "@/assets/databases/cockroachdb.svg";
import greenplum from "@/assets/databases/greenplum.png";
import redshift from "@/assets/databases/redshift.svg";
import mysql from "@/assets/databases/mysql.svg";
import mariadb from "@/assets/databases/mariadb.svg";
import sqlite from "@/assets/databases/sqlite.svg";
import libsql from "@/assets/databases/libsql.svg";
import d1 from "@/assets/databases/d1.svg";
import duckdb from "@/assets/databases/duckdb.svg";
import sqlserver from "@/assets/databases/sqlserver.svg";
import clickhouse from "@/assets/databases/clickhouse.svg";
import snowflake from "@/assets/databases/snowflake.svg";
import bigquery from "@/assets/databases/bigquery.svg";
import vertica from "@/assets/databases/vertica.png";
import cassandra from "@/assets/databases/cassandra.svg";
import redis from "@/assets/databases/redis.svg";
import mongodb from "@/assets/databases/mongodb.svg";
import elasticsearch from "@/assets/databases/elasticsearch.svg";
import dynamodb from "@/assets/databases/dynamodb.png";

/** Bundled logos: no network requests when rendering connection badges. */
export const DATABASE_LOGOS = {
  postgres,
  cockroachdb,
  greenplum,
  redshift,
  mysql,
  mariadb,
  sqlite,
  libsql,
  d1,
  duckdb,
  sqlserver,
  clickhouse,
  snowflake,
  bigquery,
  vertica,
  cassandra,
  redis,
  mongodb,
  elasticsearch,
  dynamodb,
} satisfies Record<DatabaseKind, string>;
