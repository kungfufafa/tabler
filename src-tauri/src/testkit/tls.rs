//! TLS for the tests of `tls_client` and the HTTP client: a test CA, a server certificate it
//! signed (for `db.example.test`, `localhost` and `127.0.0.1`, both valid until 2126), and a
//! small HTTPS server that records the name each client asked for (SNI) and the request it sent.
//! Made once with the `openssl` CLI (P-256 keys).

use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rustls::pki_types::pem::PemObject as _;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

/// The test CA (`CN=Tabler Test CA`).
pub const CA_PEM: &str = "-----BEGIN CERTIFICATE-----
MIIBmTCCAT+gAwIBAgIUVp+eHsyxZ5Bk2/IU6pP7cbxtd1YwCgYIKoZIzj0EAwIw
GTEXMBUGA1UEAwwOVGFibGVyIFRlc3QgQ0EwIBcNMjYxMDI5MDQwNzQyWhgPMjEy
NjEwMDUwNDA3NDJaMBkxFzAVBgNVBAMMDlRhYmxlciBUZXN0IENBMFkwEwYHKoZI
zj0CAQYIKoZIzj0DAQcDQgAEDnqZQXmbKxrYpRXpcYXdZB3Zh4e88fXwfv5KLdNb
xppQtODa7Xy6JCDylYJR80O2Gja0jqVjJ+k1J/R7U+Q4pqNjMGEwHQYDVR0OBBYE
FMhu2TbPrfwYhffXHwN2yV2tcWucMB8GA1UdIwQYMBaAFMhu2TbPrfwYhffXHwN2
yV2tcWucMA8GA1UdEwEB/wQFMAMBAf8wDgYDVR0PAQH/BAQDAgEGMAoGCCqGSM49
BAMCA0gAMEUCIQDTM4URaUehXFc7qd1NBi+YR/BmOgCj36jZkVQOvcn2MgIgQMAj
6aJ1lBez3/dkG3QzidzY7lc7hVjrEvyo++UNMdE=
-----END CERTIFICATE-----
";

/// The server's certificate, signed by [`CA_PEM`]: `CN=db.example.test`, with the names
/// `db.example.test`, `localhost` and `127.0.0.1`.
pub const SERVER_CERT_PEM: &str = "-----BEGIN CERTIFICATE-----
MIIB3DCCAYGgAwIBAgIUFlAoFlMVKy+tLArG78/HXvl3WhkwCgYIKoZIzj0EAwIw
GTEXMBUGA1UEAwwOVGFibGVyIFRlc3QgQ0EwIBcNMjYxMDI5MDQwNzQyWhgPMjEy
NjEwMDUwNDA3NDJaMBoxGDAWBgNVBAMMD2RiLmV4YW1wbGUudGVzdDBZMBMGByqG
SM49AgEGCCqGSM49AwEHA0IABAERnMxG6tRzfopCt0AkrH4b86lKKQQSSd25Q5HT
j/vBxCcdtQ6t10ToucPvku6aAfEIX9hGChJjP6uVeYkJA9SjgaMwgaAwDAYDVR0T
AQH/BAIwADAOBgNVHQ8BAf8EBAMCB4AwEwYDVR0lBAwwCgYIKwYBBQUHAwEwKwYD
VR0RBCQwIoIPZGIuZXhhbXBsZS50ZXN0gglsb2NhbGhvc3SHBH8AAAEwHQYDVR0O
BBYEFHg+k9Fsbt6i5FnK7++JEyv6Oic/MB8GA1UdIwQYMBaAFMhu2TbPrfwYhffX
HwN2yV2tcWucMAoGCCqGSM49BAMCA0kAMEYCIQC/LPyOg9HDZ3E7e4pxzHO5AKxg
xpbEHEUm/aR/ZXyUvAIhAKHzbEkkNa27MOskLhsXwDfKxfK2GaqOCCkrhtGiIiDb
-----END CERTIFICATE-----
";

/// The private key of [`SERVER_CERT_PEM`] (PKCS#8).
pub const SERVER_KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQg6lHYWWjiUakcuxTW
9QBULk889qhEXQXKJsh0Hp5c+7GhRANCAAQBEZzMRurUc36KQrdAJKx+G/OpSikE
EknduUOR04/7wcQnHbUOrddE6LnD75LumgHxCF/YRgoSYz+rlXmJCQPU
-----END PRIVATE KEY-----
";

pub fn cert_der(pem: &str) -> CertificateDer<'static> {
    CertificateDer::from_pem_slice(pem.as_bytes()).expect("a certificate")
}

/// The CA in a PEM file, as a profile's CA file names it.
pub fn ca_file() -> tempfile::NamedTempFile {
    let mut file = tempfile::NamedTempFile::new().expect("a temporary file");
    file.write_all(CA_PEM.as_bytes()).expect("written");
    file
}

/// The server's side: the server certificate followed by the CA, as servers with their own CA
/// (Elasticsearch's generated one) present them.
pub fn server_config() -> Arc<rustls::ServerConfig> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let key = PrivateKeyDer::from_pem_slice(SERVER_KEY_PEM.as_bytes()).expect("a key");
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("protocol versions")
        .with_no_client_auth()
        .with_single_cert(vec![cert_der(SERVER_CERT_PEM), cert_der(CA_PEM)], key)
        .expect("a server config");
    Arc::new(config)
}

/// What the HTTPS server saw of one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsRequest {
    /// The name the client sent (SNI); none for an IP address.
    pub sni: Option<String>,
    /// The request line.
    pub line: String,
    /// Names lower-cased.
    pub headers: Vec<(String, String)>,
}

impl TlsRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// An HTTPS server on 127.0.0.1 with [`server_config`]: it answers every request `200 ok` and
/// records it; a client that refuses its certificate leaves no record. It stops when dropped.
pub struct TlsServer {
    addr: SocketAddr,
    log: Arc<Mutex<Vec<TlsRequest>>>,
    stop: Arc<AtomicBool>,
}

impl TlsServer {
    pub fn start() -> TlsServer {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("local address");
        let log = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let config = server_config();
        {
            let (log, stop) = (log.clone(), stop.clone());
            std::thread::spawn(move || {
                for sock in listener.incoming() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(sock) = sock else { continue };
                    let (config, log) = (config.clone(), log.clone());
                    std::thread::spawn(move || serve(sock, config, log));
                }
            });
        }
        TlsServer { addr, log, stop }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn requests(&self) -> Vec<TlsRequest> {
        self.log.lock().unwrap().clone()
    }
}

impl Drop for TlsServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wakes the accept loop so it sees the flag.
        let _ = TcpStream::connect(self.addr);
    }
}

fn serve(sock: TcpStream, config: Arc<rustls::ServerConfig>, log: Arc<Mutex<Vec<TlsRequest>>>) {
    let _ = sock.set_read_timeout(Some(Duration::from_secs(10)));
    let Ok(conn) = rustls::ServerConnection::new(config) else {
        return;
    };
    let mut tls = rustls::StreamOwned::new(conn, sock);
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    // The handshake runs inside the first read; a client that refuses the certificate ends it.
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        match tls.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    let head = String::from_utf8_lossy(&buf).into_owned();
    let mut lines = head.split("\r\n");
    let line = lines.next().unwrap_or_default().to_string();
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let sni = tls.conn.server_name().map(str::to_string);
    log.lock().unwrap().push(TlsRequest { sni, line, headers });
    let _ = tls.write_all(
        b"HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok",
    );
    tls.conn.send_close_notify();
    let _ = tls.flush();
}
