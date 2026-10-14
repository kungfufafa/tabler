//! The TLS files of a connection profile (the form's Key… / Cert… buttons), read the same way for
//! PostgreSQL and MySQL so both accept the same formats and fail with the same errors naming the
//! file: PEM only; the private key unencrypted PKCS#8 (`BEGIN PRIVATE KEY`), PKCS#1
//! (`BEGIN RSA PRIVATE KEY`) or SEC1 (`BEGIN EC PRIVATE KEY`); the certificate file holding the
//! client certificate first, then its intermediates (one PEM with key + chain works for both).
//! Also the native (macOS Keychain) trust roots both drivers add to their own.

use base64::Engine as _;
use rustls::pki_types::pem::PemObject as _;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

use crate::error::AppError;

/// What the Key… / Cert… files must be (shown in errors and in the form's SSL hint).
pub const ACCEPTED_FORMATS: &str = "PEM files: an unencrypted PKCS#8, PKCS#1 or SEC1 key, and \
     the client certificate followed by its intermediates";

/// A client certificate chain (leaf first) and its private key.
pub struct ClientCertificate {
    pub certs: Vec<CertificateDer<'static>>,
    pub key: PrivateKeyDer<'static>,
}

impl std::fmt::Debug for ClientCertificate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientCertificate")
            .field("certs", &self.certs.len())
            .field("key", &"***")
            .finish()
    }
}

impl ClientCertificate {
    /// The chain as PEM (what mysql_async reads from a buffer).
    pub fn certs_pem(&self) -> Vec<u8> {
        self.certs
            .iter()
            .map(|c| pem_block("CERTIFICATE", c))
            .collect::<String>()
            .into_bytes()
    }

    /// The key as PEM, in its own format.
    pub fn key_pem(&self) -> Vec<u8> {
        let label = match &self.key {
            PrivateKeyDer::Pkcs1(_) => "RSA PRIVATE KEY",
            PrivateKeyDer::Sec1(_) => "EC PRIVATE KEY",
            _ => "PRIVATE KEY",
        };
        pem_block(label, self.key.secret_der()).into_bytes()
    }
}

/// One PEM block: base64 in lines of 64.
pub fn pem_block(label: &str, der: &[u8]) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(der);
    let mut out = format!("-----BEGIN {label}-----\n");
    for line in b64.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(line).unwrap_or_default());
        out.push('\n');
    }
    out.push_str(&format!("-----END {label}-----\n"));
    out
}

fn read(path: &str) -> Result<Vec<u8>, AppError> {
    std::fs::read(crate::tunnel::expand_tilde(path))
        .map_err(|e| AppError::Connect(format!("cannot read {path}: {e}")))
}

fn is_pem(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).is_ok_and(|t| t.contains("-----BEGIN "))
}

/// A PKCS#12 bundle (`.p12` / `.pfx`): a DER SEQUENCE starting with `INTEGER 3` (its version).
fn is_pkcs12(path: &str, bytes: &[u8]) -> bool {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    if matches!(ext.as_deref(), Some("p12" | "pfx")) {
        return true;
    }
    let body = match bytes {
        [0x30, len, rest @ ..] if *len < 0x80 => rest,
        [0x30, 0x81, _, rest @ ..] => rest,
        [0x30, 0x82, _, _, rest @ ..] => rest,
        [0x30, 0x83, _, _, _, rest @ ..] => rest,
        _ => return false,
    };
    body.starts_with(&[0x02, 0x01, 0x03])
}

/// The error for a file that is not PEM, saying what it is and how to convert it.
fn not_pem(path: &str, bytes: &[u8], key: bool) -> AppError {
    let msg = if is_pkcs12(path, bytes) {
        format!(
            "{path}: expected a PEM file, got a PKCS#12 bundle; export its key and certificate \
             as PEM (openssl pkcs12 -in {path} -nodes -out client.pem) and choose that file for \
             both Key… and Cert…"
        )
    } else if bytes.first() == Some(&0x30) {
        let convert = if key {
            format!("openssl pkey -inform DER -in {path} -out client.key")
        } else {
            format!("openssl x509 -inform DER -in {path} -out client.crt")
        };
        format!("{path}: expected a PEM file, got DER (binary); convert it ({convert})")
    } else {
        format!("{path}: expected a PEM file (-----BEGIN …-----)")
    };
    AppError::InvalidInput(msg)
}

fn is_encrypted_key(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).is_ok_and(|t| {
        t.contains("-----BEGIN ENCRYPTED PRIVATE KEY-----")
            || t.lines()
                .any(|l| l.starts_with("Proc-Type:") && l.contains("ENCRYPTED"))
    })
}

/// The certificates of a PEM file, in file order.
fn read_certs(path: &str) -> Result<Vec<CertificateDer<'static>>, AppError> {
    let bytes = read(path)?;
    if !is_pem(&bytes) {
        return Err(not_pem(path, &bytes, false));
    }
    let certs = CertificateDer::pem_slice_iter(&bytes)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| AppError::InvalidInput(format!("{path}: invalid PEM: {e:?}")))?;
    if certs.is_empty() {
        return Err(AppError::InvalidInput(format!(
            "{path}: no certificate found (expected -----BEGIN CERTIFICATE-----)"
        )));
    }
    Ok(certs)
}

/// The (first) private key of a PEM file.
fn read_key(path: &str) -> Result<PrivateKeyDer<'static>, AppError> {
    let bytes = read(path)?;
    if !is_pem(&bytes) {
        return Err(not_pem(path, &bytes, true));
    }
    if is_encrypted_key(&bytes) {
        return Err(AppError::InvalidInput(format!(
            "{path}: encrypted private keys are not supported; decrypt it first \
             (openssl pkey -in {path} -out client.key)"
        )));
    }
    PrivateKeyDer::from_pem_slice(&bytes).map_err(|e| {
        AppError::InvalidInput(match e {
            rustls::pki_types::pem::Error::NoItemsFound => format!(
                "{path}: no private key found (expected BEGIN PRIVATE KEY, RSA PRIVATE KEY or \
                 EC PRIVATE KEY)"
            ),
            other => format!("{path}: invalid PEM private key: {other:?}"),
        })
    })
}

fn provider() -> std::sync::Arc<rustls::crypto::CryptoProvider> {
    rustls::crypto::CryptoProvider::get_default()
        .cloned()
        .unwrap_or_else(|| std::sync::Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
}

/// Checks that the key is usable and belongs to the first certificate (the client's own).
fn check_pair(cert: &ClientCertificate, key_path: &str, cert_path: &str) -> Result<(), AppError> {
    use rustls::{Error, InconsistentKeys};
    match rustls::sign::CertifiedKey::from_der(
        cert.certs.clone(),
        cert.key.clone_key(),
        &provider(),
    ) {
        Ok(_) => Ok(()),
        Err(Error::InconsistentKeys(InconsistentKeys::KeyMismatch)) => {
            Err(AppError::InvalidInput(format!(
                "SSL client certificate: the first certificate in {cert_path} does not belong \
                 to the private key in {key_path}; the file must start with the client \
                 certificate, followed by its intermediates"
            )))
        }
        Err(e) => Err(AppError::InvalidInput(format!(
            "SSL client certificate: the private key in {key_path} cannot be used: {e}"
        ))),
    }
}

/// Loads the profile's Key… / Cert… pair. Both empty: no client certificate; only one of them:
/// an error (the server could not use it).
pub fn load_client_certificate(
    key_path: Option<&str>,
    cert_path: Option<&str>,
) -> Result<Option<ClientCertificate>, AppError> {
    let key_path = key_path.map(str::trim).filter(|p| !p.is_empty());
    let cert_path = cert_path.map(str::trim).filter(|p| !p.is_empty());
    let (key_path, cert_path) = match (key_path, cert_path) {
        (None, None) => return Ok(None),
        (Some(k), Some(c)) => (k, c),
        (Some(_), None) => {
            return Err(AppError::InvalidInput(
                "SSL: choose the client certificate (Cert…) that goes with the key".into(),
            ));
        }
        (None, Some(_)) => {
            return Err(AppError::InvalidInput(
                "SSL: choose the private key (Key…) of the client certificate".into(),
            ));
        }
    };
    let cert = ClientCertificate {
        certs: read_certs(cert_path)?,
        key: read_key(key_path)?,
    };
    check_pair(&cert, key_path, cert_path)?;
    Ok(Some(cert))
}

/// The trust roots of the operating system (the macOS Keychain's), those rustls can parse.
pub fn native_roots() -> Vec<CertificateDer<'static>> {
    let native = rustls_native_certs::load_native_certs();
    for err in &native.errors {
        log::warn!("native certificate store: {err}");
    }
    let mut store = rustls::RootCertStore::empty();
    native
        .certs
        .into_iter()
        .filter(|c| store.add(c.clone()).is_ok())
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;

    /// Client TLS material in every format, made with the `openssl` CLI (None without it).
    pub(crate) struct Material {
        pub dir: tempfile::TempDir,
    }

    impl Material {
        pub fn path(&self, name: &str) -> String {
            self.dir.path().join(name).to_string_lossy().into_owned()
        }
    }

    fn openssl(dir: &Path, args: &[&str]) -> bool {
        Command::new("openssl")
            .args(args)
            .current_dir(dir)
            .output()
            .is_ok_and(|o| o.status.success())
    }

    pub(crate) fn material() -> Option<Material> {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let steps: &[&[&str]] = &[
            &[
                "req",
                "-x509",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-keyout",
                "ca.key",
                "-out",
                "ca.crt",
                "-subj",
                "/CN=Tabler Test CA",
                "-days",
                "2",
            ],
            &[
                "req",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-keyout",
                "client.key",
                "-out",
                "client.csr",
                "-subj",
                "/CN=tabler_cert",
            ],
            &[
                "x509",
                "-req",
                "-in",
                "client.csr",
                "-CA",
                "ca.crt",
                "-CAkey",
                "ca.key",
                "-CAcreateserial",
                "-out",
                "client.crt",
                "-days",
                "2",
            ],
            &[
                "rsa",
                "-in",
                "client.key",
                "-traditional",
                "-out",
                "client-pkcs1.key",
            ],
            &[
                "pkcs8",
                "-topk8",
                "-in",
                "client.key",
                "-out",
                "client-encrypted.key",
                "-passout",
                "pass:tabler",
            ],
            &[
                "pkcs8",
                "-topk8",
                "-nocrypt",
                "-in",
                "client.key",
                "-outform",
                "DER",
                "-out",
                "client-key.der",
            ],
            &[
                "x509",
                "-in",
                "client.crt",
                "-outform",
                "DER",
                "-out",
                "client-crt.der",
            ],
            &[
                "pkcs12",
                "-export",
                "-inkey",
                "client.key",
                "-in",
                "client.crt",
                "-out",
                "client.bundle",
                "-passout",
                "pass:tabler",
            ],
            &[
                "ecparam",
                "-name",
                "prime256v1",
                "-genkey",
                "-noout",
                "-out",
                "client-ec.key",
            ],
            &[
                "req",
                "-x509",
                "-key",
                "client-ec.key",
                "-out",
                "client-ec.crt",
                "-subj",
                "/CN=tabler_ec",
                "-days",
                "2",
            ],
        ];
        for step in steps {
            if !openssl(d, step) {
                eprintln!("skipped: openssl {} failed or is missing", step.join(" "));
                return None;
            }
        }
        let cat = |out: &str, parts: &[&str]| {
            let text: String = parts
                .iter()
                .map(|p| std::fs::read_to_string(d.join(p)).unwrap())
                .collect();
            std::fs::write(d.join(out), text).unwrap();
        };
        cat("client-chain.crt", &["client.crt", "ca.crt"]);
        cat("client-chain-reversed.crt", &["ca.crt", "client.crt"]);
        cat(
            "client-combined.pem",
            &["client.key", "client.crt", "ca.crt"],
        );
        Some(Material { dir })
    }

    fn load(m: &Material, key: &str, cert: &str) -> Result<Option<ClientCertificate>, AppError> {
        load_client_certificate(Some(&m.path(key)), Some(&m.path(cert)))
    }

    #[test]
    fn pairs_and_missing_files() {
        assert!(load_client_certificate(None, Some(" ")).unwrap().is_none());
        let err = load_client_certificate(Some("/k.pem"), None).unwrap_err();
        assert!(err.to_string().contains("Cert…"), "{err}");
        let err = load_client_certificate(None, Some("/c.pem")).unwrap_err();
        assert!(err.to_string().contains("Key…"), "{err}");
        let err = load_client_certificate(Some("/nonexistent/k.pem"), Some("/nonexistent/c.pem"))
            .unwrap_err();
        assert!(
            err.to_string().contains("cannot read /nonexistent/"),
            "{err}"
        );
    }

    #[test]
    fn accepted_formats_load_and_round_trip_as_pem() {
        let Some(m) = material() else { return };
        for (key, cert) in [
            ("client.key", "client-chain.crt"),
            ("client-pkcs1.key", "client-chain.crt"),
            ("client-ec.key", "client-ec.crt"),
            ("client-combined.pem", "client-combined.pem"),
        ] {
            let c = load(&m, key, cert)
                .unwrap_or_else(|e| panic!("{key} / {cert}: {e}"))
                .unwrap();
            // Re-encoded PEM parses back to the same material.
            let certs = CertificateDer::pem_slice_iter(&c.certs_pem())
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert_eq!(certs, c.certs);
            let back = PrivateKeyDer::from_pem_slice(&c.key_pem()).unwrap();
            assert_eq!(back.secret_der(), c.key.secret_der());
            assert_eq!(
                std::mem::discriminant(&back),
                std::mem::discriminant(&c.key)
            );
        }
        let chain = load(&m, "client.key", "client-chain.crt").unwrap().unwrap();
        assert_eq!(chain.certs.len(), 2);
        assert!(matches!(
            load(&m, "client-pkcs1.key", "client.crt")
                .unwrap()
                .unwrap()
                .key,
            PrivateKeyDer::Pkcs1(_)
        ));
    }

    #[test]
    fn refused_formats_name_the_file() {
        let Some(m) = material() else { return };
        let msg = |key: &str, cert: &str| load(&m, key, cert).unwrap_err().to_string();

        let e = msg("client-encrypted.key", "client.crt");
        assert!(
            e.contains("client-encrypted.key") && e.contains("encrypted"),
            "{e}"
        );
        let e = msg("client-key.der", "client.crt");
        assert!(e.contains("client-key.der") && e.contains("DER"), "{e}");
        let e = msg("client.key", "client-crt.der");
        assert!(e.contains("client-crt.der") && e.contains("DER"), "{e}");
        // PKCS#12 is recognised by its content, whatever the extension.
        let e = msg("client.bundle", "client.bundle");
        assert!(e.contains("client.bundle") && e.contains("PKCS#12"), "{e}");
        let e = msg("client.key", "client-chain-reversed.crt");
        assert!(
            e.contains("SSL client certificate") && e.contains("key"),
            "{e}"
        );
        assert!(e.contains("client-chain-reversed.crt"), "{e}");
        // A key with the wrong certificate.
        let e = msg("client-ec.key", "client.crt");
        assert!(e.contains("does not belong"), "{e}");
        // Text that is not PEM, and a PEM without a key.
        std::fs::write(m.dir.path().join("junk.pem"), "hello").unwrap();
        let e = msg("junk.pem", "client.crt");
        assert!(
            e.contains("junk.pem") && e.contains("expected a PEM file"),
            "{e}"
        );
        let e = msg("client.crt", "client.crt");
        assert!(e.contains("no private key found"), "{e}");
    }

    #[test]
    fn pem_blocks_wrap_at_64() {
        let block = pem_block("CERTIFICATE", &[7u8; 100]);
        let lines: Vec<&str> = block.lines().collect();
        assert_eq!(lines.first(), Some(&"-----BEGIN CERTIFICATE-----"));
        assert_eq!(lines.last(), Some(&"-----END CERTIFICATE-----"));
        assert!(lines[1..lines.len() - 1].iter().all(|l| l.len() <= 64));
    }
}
