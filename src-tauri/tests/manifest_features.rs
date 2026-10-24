//! Guards of Cargo features the app's behaviour depends on (nothing at run time shows a missing
//! one until a user on the wrong network reports it), and of the driver crates' build choices.

fn manifest() -> String {
    std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")).expect("Cargo.toml")
}

/// The manifest line of dependency `name` (`name = ...`).
fn dependency(name: &str) -> String {
    manifest()
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with(&format!("{name} =")))
        .unwrap_or_else(|| panic!("no {name} dependency in Cargo.toml"))
        .to_string()
}

/// The features a dependency line turns on (`features = [...]`).
fn features(line: &str) -> Vec<String> {
    line.split_once("features = [")
        .and_then(|(_, rest)| rest.split_once(']'))
        .map(|(list, _)| {
            list.split(',')
                .map(|f| f.trim().trim_matches('"').to_string())
                .filter(|f| !f.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// `reqwest` is built without its default features, which leaves out `system-proxy`: a Finder or
/// Dock launched app has no `HTTPS_PROXY` in its environment, so without the feature the AI
/// Assistant ignored the proxy set in macOS System Settings.
#[test]
fn the_http_client_reads_the_system_proxy() {
    let reqwest = dependency("reqwest");
    assert!(
        reqwest.contains("\"system-proxy\""),
        "reqwest needs the system-proxy feature: {reqwest}"
    );
}

/// The crates only the drivers beyond PostgreSQL, MySQL and SQLite use.
const DRIVER_CRATES: [&str; 13] = [
    "tiberius",
    "duckdb",
    "snowflake-connector-rs",
    "scylla",
    "num-bigint",
    "bigdecimal",
    "redis",
    "mongodb",
    "tokio-rustls",
    "md-5",
    "bytes",
    "percent-encoding",
    "hex",
];

/// A default build compiles none of them: each one comes in with a `drv-*` or `common-*` feature,
/// so a driver that is not built costs nothing (DuckDB alone is minutes of C++).
#[test]
fn driver_crates_are_optional() {
    for name in DRIVER_CRATES {
        let line = dependency(name);
        assert!(
            line.contains("optional = true"),
            "{name} must be optional: {line}"
        );
    }
}

/// TLS is rustls on aws-lc-rs in every driver: Windows has no system OpenSSL, native-tls brings
/// SChannel's certificate rules next to rustls's, and ring would be a second crypto library.
#[test]
fn driver_crates_bring_no_other_tls_stack() {
    for name in DRIVER_CRATES {
        let line = dependency(name);
        for feature in features(&line) {
            let words: Vec<&str> = feature.split(['-', '_']).collect();
            let other = words.contains(&"openssl")
                || words.contains(&"ring")
                || feature.contains("native-tls")
                // mongodb's name for rustls on ring
                || (name == "mongodb" && feature == "rustls-tls");
            assert!(!other, "{name} turns on {feature}: {line}");
        }
    }
}

/// tiberius's default features include native-tls, so they stay off and rustls is chosen.
#[test]
fn sql_server_uses_rustls_without_native_tls_or_openssl() {
    let tiberius = dependency("tiberius");
    assert!(tiberius.contains("default-features = false"), "{tiberius}");
    let features = features(&tiberius);
    assert!(features.iter().any(|f| f == "rustls"), "{tiberius}");
    for bad in ["native-tls", "vendored-openssl"] {
        assert!(!features.iter().any(|f| f == bad), "{tiberius}");
    }
}

/// scylla's `openssl-010` would need an OpenSSL build, which Windows lacks.
#[test]
fn cassandra_uses_rustls_without_openssl() {
    let scylla = dependency("scylla");
    let features = features(&scylla);
    assert!(features.iter().any(|f| f == "rustls-023"), "{scylla}");
    assert!(!features.iter().any(|f| f == "openssl-010"), "{scylla}");
}

/// mongodb's default `rustls-tls` builds rustls on ring. redis's defaults add helpers the driver
/// does not use, and its `*native-tls*` features would bring SChannel: the driver wraps
/// tokio-rustls itself.
#[test]
fn redis_and_mongodb_leave_their_default_features_off() {
    let mongodb = dependency("mongodb");
    assert!(mongodb.contains("default-features = false"), "{mongodb}");
    let mongo_features = features(&mongodb);
    assert!(
        mongo_features.iter().any(|f| f == "rustls-tls-aws-lc"),
        "{mongodb}"
    );
    assert!(
        !mongo_features.iter().any(|f| f == "rustls-tls"),
        "{mongodb}"
    );

    let redis = dependency("redis");
    assert!(redis.contains("default-features = false"), "{redis}");
    assert!(
        !features(&redis).iter().any(|f| f.contains("native-tls")),
        "{redis}"
    );
}

/// Every further DuckDB feature compiles another C++ extension (or switches to a cmake build) on
/// top of a build that already takes minutes.
#[test]
fn duckdb_builds_the_bundled_engine_with_json_and_parquet_only() {
    let duckdb = dependency("duckdb");
    assert!(duckdb.contains("default-features = false"), "{duckdb}");
    let mut features = features(&duckdb);
    features.sort();
    assert_eq!(features, ["bundled", "json", "parquet"], "{duckdb}");
}

/// With debug info, DuckDB's C++ objects make the debug test binaries too big to link in 16 GB.
#[test]
fn duckdb_is_compiled_without_debug_info_in_dev_builds() {
    let manifest = manifest();
    let (_, section) = manifest
        .split_once("[profile.dev.package.libduckdb-sys]")
        .expect("a [profile.dev.package.libduckdb-sys] section");
    let settings: Vec<&str> = section
        .lines()
        .map(str::trim)
        .skip_while(|l| l.is_empty())
        .take_while(|l| !l.is_empty() && !l.starts_with('['))
        .collect();
    assert!(settings.contains(&"debug = false"), "{settings:?}");
}
