#!/bin/sh
# dev/scripts/build-sqlite.sh -- rebuild the SQLite fixture from dev/seed/sqlite/seed.sql.
#
#   sh dev/scripts/build-sqlite.sh        # (pnpm dev:sqlite:build)
#
# Output path comes from TABLER_TEST_SQLITE_PATH (environment, then .env, then .env.example,
# default dev/fixtures/sample.sqlite; relative paths are resolved from the repo root).
# The file is deleted and rebuilt from scratch every time.
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
SEED=$ROOT/dev/seed/sqlite/seed.sql

log()  { printf '\033[1;34m[build-sqlite]\033[0m %s\n' "$*"; }
fail() { printf '\033[1;31m[build-sqlite] error:\033[0m %s\n' "$*" >&2; exit 1; }

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

command -v sqlite3 >/dev/null 2>&1 || fail "sqlite3 not found in PATH"
[ -f "$SEED" ] || fail "seed file missing: $SEED"

OUT=${TABLER_TEST_SQLITE_PATH:-$(env_lookup TABLER_TEST_SQLITE_PATH)}
OUT=${OUT:-dev/fixtures/sample.sqlite}
case $OUT in
  /*) ;;
  *) OUT=$ROOT/$OUT ;;
esac

mkdir -p "$(dirname "$OUT")"
log "removing ${OUT#"$ROOT"/} (and any -journal/-wal/-shm files)"
rm -f "$OUT" "$OUT-journal" "$OUT-wal" "$OUT-shm"

log "building from ${SEED#"$ROOT"/}"
sqlite3 -bail "$OUT" < "$SEED"

log "row counts"
sqlite3 -header -column "$OUT" "
  SELECT 'users' AS object, count(*) AS rows FROM users
  UNION ALL SELECT 'posts', count(*) FROM posts
  UNION ALL SELECT 'tags', count(*) FROM tags
  UNION ALL SELECT 'post_tags', count(*) FROM post_tags
  UNION ALL SELECT 'audit_log', count(*) FROM audit_log
  UNION ALL SELECT 'events_big', count(*) FROM events_big
  UNION ALL SELECT 'types_zoo', count(*) FROM types_zoo
  UNION ALL SELECT 'types_zoo_affinity', count(*) FROM types_zoo_affinity
  UNION ALL SELECT '\"Order Items\"', count(*) FROM \"Order Items\"
  UNION ALL SELECT '\"CamelCase\"', count(*) FROM \"CamelCase\"
  UNION ALL SELECT 'active_users (view)', count(*) FROM active_users
  UNION ALL SELECT 'post_counts (view)', count(*) FROM post_counts;"

size=$(wc -c < "$OUT" | tr -d ' ')
log "wrote ${OUT#"$ROOT"/} ($((size / 1024)) KiB)"
