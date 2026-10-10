#!/bin/sh
# dev/scripts/db-seed.sh -- (re)seed the LOCAL development databases.
#
#   sh dev/scripts/db-seed.sh           # Postgres + MySQL   (pnpm dev:db:seed)
#   sh dev/scripts/db-seed.sh pg        # Postgres only
#   sh dev/scripts/db-seed.sh mysql     # MySQL only
#
# Connection URLs come from TABLER_TEST_PG_URL / TABLER_TEST_MYSQL_URL, looked up in this
# order: environment, .env, .env.example, built-in default. The database named in each URL
# must start with "tabler_" (normally tabler_dev): it is DROPPED, recreated, and then every
# dev/seed/<engine>/*.sql file is applied in order. MySQL additionally gets tabler_inv.
# Idempotent -- run it as often as you like. POSIX sh + sed/awk only.
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
SEED_DIR=$ROOT/dev/seed

# ---------------------------------------------------------------------------
# helpers
# ---------------------------------------------------------------------------
log()  { printf '\033[1;34m[db-seed]\033[0m %s\n' "$*"; }
fail() { printf '\033[1;31m[db-seed] error:\033[0m %s\n' "$*" >&2; exit 1; }

# env_lookup KEY -> prints the value of KEY from .env, else .env.example, else nothing.
# Only KEY=VALUE lines are read (nothing is sourced/executed); surrounding quotes are stripped.
env_lookup() {
  for f in "$ROOT/.env" "$ROOT/.env.example"; do
    [ -f "$f" ] || continue
    v=$(sed -n "s/^[[:space:]]*$1=//p" "$f" | tail -n 1 | sed -e 's/[[:space:]]*$//' -e 's/^"\(.*\)"$/\1/' -e "s/^'\(.*\)'$/\1/")
    if [ -n "$v" ]; then
      printf '%s' "$v"
      return 0
    fi
  done
  return 0
}

# urldecode STRING -> percent-decoded STRING (for passwords such as p%40ss)
urldecode() {
  printf '%s' "$1" | awk '
    BEGIN { for (i = 0; i < 256; i++) h[sprintf("%02X", i)] = sprintf("%c", i) }
    {
      s = $0; o = ""
      while (match(s, /%[0-9A-Fa-f][0-9A-Fa-f]/)) {
        o = o substr(s, 1, RSTART - 1) h[toupper(substr(s, RSTART + 1, 2))]
        s = substr(s, RSTART + 3)
      }
      printf "%s%s", o, s
    }'
}

# parse_url URL -> sets U_SCHEME U_USER U_PASS U_HOST U_PORT U_DB
#   scheme://[user[:password]@][host][:port][/dbname][?params]
parse_url() {
  _u=$1
  case $_u in
    *://*) ;;
    *) fail "not a URL: $_u" ;;
  esac
  U_SCHEME=${_u%%://*}
  _r=${_u#*://}
  _r=${_r%%\?*}                       # drop ?query
  case $_r in
    *@*) _auth=${_r%@*}; _hp=${_r##*@} ;;   # split on the LAST '@' (passwords may contain one)
    *)   _auth=''; _hp=$_r ;;
  esac
  case $_auth in
    *:*) U_USER=${_auth%%:*}; U_PASS=$(urldecode "${_auth#*:}") ;;
    *)   U_USER=$_auth; U_PASS='' ;;
  esac
  case $_hp in
    */*) _host=${_hp%%/*}; U_DB=${_hp#*/} ;;
    *)   _host=$_hp; U_DB='' ;;
  esac
  case $_host in
    \[*\]*) U_HOST=${_host%%]*}; U_HOST=${U_HOST#\[}; _p=${_host#*]}; U_PORT=${_p#:} ;;   # [::1]:5432
    *:*)    U_HOST=${_host%%:*}; U_PORT=${_host##*:} ;;
    *)      U_HOST=$_host; U_PORT='' ;;
  esac
}

require_managed_db() {
  case $1 in
    tabler_*) ;;
    *) fail "refusing to drop database '$1': this script only manages databases named tabler_* (edit the URL in .env)" ;;
  esac
}

# ---------------------------------------------------------------------------
# PostgreSQL
# ---------------------------------------------------------------------------
seed_postgres() {
  command -v psql >/dev/null 2>&1    || fail "psql not found in PATH"
  command -v createdb >/dev/null 2>&1 || fail "createdb not found in PATH"

  parse_url "$PG_URL"
  db=${U_DB:-tabler_dev}
  require_managed_db "$db"

  set --
  [ -n "$U_HOST" ] && set -- "$@" -h "$U_HOST"
  [ -n "$U_PORT" ] && set -- "$@" -p "$U_PORT"
  [ -n "$U_USER" ] && set -- "$@" -U "$U_USER"
  if [ -n "$U_PASS" ]; then PGPASSWORD=$U_PASS; export PGPASSWORD; fi
  PGCLIENTENCODING=UTF8; export PGCLIENTENCODING

  log "postgres: ${U_HOST:-local socket}:${U_PORT:-5432} as ${U_USER:-$(id -un)} -> drop + create '$db'"
  dropdb --if-exists --force "$@" "$db"
  createdb "$@" -E UTF8 "$db"

  for f in "$SEED_DIR"/postgres/*.sql; do
    log "postgres: applying ${f#"$ROOT"/}"
    psql -X -q -v ON_ERROR_STOP=1 "$@" -d "$db" -f "$f"
  done

  log "postgres: row counts in '$db'"
  psql -X "$@" -d "$db" -c "
    SELECT 'users' AS object, count(*) AS rows FROM users
    UNION ALL SELECT 'posts', count(*) FROM posts
    UNION ALL SELECT 'tags', count(*) FROM tags
    UNION ALL SELECT 'post_tags', count(*) FROM post_tags
    UNION ALL SELECT 'audit_log', count(*) FROM audit_log
    UNION ALL SELECT 'events_big', count(*) FROM events_big
    UNION ALL SELECT 'types_zoo', count(*) FROM types_zoo
    UNION ALL SELECT '\"Order Items\"', count(*) FROM \"Order Items\"
    UNION ALL SELECT '\"CamelCase\"', count(*) FROM \"CamelCase\"
    UNION ALL SELECT 'inventory.items', count(*) FROM inventory.items
    UNION ALL SELECT 'inventory.stock_movements', count(*) FROM inventory.stock_movements
    UNION ALL SELECT 'active_users (view)', count(*) FROM active_users
    UNION ALL SELECT 'post_counts (view)', count(*) FROM post_counts
    UNION ALL SELECT 'post_counts_mv (matview)', count(*) FROM post_counts_mv;"
  unset PGPASSWORD
}

# ---------------------------------------------------------------------------
# MySQL
# ---------------------------------------------------------------------------
seed_mysql() {
  command -v mysql >/dev/null 2>&1 || fail "mysql client not found in PATH"

  parse_url "$MYSQL_URL"
  db=${U_DB:-tabler_dev}
  require_managed_db "$db"
  inv_db=tabler_inv

  set --
  [ -n "$U_HOST" ] && set -- "$@" -h "$U_HOST"
  [ -n "$U_PORT" ] && set -- "$@" -P "$U_PORT"
  [ -n "$U_USER" ] && set -- "$@" -u "$U_USER"
  [ -n "$U_PASS" ] && set -- "$@" "-p$U_PASS"
  set -- "$@" --default-character-set=utf8mb4

  log "mysql: ${U_HOST:-localhost}:${U_PORT:-3306} as ${U_USER:-root} -> drop + create '$db' and '$inv_db'"
  mysql "$@" -e "
    DROP DATABASE IF EXISTS \`$db\`;
    DROP DATABASE IF EXISTS \`$inv_db\`;
    CREATE DATABASE \`$db\` CHARACTER SET utf8mb4;
    CREATE DATABASE \`$inv_db\` CHARACTER SET utf8mb4;"

  for f in "$SEED_DIR"/mysql/*.sql; do
    log "mysql: applying ${f#"$ROOT"/}"
    mysql "$@" "$db" < "$f"
  done

  log "mysql: row counts in '$db' / '$inv_db'"
  mysql "$@" -t "$db" -e "
    SELECT 'users' AS object, count(*) AS \`rows\` FROM users
    UNION ALL SELECT 'posts', count(*) FROM posts
    UNION ALL SELECT 'tags', count(*) FROM tags
    UNION ALL SELECT 'post_tags', count(*) FROM post_tags
    UNION ALL SELECT 'audit_log', count(*) FROM audit_log
    UNION ALL SELECT 'events_big', count(*) FROM events_big
    UNION ALL SELECT 'types_zoo', count(*) FROM types_zoo
    UNION ALL SELECT '\`Order Items\`', count(*) FROM \`Order Items\`
    UNION ALL SELECT '\`CamelCase\`', count(*) FROM \`CamelCase\`
    UNION ALL SELECT '$inv_db.items', count(*) FROM \`$inv_db\`.items
    UNION ALL SELECT '$inv_db.stock_movements', count(*) FROM \`$inv_db\`.stock_movements
    UNION ALL SELECT 'active_users (view)', count(*) FROM active_users
    UNION ALL SELECT 'post_counts (view)', count(*) FROM post_counts;"
}

# ---------------------------------------------------------------------------
# main
# ---------------------------------------------------------------------------
PG_URL=${TABLER_TEST_PG_URL:-$(env_lookup TABLER_TEST_PG_URL)}
PG_URL=${PG_URL:-postgres://$(id -un)@127.0.0.1:5432/tabler_dev}
MYSQL_URL=${TABLER_TEST_MYSQL_URL:-$(env_lookup TABLER_TEST_MYSQL_URL)}
MYSQL_URL=${MYSQL_URL:-mysql://root@127.0.0.1:3306/tabler_dev}

do_pg=1; do_mysql=1
if [ $# -gt 0 ]; then
  do_pg=0; do_mysql=0
  for a in "$@"; do
    case $a in
      pg|postgres|postgresql) do_pg=1 ;;
      mysql|mariadb)          do_mysql=1 ;;
      all)                    do_pg=1; do_mysql=1 ;;
      -h|--help) sed -n '2,12p' "$0"; exit 0 ;;
      *) fail "unknown argument '$a' (expected: pg | mysql | all)" ;;
    esac
  done
fi

[ "$do_pg" -eq 1 ]    && seed_postgres
[ "$do_mysql" -eq 1 ] && seed_mysql
log "done"
