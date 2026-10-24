//! CockroachDB, Greenplum and Amazon Redshift against a real server: the pg-flavors lane's suite,
//! which replaces this stub (docs/drivers/pg-flavors.md). Built only with `drv-pg-flavors`
//! (`required-features` in Cargo.toml) and run with `node dev/scripts/lane.mjs pg-flavors it`,
//! which loads `.env`. Each test skips itself while its variable is unset, and is `#[ignore]`d so
//! that a plain `cargo test --all-features` never reaches a server (a cloud one bills the queries).

#![cfg(feature = "drv-pg-flavors")]

mod common;

#[test]
#[ignore = "set TABLER_TEST_CRDB_URL"]
fn cockroachdb_server() {
    let Some(_url) = common::url("TABLER_TEST_CRDB_URL") else {
        return;
    };
    // The lane's suite: connect, list, browse with paging, count, fetch, a commit with one
    // failing change, a console run, a cancel and a timeout (docs/drivers/README.md).
}

#[test]
#[ignore = "set TABLER_TEST_GREENPLUM_URL"]
fn greenplum_server() {
    let Some(_url) = common::url("TABLER_TEST_GREENPLUM_URL") else {
        return;
    };
    // The lane's suite: connect, list, browse with paging, count, fetch, a commit with one
    // failing change, a console run, a cancel and a timeout (docs/drivers/README.md).
}

#[test]
#[ignore = "set TABLER_TEST_REDSHIFT_URL"]
fn redshift_server() {
    let Some(_url) = common::url("TABLER_TEST_REDSHIFT_URL") else {
        return;
    };
    // The lane's suite: connect, list, browse with paging, count, fetch, a commit with one
    // failing change, a console run, a cancel and a timeout (docs/drivers/README.md).
}
