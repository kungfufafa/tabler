//! SQL of connection options: the SQLCipher key pragmas, the connection encoding and the
//! PostgreSQL server settings of a profile's extra connection parameters.

use super::Dialect;
use super::quote::MysqlLiteralMode;

/// `PRAGMA key = '…'`, the first statement on a SQLCipher connection. A raw key written the
/// SQLCipher way (`x'…'`, 64 or 96 hex digits: key, or key + salt) is passed as such; anything
/// else is a passphrase.
pub fn sqlite_key(key: &str) -> String {
    let raw = key
        .strip_prefix("x'")
        .or_else(|| key.strip_prefix("X'"))
        .and_then(|k| k.strip_suffix('\''))
        .filter(|hex| {
            (hex.len() == 64 || hex.len() == 96) && hex.chars().all(|c| c.is_ascii_hexdigit())
        });
    match raw {
        // A double-quoted string holding the blob literal, as SQLCipher documents it.
        Some(hex) => format!("PRAGMA key = \"x'{hex}'\";"),
        None => format!(
            "PRAGMA key = {};",
            Dialect::Sqlite.quote_literal_with(key, MysqlLiteralMode::BackslashEscapes)
        ),
    }
}

/// `PRAGMA cipher_compatibility = N` (SQLCipher 1–4), right after the key.
pub fn sqlite_cipher_compatibility(version: u8) -> String {
    format!("PRAGMA cipher_compatibility = {version};")
}

/// Reads the schema: the first statement that needs the key, so a wrong one fails here.
pub const SQLITE_KEY_CHECK: &str = "SELECT count(*) FROM sqlite_master";

/// Attaches an encrypted database file (`?1`) under the key `?2` as `tabler_copy`.
pub const SQLITE_ATTACH_ENCRYPTED_COPY: &str = "ATTACH DATABASE ?1 AS tabler_copy KEY ?2";
/// SQLCipher's export of `main` into the attached copy (schema, data, user version).
pub const SQLITE_EXPORT_TO_COPY: &str = "SELECT sqlcipher_export('tabler_copy')";
pub const SQLITE_DETACH_COPY: &str = "DETACH DATABASE tabler_copy";

/// `PRAGMA cipher_compatibility` of the attached copy, before anything is written to it.
pub fn sqlite_copy_cipher_compatibility(version: u8) -> String {
    format!("PRAGMA tabler_copy.cipher_compatibility = {version};")
}

/// A connection encoding name as typed (`utf8mb4`, `LATIN1`, `WIN1252`, `EUC_JP`…): letters,
/// digits, `_` and `-` only, so it can never be anything but a name.
pub fn valid_encoding(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 40
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// MySQL `SET NAMES '<charset>'` (character_set_client, _connection and _results).
pub fn mysql_set_names(charset: &str) -> String {
    format!(
        "SET NAMES {}",
        Dialect::Mysql.quote_literal_with(charset, MysqlLiteralMode::BackslashEscapes)
    )
}

/// The live session's encoding (View Using Encoding…).
pub const PG_SHOW_CLIENT_ENCODING: &str = "SHOW client_encoding";
pub const MYSQL_SHOW_CHARSET: &str = "SELECT @@character_set_client";

/// PostgreSQL `SET client_encoding = '<name>'`.
pub fn pg_client_encoding(name: &str) -> String {
    format!(
        "SET client_encoding = {};",
        Dialect::Postgres.quote_literal_with(name, MysqlLiteralMode::BackslashEscapes)
    )
}

/// A server setting of the session (`set_config(name, value, false)`): the value is read the way
/// `SET` reads it, list settings such as `search_path` included.
pub fn pg_set_config(name: &str, value: &str) -> String {
    let lit = |s: &str| Dialect::Postgres.quote_literal_with(s, MysqlLiteralMode::BackslashEscapes);
    format!("SELECT set_config({}, {}, false);", lit(name), lit(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_key_pragmas() {
        assert_eq!(sqlite_key("secret"), "PRAGMA key = 'secret';");
        assert_eq!(sqlite_key("it's"), "PRAGMA key = 'it''s';");
        let hex = "2DD29CA851E7B56E4697B0E1F08507293D761A05CE4D1B628663F411A8086D99";
        assert_eq!(
            sqlite_key(&format!("x'{hex}'")),
            format!("PRAGMA key = \"x'{hex}'\";")
        );
        // Not a raw key: too short, or not hex.
        assert_eq!(sqlite_key("x'abc'"), "PRAGMA key = 'x''abc''';");
        assert_eq!(
            sqlite_cipher_compatibility(3),
            "PRAGMA cipher_compatibility = 3;"
        );
    }

    #[test]
    fn encodings() {
        assert!(valid_encoding("utf8mb4") && valid_encoding("EUC_JP") && valid_encoding("utf-8"));
        assert!(!valid_encoding("") && !valid_encoding("latin1'; DROP") && !valid_encoding("a b"));
        assert_eq!(mysql_set_names("latin1"), "SET NAMES 'latin1'");
        assert_eq!(
            pg_client_encoding("LATIN1"),
            "SET client_encoding = 'LATIN1';"
        );
    }

    #[test]
    fn pg_settings() {
        assert_eq!(
            pg_set_config("search_path", "app, public"),
            "SELECT set_config('search_path', 'app, public', false);"
        );
        assert_eq!(
            pg_set_config("x.y", "it's \\ here"),
            "SELECT set_config('x.y', E'it''s \\\\ here', false);"
        );
    }
}
