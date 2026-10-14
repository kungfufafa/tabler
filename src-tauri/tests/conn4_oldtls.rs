//! MySQL "Ver." ▸ Old MySQL against a server that offers nothing newer than TLS 1.1.
//!
//! No real MySQL 5.5 / 5.6 is needed: a small fake server (python3 + its `ssl` module, a
//! throwaway self-signed certificate made with the `openssl` command) speaks the MySQL handshake
//! in clear, upgrades the connection to TLS 1.0 / 1.1 only, accepts any login and answers the
//! handful of queries a connect makes. It reports the negotiated TLS version on stdout. Skips
//! itself when python3, its TLS 1.1 support or `openssl` is missing, and on non-macOS builds
//! (the platform TLS path of the Old variant exists for macOS only).
//!
//! The Default variant (rustls, TLS 1.2 and up) must be refused by the same server: that is what
//! the Old variant exists for.
//!
//! The same server also asks a user for `mysql_clear_password` (PAM / LDAP): the password may go
//! over TCP only through an SSH tunnel (`host_override`), TLS or a socket.

#![allow(clippy::result_large_err)]
#![cfg(target_os = "macos")]

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use tabler_lib::drivers::{ConnectTarget, connect};
use tabler_lib::types::profile::{
    ConnectionProfile, Driver, MysqlVersion, ProfileOptions, SslMode,
};

const SERVER: &str = r#"
import os, socket, ssl, struct, sys, threading

cert, key = sys.argv[1], sys.argv[2]
ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
try:
    ctx.minimum_version = ssl.TLSVersion.TLSv1
    ctx.maximum_version = ssl.TLSVersion.TLSv1_1
    ctx.set_ciphers("ALL:@SECLEVEL=0")
    ctx.load_cert_chain(cert, key)
except Exception as e:
    print("UNSUPPORTED %s" % e, flush=True)
    sys.exit(3)

def pkt(seq, payload):
    return struct.pack("<I", len(payload))[:3] + bytes([seq & 255]) + payload

def lenenc(b):
    assert len(b) < 251
    return bytes([len(b)]) + b

def recv_exact(s, n):
    out = b""
    while len(out) < n:
        chunk = s.recv(n - len(out))
        if not chunk:
            raise EOFError()
        out += chunk
    return out

def read_packet(s):
    hdr = recv_exact(s, 4)
    n = hdr[0] | (hdr[1] << 8) | (hdr[2] << 16)
    return hdr[3], recv_exact(s, n)

OK = b"\x00\x00\x00" + struct.pack("<H", 2) + struct.pack("<H", 0)
EOF = b"\xfe" + struct.pack("<H", 0) + struct.pack("<H", 2)

def resultset(s, seq, sql):
    cols = [c.strip() for c in sql[6:].split(",")]
    s.sendall(pkt(seq, bytes([len(cols)])))
    seq += 1
    for c in cols:
        d = (lenenc(b"def") + lenenc(b"") + lenenc(b"") + lenenc(b"") + lenenc(c.encode())
             + lenenc(c.encode()) + b"\x0c" + struct.pack("<H", 33) + struct.pack("<I", 255)
             + b"\xfd" + struct.pack("<H", 0) + b"\x00" + b"\x00\x00")
        s.sendall(pkt(seq, d)); seq += 1
    s.sendall(pkt(seq, EOF)); seq += 1
    row = b""
    for c in cols:
        low = c.lower()
        v = b"5.5.62-fake" if "version" in low else b"16777216" if "max_allowed_packet" in low \
            else b"28800" if "timeout" in low else b""
        row += lenenc(v)
    s.sendall(pkt(seq, row)); seq += 1
    s.sendall(pkt(seq, EOF))

def serve(raw):
    try:
        salt = bytes((i % 120) + 1 for i in range(20))
        caps = 1 | 2 | 4 | 8 | 0x200 | 0x800 | 0x2000 | 0x8000 | 0x80000
        hello = (b"\x0a" + b"5.5.62-fake\x00" + struct.pack("<I", 7) + salt[:8] + b"\x00"
                 + struct.pack("<H", caps & 0xFFFF) + bytes([33]) + struct.pack("<H", 2)
                 + struct.pack("<H", caps >> 16) + bytes([21]) + b"\x00" * 10 + salt[8:] + b"\x00"
                 + b"mysql_native_password\x00")
        raw.sendall(pkt(0, hello))
        seq, first = read_packet(raw)
        if len(first) == 32:
            # An SSLRequest: the login follows inside TLS 1.0 / 1.1.
            s = ctx.wrap_socket(raw, server_side=True)
            print("TLS %s %s" % (s.version(), s.cipher()[0]), flush=True)
            seq, login = read_packet(s)
        else:
            s, login = raw, first
            print("PLAIN", flush=True)
        # HandshakeResponse41: 32 fixed bytes, then the user name.
        user = login[32:].split(b"\x00")[0]
        if user == b"oldpw":
            # A pre-4.1 password hash: the server asks for the old scramble.
            s.sendall(pkt(seq + 1, b"\xfe" + b"mysql_old_password\x00" + b"abcdefgh\x00"))
            seq, _scramble = read_packet(s)
        elif user == b"clearpw":
            # PAM / LDAP: the server asks for the password in clear text.
            s.sendall(pkt(seq + 1, b"\xfe" + b"mysql_clear_password\x00"))
            seq, reply = read_packet(s)
            print("CLEAR %s" % reply.rstrip(b"\x00").decode(), flush=True)
        s.sendall(pkt(seq + 1, OK))
        while True:
            seq, p = read_packet(s)
            if p[0] == 0x01:
                break
            if p[0] == 0x03 and p[1:].decode(errors="replace").strip().upper().startswith("SELECT"):
                resultset(s, seq + 1, p[1:].decode(errors="replace").strip())
            else:
                s.sendall(pkt(seq + 1, OK))
    except Exception as e:
        print("ERROR %s: %s" % (type(e).__name__, e), flush=True)
    finally:
        try:
            raw.close()
        except Exception:
            pass

srv = socket.socket()
srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
srv.bind(("127.0.0.1", 0))
srv.listen(8)
print("PORT %d" % srv.getsockname()[1], flush=True)
while True:
    conn, _ = srv.accept()
    threading.Thread(target=serve, args=(conn,), daemon=True).start()
"#;

struct FakeServer {
    child: Child,
    port: u16,
    lines: Receiver<String>,
    /// The server's certificate (self-signed, so it is its own CA).
    cert: std::path::PathBuf,
    /// A certificate that signed nothing the server uses.
    other_cert: std::path::PathBuf,
    _dir: tempfile::TempDir,
}

/// A self-signed certificate for `localhost` with the extensions Apple's trust evaluation wants
/// of a TLS server certificate (`serverAuth`, a SAN): (certificate, key).
fn make_cert(
    dir: &std::path::Path,
    name: &str,
) -> Result<(std::path::PathBuf, std::path::PathBuf), String> {
    let (cert, key) = (
        dir.join(format!("{name}.pem")),
        dir.join(format!("{name}.key")),
    );
    let made = Command::new("openssl")
        .args([
            "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "2",
        ])
        .args(["-subj", &format!("/CN={name}")])
        .args(["-addext", "subjectAltName=DNS:localhost"])
        .args(["-addext", "extendedKeyUsage=serverAuth"])
        .args(["-addext", "basicConstraints=critical,CA:TRUE"])
        .arg("-keyout")
        .arg(&key)
        .arg("-out")
        .arg(&cert)
        .stderr(Stdio::null())
        .stdout(Stdio::null())
        .status()
        .map_err(|e| format!("no openssl command: {e}"))?;
    if !made.success() {
        return Err("openssl could not make a certificate".into());
    }
    Ok((cert, key))
}

impl Drop for FakeServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl FakeServer {
    /// Starts the server, or says why the test cannot run here.
    fn start() -> Result<FakeServer, String> {
        let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
        let script = dir.path().join("server.py");
        let (cert, key) = make_cert(dir.path(), "tabler-oldtls-test")?;
        let (other_cert, _) = make_cert(dir.path(), "tabler-oldtls-other")?;
        std::fs::write(&script, SERVER).map_err(|e| e.to_string())?;
        let mut child = Command::new("python3")
            .arg(&script)
            .arg(&cert)
            .arg(&key)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("no python3: {e}"))?;
        let stdout = child.stdout.take().expect("piped");
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let first = lines
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| "the fake server did not start".to_string())?;
        let Some(port) = first.strip_prefix("PORT ").and_then(|p| p.parse().ok()) else {
            let _ = child.kill();
            return Err(format!("the fake server cannot run here: {first}"));
        };
        Ok(FakeServer {
            child,
            port,
            lines,
            cert,
            other_cert,
            _dir: dir,
        })
    }

    /// The next line the server printed about a connection (`TLS TLSv1.1 <cipher>`, `PLAIN`).
    /// `ERROR ...` lines (a client that hung up on purpose) are skipped.
    fn next_line(&self) -> String {
        loop {
            let line = self
                .lines
                .recv_timeout(Duration::from_secs(10))
                .expect("the fake server said nothing");
            if !line.starts_with("ERROR ") {
                return line;
            }
        }
    }
}

fn profile(port: u16, ssl: SslMode, version: Option<MysqlVersion>) -> ConnectionProfile {
    profile_to("127.0.0.1", port, ssl, version, None)
}

fn profile_to(
    host: &str,
    port: u16,
    ssl: SslMode,
    version: Option<MysqlVersion>,
    ca: Option<&std::path::Path>,
) -> ConnectionProfile {
    ConnectionProfile {
        id: uuid::Uuid::new_v4(),
        name: "conn4 old tls".into(),
        color: None,
        group: None,
        driver: Driver::Mysql,
        host: Some(host.into()),
        port: Some(port),
        user: Some("legacy".into()),
        database: None,
        ssl,
        ssl_ca_path: ca.map(|c| c.to_string_lossy().into_owned()),
        sqlite_path: None,
        ssh: None,
        options: ProfileOptions {
            connect_timeout_secs: Some(15),
            mysql_version: version,
            ..ProfileOptions::default()
        },
        sort_order: 0,
        created_at: String::new(),
        updated_at: String::new(),
        last_connected_at: None,
    }
}

async fn open(p: &ConnectionProfile) -> Result<(), String> {
    open_via(p, None).await
}

/// `tunnel`: the local end of an SSH tunnel (the driver connects there instead of the profile's
/// host; here it is the fake server itself).
async fn open_via(p: &ConnectionProfile, tunnel: Option<(String, u16)>) -> Result<(), String> {
    let session = connect(ConnectTarget {
        profile: p,
        password: Some("secret"),
        host_override: tunnel,
    })
    .await
    .map_err(|e| e.to_string())?;
    session.close().await;
    Ok(())
}

#[tokio::test]
async fn conn4_old_mysql_negotiates_tls_1_1_and_the_default_driver_cannot() {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();
    let server = match FakeServer::start() {
        Ok(server) => server,
        Err(why) => {
            println!("skipped: {why}");
            return;
        }
    };

    // Old MySQL, the modes that need no certificate check: TLS 1.1 is negotiated.
    for ssl in [SslMode::Require, SslMode::Prefer] {
        let p = profile(server.port, ssl, Some(MysqlVersion::Old));
        open(&p)
            .await
            .unwrap_or_else(|e| panic!("Old MySQL with {ssl:?} against a TLS 1.1 server: {e}"));
        let line = server.next_line();
        assert!(line.starts_with("TLS TLSv1.1 "), "{line}");
    }

    // The verifying modes keep their meaning on the platform TLS path. The certificate lists
    // `localhost` (not the IP), and its own file is the CA.
    let old = Some(MysqlVersion::Old);
    let verify = |host: &str, ssl: SslMode, ca: &std::path::Path| {
        profile_to(host, server.port, ssl, old, Some(ca))
    };
    // VERIFY-CA: the chain is checked, the name is not.
    open(&verify("127.0.0.1", SslMode::VerifyCa, &server.cert))
        .await
        .unwrap_or_else(|e| panic!("Old MySQL VERIFY-CA to 127.0.0.1 with the right CA: {e}"));
    assert!(server.next_line().starts_with("TLS TLSv1.1 "));
    // ... and it is checked: a CA that signed nothing is refused.
    let err = open(&verify("127.0.0.1", SslMode::VerifyCa, &server.other_cert))
        .await
        .expect_err("VERIFY-CA with the wrong CA must fail");
    println!("Old MySQL VERIFY-CA with the wrong CA: {err}");
    assert!(err.contains("not trusted"), "{err}");
    assert!(!err.contains("host name"), "{err}");
    // VERIFY-IDENTITY: the name is checked against the profile host.
    let err = open(&verify("127.0.0.1", SslMode::VerifyFull, &server.cert))
        .await
        .expect_err("VERIFY-IDENTITY to an IP the certificate does not list must fail");
    println!("Old MySQL VERIFY-IDENTITY to 127.0.0.1: {err}");
    assert!(err.contains("host name"), "{err}");
    open(&verify("localhost", SslMode::VerifyFull, &server.cert))
        .await
        .unwrap_or_else(|e| panic!("Old MySQL VERIFY-IDENTITY to localhost: {e}"));
    // Drain what the server saw: three TLS 1.1 sessions were set up for the three failures and
    // successes above that reached the handshake end; the point is the versions, not the count.
    while let Ok(line) = server.lines.recv_timeout(Duration::from_millis(300)) {
        assert!(
            !line.starts_with("TLS ") || line.starts_with("TLS TLSv1.1 "),
            "{line}"
        );
    }

    // Pre-4.1 password hashes (`mysql_old_password`, the server's auth switch for such a user):
    // only the Old variant sends the old scramble; the others refuse the downgrade.
    let mut oldpw = profile(server.port, SslMode::Disable, old);
    oldpw.user = Some("oldpw".into());
    open(&oldpw)
        .await
        .unwrap_or_else(|e| panic!("Old MySQL with a mysql_old_password login: {e}"));
    assert_eq!(server.next_line(), "PLAIN");
    for version in [
        None,
        Some(MysqlVersion::Default),
        Some(MysqlVersion::Mariadb),
        Some(MysqlVersion::Singlestore),
    ] {
        oldpw.options.mysql_version = version;
        let err = open(&oldpw)
            .await
            .expect_err("only Old MySQL may use mysql_old_password");
        println!("{version:?} with a mysql_old_password login: {err}");
        assert!(err.to_ascii_lowercase().contains("old"), "{err}");
        assert_eq!(server.next_line(), "PLAIN");
    }

    // The default driver (rustls) speaks TLS 1.2 and up only: the handshake fails. With PREFERRED
    // it falls back to a clear-text connection, which this server (it insists on TLS after the
    // SSLRequest) never accepts; with REQUIRED the TLS error is the answer.
    let p = profile(server.port, SslMode::Require, None);
    let err = open(&p)
        .await
        .expect_err("rustls must not negotiate TLS 1.1");
    println!("Default variant against a TLS 1.1 server: {err}");
    assert!(
        err.to_ascii_lowercase().contains("tls") || err.to_ascii_lowercase().contains("protocol"),
        "{err}"
    );
}

#[tokio::test]
async fn conn4_a_clear_text_password_goes_through_an_ssh_tunnel_but_not_over_plain_tcp() {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();
    let server = match FakeServer::start() {
        Ok(server) => server,
        Err(why) => {
            println!("skipped: {why}");
            return;
        }
    };
    let tunnel = || Some(("127.0.0.1".to_string(), server.port));
    let mut p = profile(server.port, SslMode::Disable, None);
    p.user = Some("clearpw".into());
    p.options.mysql_cleartext_plugin = Some(true);

    // Plain TCP, no TLS, no tunnel: refused, and the password never leaves.
    let err = open_via(&p, None)
        .await
        .expect_err("plain TCP must not carry a clear-text password");
    println!("direct TCP: {err}");
    assert!(
        err.contains("clear text") && err.contains("SSH tunnel"),
        "{err}"
    );
    assert_eq!(server.next_line(), "PLAIN");

    // The same profile through an SSH tunnel (SSL still DISABLED: SSH encrypts the connection).
    open_via(&p, tunnel())
        .await
        .unwrap_or_else(|e| panic!("a clear-text password through an SSH tunnel: {e}"));
    assert_eq!(server.next_line(), "PLAIN");
    assert_eq!(server.next_line(), "CLEAR secret");

    // Not asked for: refused even through the tunnel (the option is the user's consent).
    p.options.mysql_cleartext_plugin = None;
    let err = open_via(&p, tunnel())
        .await
        .expect_err("without Enable CLEARTEXT_PLUGIN the password stays home");
    assert!(err.contains("Enable CLEARTEXT_PLUGIN"), "{err}");
    assert_eq!(server.next_line(), "PLAIN");
}
