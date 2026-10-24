//! The rustls crypto provider for tests.

use std::sync::Once;

/// Installs aws-lc-rs as rustls's process-wide provider, as `run()` does at startup. A
/// `ClientConfig::builder()` without an installed provider panics once a dependency enables
/// `rustls/ring` as well (rustls can no longer pick one by itself), so every test that builds a
/// TLS client calls this first. Only the first call does anything.
pub fn install_crypto() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // A test that installs it on its own may have come first: that is the same provider.
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installs_aws_lc_once() {
        install_crypto();
        install_crypto();
        let installed = rustls::crypto::CryptoProvider::get_default().expect("a provider");
        let groups = |p: &rustls::crypto::CryptoProvider| {
            p.kx_groups.iter().map(|g| g.name()).collect::<Vec<_>>()
        };
        // aws-lc-rs offers the post-quantum hybrid group, which ring does not.
        assert_eq!(
            groups(installed),
            groups(&rustls::crypto::aws_lc_rs::default_provider())
        );
    }
}
