//! MongoDB against a real server: the mongodb lane's suite, which replaces this stub
//! (docs/drivers/mongodb.md). Built only with `drv-mongodb` (`required-features` in Cargo.toml) and
//! run with `node dev/scripts/lane.mjs mongodb it`, which loads `.env`. Each test skips itself
//! while its variable is unset, and is `#[ignore]`d so that a plain `cargo test --all-features`
//! never reaches a server (a cloud one bills the queries).

#![cfg(feature = "drv-mongodb")]

mod common;

#[test]
#[ignore = "set TABLER_TEST_MONGODB_URL"]
fn real_server() {
    let Some(_url) = common::url("TABLER_TEST_MONGODB_URL") else {
        return;
    };
    // The lane's suite: connect, list, browse with paging, count, fetch, a commit with one
    // failing change, a console run, a cancel and a timeout (docs/drivers/README.md).
}
