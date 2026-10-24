//! DuckDB against a real database file: the duckdb lane's suite, which replaces this stub
//! (docs/drivers/duckdb.md). Built only with `drv-duckdb` (`required-features` in Cargo.toml) and
//! run with `node dev/scripts/lane.mjs duckdb it`, which loads `.env`. Each test skips itself while
//! its variable is unset, and is `#[ignore]`d so that a plain `cargo test --all-features` leaves
//! the file alone.

#![cfg(feature = "drv-duckdb")]

mod common;

#[test]
#[ignore = "set TABLER_TEST_DUCKDB_PATH"]
fn real_file() {
    let Some(_path) = common::url("TABLER_TEST_DUCKDB_PATH") else {
        return;
    };
    // The lane's suite: connect, list, browse with paging, count, fetch, a commit with one
    // failing change, a console run, a cancel and a timeout (docs/drivers/README.md).
}
