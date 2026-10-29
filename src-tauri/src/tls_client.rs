//! The rustls client configs of the drivers beyond PostgreSQL, MySQL and SQLite: the HTTP drivers
//! (`drivers::common::http`) and those that run TLS over a socket of their own (Vertica after its
//! TLS request, Redis, Cassandra). The modes mean what they mean for PostgreSQL, which keeps its
//! own copy in `drivers/postgres/connect.rs`:
//! - `disable`, `prefer`, `require`: encryption without checking who the server is;
//! - `verify-ca`: the chain must lead to a trusted root, whatever name the certificate holds;
//! - `verify-full`: the chain and the server's name.
//!
//! Trusted roots are the operating system's plus the profile's CA file. A SHA-256 pin (an
//! Elasticsearch CA fingerprint) is checked on top of any mode: one of the certificates the server
//! presents must have that fingerprint, and the server's own certificate must be it or be signed
//! through it, so a pinned CA cannot vouch for a stranger's certificate.

use std::net::IpAddr;
use std::sync::Arc;

use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::pem::PemObject as _;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::server::ParsedCertificate;
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};

use crate::error::AppError;
use crate::tls_files::ClientCertificate;
use crate::types::profile::SslMode;

/// The client config for `mode`, trusting the system's roots plus the certificates of the PEM
/// file `ca` (the profile's CA file), with a client certificate when the server wants one, the
/// ALPN protocols to offer and an optional SHA-256 pin. The server's name is the one the caller
/// gives rustls (or tokio-rustls) when it connects: the profile's host, also through a tunnel
/// (`ConnectTarget::tls_host`).
pub fn rustls_config(
    mode: SslMode,
    ca: Option<&str>,
    identity: Option<ClientCertificate>,
    alpn: &[&[u8]],
    pin_sha256: Option<[u8; 32]>,
) -> Result<ClientConfig, AppError> {
    rustls_config_through(mode, ca, identity, alpn, pin_sha256, None)
}

/// [`rustls_config`] for a socket that reaches the server through a tunnel listening on `local`,
/// when the server is an IP address: the connector names the tunnel's address, and the
/// certificate is checked for `server` instead (only for that name; other hosts as named).
pub(crate) fn rustls_config_through(
    mode: SslMode,
    ca: Option<&str>,
    identity: Option<ClientCertificate>,
    alpn: &[&[u8]],
    pin_sha256: Option<[u8; 32]>,
    redirect: Option<(IpAddr, ServerName<'static>)>,
) -> Result<ClientConfig, AppError> {
    let provider = provider();
    let check = match mode {
        SslMode::Disable | SslMode::Prefer | SslMode::Require => Check::Nothing,
        SslMode::VerifyCa => Check::Chain(Arc::new(root_store(ca)?)),
        SslMode::VerifyFull => Check::Full(
            WebPkiServerVerifier::builder_with_provider(
                Arc::new(root_store(ca)?),
                provider.clone(),
            )
            .build()
            .map_err(|e| AppError::Connect(format!("tls: {e}")))?,
        ),
    };
    let verifier = Verifier {
        check,
        pin: pin_sha256,
        redirect: redirect.map(|(ip, server)| (ServerName::IpAddress(ip.into()), server)),
        provider: provider.clone(),
    };
    let builder = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| AppError::Connect(format!("tls: {e}")))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier));
    let mut config = match identity {
        None => builder.with_no_client_auth(),
        Some(ClientCertificate { certs, key }) => builder
            .with_client_auth_cert(certs, key)
            .map_err(|e| AppError::Connect(format!("SSL client certificate: {e}")))?,
    };
    config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
    Ok(config)
}

/// The name rustls checks and sends (SNI) for `host`: a DNS name or an IP address (`[::1]` in a
/// URL's brackets too).
pub fn server_name(host: &str) -> Result<ServerName<'static>, AppError> {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    ServerName::try_from(bare.to_string())
        .map_err(|_| AppError::InvalidInput(format!("{host} is not a valid server name for TLS")))
}

/// SHA-256 of a DER certificate (what a pin holds).
pub fn sha256(der: &[u8]) -> [u8; 32] {
    let digest = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, der);
    let mut out = [0u8; 32];
    out.copy_from_slice(digest.as_ref());
    out
}

/// A SHA-256 fingerprint as people copy it: 64 hex digits, with or without `:` or spaces between
/// the bytes (`openssl x509 -fingerprint -sha256`, Elasticsearch's `ca_trusted_fingerprint`).
pub fn parse_pin(text: &str) -> Result<[u8; 32], AppError> {
    let digits: String = text
        .chars()
        .filter(|c| !matches!(c, ':' | ' ' | '\t'))
        .collect();
    let bad = || {
        AppError::InvalidInput(format!(
            "the certificate fingerprint must be a SHA-256 in hex (64 digits): {}",
            text.trim()
        ))
    };
    if digits.len() != 64 || !digits.is_ascii() {
        return Err(bad());
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&digits[2 * i..2 * i + 2], 16).map_err(|_| bad())?;
    }
    Ok(out)
}

/// The process-wide crypto provider (installed by `lib.rs::run` / the tests).
fn provider() -> Arc<CryptoProvider> {
    CryptoProvider::get_default()
        .cloned()
        .unwrap_or_else(|| Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
}

/// The system's roots plus those of the PEM file `ca`.
fn root_store(ca: Option<&str>) -> Result<RootCertStore, AppError> {
    let mut roots = RootCertStore::empty();
    roots.add_parsable_certificates(crate::tls_files::native_roots());
    if let Some(path) = ca.map(str::trim).filter(|p| !p.is_empty()) {
        let pem = std::fs::read(path)
            .map_err(|e| AppError::Connect(format!("cannot read CA file {path}: {e}")))?;
        let mut added = 0usize;
        for cert in CertificateDer::pem_slice_iter(&pem) {
            let cert =
                cert.map_err(|e| AppError::Connect(format!("invalid PEM in {path}: {e:?}")))?;
            roots
                .add(cert)
                .map_err(|e| AppError::Connect(format!("invalid certificate in {path}: {e}")))?;
            added += 1;
        }
        if added == 0 {
            return Err(AppError::Connect(format!(
                "no certificates found in CA file {path}"
            )));
        }
    }
    if roots.is_empty() {
        return Err(AppError::Connect(
            "no trusted root certificates available".into(),
        ));
    }
    Ok(roots)
}

/// What is checked of the server's chain.
#[derive(Debug)]
enum Check {
    Nothing,
    /// The chain leads to one of these roots (`verify-ca`).
    Chain(Arc<RootCertStore>),
    /// The chain and the name (`verify-full`).
    Full(Arc<WebPkiServerVerifier>),
}

#[derive(Debug)]
struct Verifier {
    check: Check,
    pin: Option<[u8; 32]>,
    /// The name a connector gives (a tunnel's address) and the name to check instead.
    redirect: Option<(ServerName<'static>, ServerName<'static>)>,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for Verifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let name = match &self.redirect {
            Some((local, server)) if local == server_name => server,
            _ => server_name,
        };
        match &self.check {
            Check::Nothing => {}
            Check::Chain(roots) => {
                let cert = ParsedCertificate::try_from(end_entity)?;
                rustls::client::verify_server_cert_signed_by_trust_anchor(
                    &cert,
                    roots,
                    intermediates,
                    now,
                    self.provider.signature_verification_algorithms.all,
                )?;
            }
            Check::Full(webpki) => {
                webpki.verify_server_cert(end_entity, intermediates, name, ocsp_response, now)?;
            }
        }
        if let Some(pin) = &self.pin {
            check_pin(pin, end_entity, intermediates, now, &self.provider)?;
        }
        Ok(ServerCertVerified::assertion())
    }

    // The handshake signatures are always checked: with a pin, they prove the server holds the
    // key of the certificate it showed.
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// The server's certificate is the pinned one, or a certificate it presented is and signed it
/// (directly or through the others it presented).
fn check_pin(
    pin: &[u8; 32],
    end_entity: &CertificateDer<'_>,
    intermediates: &[CertificateDer<'_>],
    now: UnixTime,
    provider: &CryptoProvider,
) -> Result<(), rustls::Error> {
    if sha256(end_entity) == *pin {
        return Ok(());
    }
    let mismatch = || {
        rustls::Error::General(
            "the server's certificate does not match the pinned SHA-256 fingerprint".into(),
        )
    };
    let at = intermediates
        .iter()
        .position(|c| sha256(c) == *pin)
        .ok_or_else(mismatch)?;
    let mut anchor = RootCertStore::empty();
    anchor
        .add(intermediates[at].clone().into_owned())
        .map_err(|_| mismatch())?;
    let others: Vec<CertificateDer<'_>> = intermediates
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != at)
        .map(|(_, c)| c.clone())
        .collect();
    let cert = ParsedCertificate::try_from(end_entity)?;
    rustls::client::verify_server_cert_signed_by_trust_anchor(
        &cert,
        &anchor,
        &others,
        now,
        provider.signature_verification_algorithms.all,
    )
    .map_err(|_| mismatch())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::install_crypto;
    use crate::testkit::tls::{CA_PEM, SERVER_CERT_PEM, ca_file, cert_der, server_config};

    /// Runs a handshake in memory between `config` and the test server, which presents its
    /// certificate and the CA; the client connects to `name`.
    fn handshake(config: ClientConfig, name: &str) -> Result<(), rustls::Error> {
        let name = server_name(name).unwrap();
        let mut client = rustls::ClientConnection::new(Arc::new(config), name)?;
        let mut server = rustls::ServerConnection::new(server_config())?;
        for _ in 0..20 {
            if !client.is_handshaking() && !server.is_handshaking() {
                return Ok(());
            }
            let mut buf = Vec::new();
            client.write_tls(&mut buf).unwrap();
            server.read_tls(&mut buf.as_slice()).unwrap();
            if let Err(e) = server.process_new_packets() {
                // Lets the client read the server's alert.
                let mut alert = Vec::new();
                let _ = server.write_tls(&mut alert);
                let _ = client.read_tls(&mut alert.as_slice());
                return client.process_new_packets().map(|_| ()).and(Err(e));
            }
            let mut buf = Vec::new();
            server.write_tls(&mut buf).unwrap();
            client.read_tls(&mut buf.as_slice()).unwrap();
            client.process_new_packets()?;
        }
        panic!("the handshake did not finish");
    }

    fn config(mode: SslMode, ca: Option<&str>, pin: Option<[u8; 32]>) -> ClientConfig {
        install_crypto();
        rustls_config(mode, ca, None, &[b"http/1.1"], pin).unwrap()
    }

    #[test]
    fn modes_check_the_chain_and_the_name_as_they_say() {
        let ca = ca_file();
        let ca = ca.path().to_str().unwrap();
        for mode in [SslMode::Disable, SslMode::Prefer, SslMode::Require] {
            handshake(config(mode, None, None), "elsewhere.test").unwrap();
        }
        // The test CA is not one of the system's.
        let untrusted = handshake(config(SslMode::VerifyFull, None, None), "db.example.test");
        assert!(
            matches!(untrusted, Err(rustls::Error::InvalidCertificate(_))),
            "{untrusted:?}"
        );
        assert!(handshake(config(SslMode::VerifyCa, None, None), "db.example.test").is_err());
        handshake(
            config(SslMode::VerifyFull, Some(ca), None),
            "db.example.test",
        )
        .unwrap();
        handshake(config(SslMode::VerifyFull, Some(ca), None), "127.0.0.1").unwrap();
        let wrong_name = handshake(config(SslMode::VerifyFull, Some(ca), None), "other.test");
        assert!(wrong_name.is_err(), "{wrong_name:?}");
        // verify-ca checks the chain only.
        handshake(config(SslMode::VerifyCa, Some(ca), None), "other.test").unwrap();
    }

    #[test]
    fn a_pin_matches_the_server_certificate_or_the_ca_that_signed_it() {
        let leaf = sha256(&cert_der(SERVER_CERT_PEM));
        let ca = sha256(&cert_der(CA_PEM));
        handshake(
            config(SslMode::Require, None, Some(leaf)),
            "db.example.test",
        )
        .unwrap();
        handshake(config(SslMode::Require, None, Some(ca)), "db.example.test").unwrap();
        let mut other = ca;
        other[0] ^= 1;
        let refused = handshake(
            config(SslMode::Require, None, Some(other)),
            "db.example.test",
        );
        assert!(
            matches!(&refused, Err(rustls::Error::General(m)) if m.contains("pinned")),
            "{refused:?}"
        );
        // A pin adds to verify-full; it does not replace the name check.
        let file = ca_file();
        let ca_path = file.path().to_str().unwrap();
        handshake(
            config(SslMode::VerifyFull, Some(ca_path), Some(ca)),
            "db.example.test",
        )
        .unwrap();
        assert!(
            handshake(
                config(SslMode::VerifyFull, Some(ca_path), Some(ca)),
                "other.test"
            )
            .is_err()
        );
    }

    #[test]
    fn a_tunnel_to_an_ip_address_checks_the_server_name() {
        install_crypto();
        let file = ca_file();
        let ca = file.path().to_str().unwrap();
        let local: IpAddr = "127.0.0.2".parse().unwrap();
        // The certificate names 127.0.0.1, not the tunnel's 127.0.0.2.
        let through = |server: &str| {
            rustls_config_through(
                SslMode::VerifyFull,
                Some(ca),
                None,
                &[],
                None,
                Some((local, server_name(server).unwrap())),
            )
            .unwrap()
        };
        handshake(through("127.0.0.1"), "127.0.0.2").unwrap();
        assert!(handshake(through("10.0.0.9"), "127.0.0.2").is_err());
        // Another name than the tunnel's is checked as itself.
        handshake(through("10.0.0.9"), "db.example.test").unwrap();
    }

    #[test]
    fn alpn_and_client_certificates_are_set() {
        install_crypto();
        let c = rustls_config(SslMode::Require, None, None, &[b"h2", b"http/1.1"], None).unwrap();
        assert_eq!(c.alpn_protocols, vec![b"h2".to_vec(), b"http/1.1".to_vec()]);
        assert!(!c.client_auth_cert_resolver.has_certs());
        let key = rustls::pki_types::PrivateKeyDer::from_pem_slice(
            crate::testkit::tls::SERVER_KEY_PEM.as_bytes(),
        )
        .unwrap();
        let identity = ClientCertificate {
            certs: vec![cert_der(SERVER_CERT_PEM)],
            key,
        };
        let c = rustls_config(SslMode::Require, None, Some(identity), &[], None).unwrap();
        assert!(c.client_auth_cert_resolver.has_certs());
        assert!(c.alpn_protocols.is_empty());
    }

    #[test]
    fn ca_files_are_read_or_refused_with_their_path() {
        install_crypto();
        let missing = rustls_config(
            SslMode::VerifyCa,
            Some("/nonexistent/ca.pem"),
            None,
            &[],
            None,
        )
        .unwrap_err();
        assert_eq!(missing.code(), "CONNECT_FAILED");
        assert!(missing.to_string().contains("/nonexistent/ca.pem"));
        let empty = tempfile::NamedTempFile::new().unwrap();
        let err =
            rustls_config(SslMode::VerifyFull, empty.path().to_str(), None, &[], None).unwrap_err();
        assert!(err.to_string().contains("no certificates found"), "{err}");
        // Modes that verify nothing read no file.
        rustls_config(
            SslMode::Require,
            Some("/nonexistent/ca.pem"),
            None,
            &[],
            None,
        )
        .unwrap();
    }

    #[test]
    fn pins_parse_with_or_without_separators() {
        let hex = "a62aac8b6ffdf8ce8f1b2e0b4e5916963774659cf150ff600b4a0f56afa911a0";
        let pin = parse_pin(hex).unwrap();
        assert_eq!(pin[0], 0xa6);
        assert_eq!(pin[31], 0xa0);
        let colons = hex
            .as_bytes()
            .chunks(2)
            .map(|p| std::str::from_utf8(p).unwrap().to_ascii_uppercase())
            .collect::<Vec<_>>()
            .join(":");
        assert_eq!(parse_pin(&colons).unwrap(), pin);
        assert_eq!(parse_pin(&format!(" {hex} ")).unwrap(), pin);
        let long = format!("{hex}00");
        let not_hex = hex.replace('a', "g");
        let bad: [&str; 5] = ["", "abc", &hex[..62], &long, &not_hex];
        for bad in bad {
            assert_eq!(parse_pin(bad).unwrap_err().code(), "INVALID_INPUT", "{bad}");
        }
    }

    #[test]
    fn server_names_take_hosts_and_bracketed_addresses() {
        assert!(matches!(
            server_name("db.example.test").unwrap(),
            ServerName::DnsName(_)
        ));
        assert!(matches!(
            server_name("[::1]").unwrap(),
            ServerName::IpAddress(_)
        ));
        assert!(matches!(
            server_name("10.0.0.1").unwrap(),
            ServerName::IpAddress(_)
        ));
        assert_eq!(server_name("bad name").unwrap_err().code(), "INVALID_INPUT");
    }
}
