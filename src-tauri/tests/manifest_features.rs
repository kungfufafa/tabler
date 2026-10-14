//! Guards of Cargo features the app's behaviour depends on (nothing at run time shows a missing
//! one until a user on the wrong network reports it).

/// The manifest line of dependency `name` (`name = ...`).
fn dependency(name: &str) -> String {
    let manifest = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .expect("Cargo.toml");
    manifest
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with(&format!("{name} =")))
        .unwrap_or_else(|| panic!("no {name} dependency in Cargo.toml"))
        .to_string()
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
