#!/bin/sh
# dev/scripts/build-duckdb.sh -- rebuild the DuckDB fixture from src-tauri/tests/fixtures/duckdb/sample.sql.
#
#   sh dev/scripts/build-duckdb.sh        # (pnpm dev:duckdb:build)
#
# The file is written by the app's own DuckDB: the DuckDB lane's ignored test
# drivers::duckdb::testing::write_sample_file, run through `lane.mjs duckdb test` (cargo with
# `drv-duckdb` in a lane slot). Nothing is downloaded once the bundled `duckdb` crate is in the
# cargo cache, but a cold build compiles DuckDB's C++ for several minutes.
#
# Output path comes from TABLER_TEST_DUCKDB_PATH (environment, then .env, then .env.example,
# default dev/fixtures/sample.duckdb; relative paths are resolved from the repo root).
# The file is deleted and rebuilt from scratch every time.
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
SQL=$ROOT/src-tauri/tests/fixtures/duckdb/sample.sql

log()  { printf '\033[1;34m[build-duckdb]\033[0m %s\n' "$*"; }
fail() { printf '\033[1;31m[build-duckdb] error:\033[0m %s\n' "$*" >&2; exit 1; }

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

command -v node >/dev/null 2>&1 || fail "node not found in PATH"
command -v cargo >/dev/null 2>&1 || fail "cargo not found in PATH"
[ -f "$SQL" ] || fail "sample SQL missing: $SQL"

OUT=${TABLER_TEST_DUCKDB_PATH:-$(env_lookup TABLER_TEST_DUCKDB_PATH)}
OUT=${OUT:-dev/fixtures/sample.duckdb}
case $OUT in
  /* | [A-Za-z]:*) ;;
  *) OUT=$ROOT/$OUT ;;
esac
# The test binary is a native program: on Windows (Git Bash) it needs C:/… rather than /c/….
NATIVE=$OUT
if command -v cygpath >/dev/null 2>&1; then
  NATIVE=$(cygpath -m "$OUT")
fi

mkdir -p "$(dirname "$OUT")"
log "removing ${OUT#"$ROOT"/} (and its .wal)"
rm -f "$OUT" "$OUT.wal"

log "building from ${SQL#"$ROOT"/}"
TABLER_DUCKDB_SAMPLE_OUT=$NATIVE node "$ROOT/dev/scripts/lane.mjs" duckdb test \
  drivers::duckdb::testing::write_sample_file -- --ignored --exact

[ -f "$OUT" ] || fail "the test wrote no ${OUT#"$ROOT"/}"
size=$(wc -c < "$OUT" | tr -d ' ')
log "wrote ${OUT#"$ROOT"/} ($((size / 1024)) KiB)"
