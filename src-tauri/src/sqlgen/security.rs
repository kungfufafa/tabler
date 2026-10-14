//! Read-only queries behind the status bar's details popover: whether the session is encrypted
//! (TLS version and cipher) and its character set. Fixed text, no identifiers or literals.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;

use super::Dialect;
use crate::types::query::ResultSet;
use crate::types::security::SessionDetails;
use crate::types::value::Value;

/// The session's TLS state, one row: PostgreSQL `ssl, version, cipher` (`pg_stat_ssl`, 9.5+);
/// MySQL / MariaDB `Variable_name, Value` rows of `Ssl_version` / `Ssl_cipher`. None for SQLite.
pub fn tls_sql(dialect: Dialect) -> Option<&'static str> {
    match dialect {
        Dialect::Postgres => Some(
            "SELECT ssl, version, cipher FROM pg_catalog.pg_stat_ssl WHERE pid = pg_catalog.pg_backend_pid()",
        ),
        Dialect::Mysql => {
            Some("SHOW SESSION STATUS WHERE Variable_name IN ('Ssl_version', 'Ssl_cipher')")
        }
        Dialect::Sqlite => None,
    }
}

/// The session's character set, one row with one column.
pub fn encoding_sql(dialect: Dialect) -> &'static str {
    match dialect {
        Dialect::Postgres => "SELECT pg_catalog.current_setting('client_encoding')",
        Dialect::Mysql => "SELECT @@character_set_connection",
        Dialect::Sqlite => "PRAGMA encoding",
    }
}

/// A textual cell (`SHOW` output can come back as bytes); None for NULL and empty text.
fn text(v: &Value) -> Option<String> {
    let s = match v {
        Value::Text(s) | Value::Enum(s) => s.clone(),
        Value::Other(o) => o.text.clone(),
        Value::Bytes(b) => String::from_utf8(B64.decode(&b.base64).ok()?).ok()?,
        _ => return None,
    };
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Fills the TLS fields of `details` from the rows of `tls_sql`.
pub fn read_tls(dialect: Dialect, rows: &ResultSet, details: &mut SessionDetails) {
    match dialect {
        Dialect::Postgres => {
            let Some(row) = rows.rows.first() else {
                return;
            };
            details.tls = matches!(row.first(), Some(Value::Bool(true)));
            if details.tls {
                details.tls_version = row.get(1).and_then(text);
                details.tls_cipher = row.get(2).and_then(text);
            }
        }
        Dialect::Mysql => {
            for row in &rows.rows {
                let (Some(name), value) = (row.first().and_then(text), row.get(1).and_then(text))
                else {
                    continue;
                };
                match name.to_ascii_lowercase().as_str() {
                    "ssl_version" => details.tls_version = value,
                    "ssl_cipher" => details.tls_cipher = value,
                    _ => {}
                }
            }
            // An unencrypted session reports both as empty strings.
            details.tls = details.tls_cipher.is_some() || details.tls_version.is_some();
        }
        Dialect::Sqlite => {}
    }
}

/// The character set from the row of `encoding_sql`.
pub fn read_encoding(rows: &ResultSet) -> Option<String> {
    rows.rows.first()?.first().and_then(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(rows: Vec<Vec<Value>>) -> ResultSet {
        ResultSet {
            columns: Vec::new(),
            rows,
            truncated: false,
        }
    }
    fn t(s: &str) -> Value {
        Value::Text(s.into())
    }

    #[test]
    fn has_queries_per_dialect() {
        assert!(tls_sql(Dialect::Postgres).unwrap().contains("pg_stat_ssl"));
        assert!(
            tls_sql(Dialect::Mysql)
                .unwrap()
                .starts_with("SHOW SESSION STATUS")
        );
        assert_eq!(tls_sql(Dialect::Sqlite), None);
        assert_eq!(encoding_sql(Dialect::Sqlite), "PRAGMA encoding");
    }

    #[test]
    fn reads_postgres_tls() {
        let mut d = SessionDetails::default();
        let rows = set(vec![vec![
            Value::Bool(true),
            t("TLSv1.3"),
            t("TLS_AES_256_GCM_SHA384"),
        ]]);
        read_tls(Dialect::Postgres, &rows, &mut d);
        assert!(d.tls);
        assert_eq!(d.tls_version.as_deref(), Some("TLSv1.3"));
        assert_eq!(d.tls_cipher.as_deref(), Some("TLS_AES_256_GCM_SHA384"));

        let mut plain = SessionDetails::default();
        let rows = set(vec![vec![Value::Bool(false), Value::Null, Value::Null]]);
        read_tls(Dialect::Postgres, &rows, &mut plain);
        assert_eq!(plain, SessionDetails::default());
        read_tls(Dialect::Postgres, &set(vec![]), &mut plain);
        assert!(!plain.tls);
    }

    #[test]
    fn reads_mysql_tls() {
        let mut d = SessionDetails::default();
        let rows = set(vec![
            vec![t("Ssl_cipher"), t("TLS_AES_128_GCM_SHA256")],
            vec![t("Ssl_version"), t("TLSv1.3")],
        ]);
        read_tls(Dialect::Mysql, &rows, &mut d);
        assert!(d.tls);
        assert_eq!(d.tls_version.as_deref(), Some("TLSv1.3"));

        let mut plain = SessionDetails::default();
        let rows = set(vec![
            vec![t("Ssl_cipher"), t("")],
            vec![t("Ssl_version"), t("")],
        ]);
        read_tls(Dialect::Mysql, &rows, &mut plain);
        assert!(!plain.tls);
        assert_eq!(plain.tls_cipher, None);

        // SHOW output as bytes.
        let bytes = |s: &str| {
            Value::Bytes(crate::types::value::BytesValue {
                base64: B64.encode(s),
                len: s.len() as u64,
                truncated: false,
            })
        };
        let mut b = SessionDetails::default();
        read_tls(
            Dialect::Mysql,
            &set(vec![vec![bytes("Ssl_cipher"), bytes("DHE-RSA-AES256-SHA")]]),
            &mut b,
        );
        assert!(b.tls);
        assert_eq!(b.tls_cipher.as_deref(), Some("DHE-RSA-AES256-SHA"));
    }

    #[test]
    fn reads_the_encoding() {
        assert_eq!(
            read_encoding(&set(vec![vec![t("UTF8")]])).as_deref(),
            Some("UTF8")
        );
        assert_eq!(read_encoding(&set(vec![vec![Value::Null]])), None);
        assert_eq!(read_encoding(&set(vec![])), None);
    }
}
