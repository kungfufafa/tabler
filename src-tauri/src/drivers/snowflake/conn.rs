//! One Snowflake session over snowflake-connector-rs: sign-in with the session's warehouse,
//! role, database, schema and parameters; statements with `?` binds, read partition by
//! partition into `Value` rows up to a row cap; cancel through the connector's abort request.
//!
//! The session API keeps state across statements (USE, ALTER SESSION, transactions, session
//! variables, temporary tables), as an editor session needs. Dropping a running statement's
//! future does not stop it on the server: cancel and timeouts go through `SnowflakeCancel`.

use std::collections::{BTreeMap, HashMap};
use std::ops::ControlFlow;
use std::time::Duration;

use base64::Engine as _;
use futures::future::BoxFuture;
use snowflake_connector_rs::bind::{BindType, RawBind};
use snowflake_connector_rs::{
    AuthConfig, Client, ClientConfig, EndpointConfig, QueryCanceller, QueryConfig, QueryHandle,
    ResultCursor, Session, SessionConfig, Statement,
};

use super::catalog::Rows;
use super::decode::{ValueRow, result_column};
use super::errors::{connect_error, map_error};
use crate::drivers::CancelAction;
use crate::error::AppError;
use crate::types::query::ResultColumn;
use crate::types::value::Value;

/// How long a statement may run before the connector gives up waiting (the app's own query
/// time-out cancels sooner).
pub const DEFAULT_RESPONSE_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);
/// How long an abort request may take.
const CANCEL_TIMEOUT: Duration = Duration::from_secs(10);

/// Everything a sign-in needs.
pub struct ConnectSpec {
    /// The account identifier (`params::normalize_account`).
    pub account: String,
    pub user: String,
    pub auth: AuthConfig,
    pub warehouse: Option<String>,
    pub role: Option<String>,
    pub database: Option<String>,
    pub schema: Option<String>,
    /// Another endpoint than the account's (PrivateLink, a test server).
    pub base_url: Option<reqwest::Url>,
    pub session_parameters: BTreeMap<String, serde_json::Value>,
    pub response_timeout: Duration,
}

impl std::fmt::Debug for ConnectSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectSpec")
            .field("account", &self.account)
            .field("user", &self.user)
            .field("auth", &self.auth)
            .field("warehouse", &self.warehouse)
            .field("role", &self.role)
            .field("database", &self.database)
            .field("schema", &self.schema)
            .field(
                "base_url",
                &self.base_url.as_ref().map(reqwest::Url::as_str),
            )
            .finish_non_exhaustive()
    }
}

/// The connector's client for `spec` (no request is sent yet).
pub fn client(spec: &ConnectSpec) -> Result<Client, AppError> {
    let mut session = SessionConfig::new();
    let named = |s: &Option<String>| s.as_deref().filter(|s| !s.is_empty()).map(str::to_string);
    if let Some(w) = named(&spec.warehouse) {
        session = session.with_warehouse(w);
    }
    if let Some(r) = named(&spec.role) {
        session = session.with_role(r);
    }
    if let Some(d) = named(&spec.database) {
        session = session.with_database(d);
    }
    if let Some(s) = named(&spec.schema) {
        session = session.with_schema(s);
    }
    let parameters: HashMap<String, serde_json::Value> = spec
        .session_parameters
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    if !parameters.is_empty() {
        session = session.with_session_parameters(parameters);
    }
    let mut config = ClientConfig::new(spec.user.clone(), spec.account.clone(), spec.auth.clone())
        .with_session(session)
        .with_query(
            QueryConfig::new()
                .with_query_response_timeout(spec.response_timeout)
                .with_query_cancel_request_timeout(CANCEL_TIMEOUT),
        );
    if let Some(url) = &spec.base_url {
        // Checked again here, whoever built the spec: plain http only to this machine.
        let url = super::params::base_url(url.as_str())?;
        config = config.with_endpoint(EndpointConfig::custom_base_url(url));
    }
    Client::new(config).map_err(connect_error)
}

/// A signed-in session.
pub struct SnowflakeConn {
    client: Client,
    session: Session,
}

impl std::fmt::Debug for SnowflakeConn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SnowflakeConn").finish_non_exhaustive()
    }
}

/// Signs in.
pub async fn connect(spec: &ConnectSpec) -> Result<SnowflakeConn, AppError> {
    let client = client(spec)?;
    let session = client.create_session().await.map_err(connect_error)?;
    Ok(SnowflakeConn { client, session })
}

/// A statement's result: its columns and rows (DML and DDL answer with rows too: `number of
/// rows inserted`, `status`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Outcome {
    pub query_id: String,
    pub columns: Vec<ResultColumn>,
    pub rows: Vec<Vec<Value>>,
    /// More rows were left unread past the cap.
    pub truncated: bool,
}

impl Outcome {
    /// The rows a DML statement touched: the sum of its `number of rows …` columns (INSERT,
    /// UPDATE, DELETE, MERGE, multi-table INSERT), or None for a statement that reports none.
    pub fn affected(&self) -> Option<u64> {
        let counted: Vec<usize> = self
            .columns
            .iter()
            .enumerate()
            .filter(|(_, c)| c.name.to_ascii_lowercase().starts_with("number of rows"))
            .map(|(i, _)| i)
            .collect();
        if counted.is_empty() {
            return None;
        }
        let row = self.rows.first()?;
        counted.iter().try_fold(0u64, |sum, &i| {
            let n = match row.get(i)? {
                Value::Int(n) => u64::try_from(*n).ok()?,
                v => v.as_text()?.trim().parse().ok()?,
            };
            sum.checked_add(n)
        })
    }

    /// The result as named rows, for the catalog parsers.
    pub fn into_rows(self) -> Rows {
        Rows {
            columns: self.columns.into_iter().map(|c| c.name).collect(),
            rows: self.rows,
        }
    }
}

/// A `Value` as a bind: text for what Snowflake casts from text (decimals, temporal values,
/// JSON, which `PARSE_JSON(?)` reads), typed for numbers, booleans and bytes.
pub fn bind(value: &Value) -> Result<RawBind, AppError> {
    Ok(match value {
        Value::Null => RawBind::null(BindType::Text),
        Value::Bool(b) => RawBind::new(BindType::Boolean, if *b { "true" } else { "false" }),
        Value::Int(n) => RawBind::new(BindType::Fixed, n.to_string()),
        Value::BigInt(s) => {
            let digits = s.strip_prefix('-').unwrap_or(s);
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return Err(AppError::InvalidInput(format!("{s} is no integer")));
            }
            RawBind::new(BindType::Fixed, s.clone())
        }
        Value::Float(f) => RawBind::new(BindType::Real, format!("{f:?}")),
        Value::Bytes(b) => {
            if b.truncated {
                return Err(AppError::InvalidInput(
                    "a binary value that was only partly loaded cannot be written back".into(),
                ));
            }
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&b.base64)
                .map_err(|e| AppError::InvalidInput(format!("binary value: {e}")))?;
            let hex: String = bytes.iter().map(|b| format!("{b:02X}")).collect();
            RawBind::new(BindType::Binary, hex)
        }
        Value::Array(items) => RawBind::new(BindType::Text, json_array(items)?),
        Value::Default => {
            return Err(AppError::InvalidInput(
                "DEFAULT is a keyword, not a value to bind".into(),
            ));
        }
        other => RawBind::new(BindType::Text, other.as_text().unwrap_or_default()),
    })
}

/// A VECTOR's elements as the JSON array text Snowflake casts.
fn json_array(items: &[Value]) -> Result<String, AppError> {
    let parts: Result<Vec<String>, AppError> = items
        .iter()
        .map(|v| match v {
            Value::Int(n) => Ok(n.to_string()),
            Value::Float(f) => Ok(format!("{f:?}")),
            Value::BigInt(s) | Value::Decimal(s) => Ok(s.clone()),
            Value::Null => Ok("null".into()),
            other => Ok(serde_json::Value::String(other.as_text().unwrap_or_default()).to_string()),
        })
        .collect();
    Ok(format!("[{}]", parts?.join(",")))
}

/// The statement with its binds.
pub fn statement(
    sql: &str,
    params: &[Value],
) -> Result<Statement<snowflake_connector_rs::PositionalBinds>, AppError> {
    let mut stmt = Statement::new_positional(sql.to_string());
    for p in params {
        stmt = stmt.bind(bind(p)?);
    }
    Ok(stmt)
}

/// Stops one statement on the server (the connector's abort request).
#[derive(Clone)]
pub struct SnowflakeCancel(QueryCanceller);

impl SnowflakeCancel {
    pub async fn cancel(&self) -> Result<(), AppError> {
        self.0
            .cancel()
            .await
            .map(|_| ())
            .map_err(|e| map_error(e, None))
    }
}

impl std::fmt::Debug for SnowflakeCancel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("SnowflakeCancel")
            .field(&self.0.request_id())
            .finish()
    }
}

impl CancelAction for SnowflakeCancel {
    fn fire(&self) -> BoxFuture<'static, Result<(), AppError>> {
        let this = self.clone();
        Box::pin(async move { this.cancel().await })
    }

    fn label(&self) -> &'static str {
        "abort request"
    }
}

impl SnowflakeConn {
    /// A statement ready to run, and what cancels it (before or while it runs).
    pub fn prepare(
        &self,
        sql: &str,
        params: &[Value],
    ) -> Result<(QueryHandle, SnowflakeCancel), AppError> {
        let handle = self
            .session
            .query_handle(statement(sql, params)?)
            .map_err(|e| map_error(e, Some(sql)))?;
        let cancel = SnowflakeCancel(handle.canceller());
        Ok((handle, cancel))
    }

    /// Runs a prepared statement and reads up to `max_rows` rows.
    pub async fn fetch(
        handle: QueryHandle,
        sql: &str,
        max_rows: Option<usize>,
    ) -> Result<Outcome, AppError> {
        let mut out = Outcome::default();
        let mut cursor = handle
            .execute()
            .await
            .map_err(|e| map_error(e, Some(sql)))?;
        out.query_id = cursor.query_id().to_string();
        out.columns = cursor
            .schema()
            .columns()
            .iter()
            .map(result_column)
            .collect();
        let rows = &mut out.rows;
        let truncated = stream(&mut cursor, sql, |batch| {
            for row in batch {
                if max_rows.is_some_and(|m| rows.len() >= m) {
                    return ControlFlow::Break(());
                }
                rows.push(row);
            }
            ControlFlow::Continue(())
        })
        .await?;
        out.truncated = truncated;
        Ok(out)
    }

    /// Runs one statement (no cancel handle kept).
    pub async fn query(
        &self,
        sql: &str,
        params: &[Value],
        max_rows: Option<usize>,
    ) -> Result<Outcome, AppError> {
        let (handle, _) = self.prepare(sql, params)?;
        Self::fetch(handle, sql, max_rows).await
    }

    /// Free: answered by the cloud services layer, no warehouse resumes.
    pub async fn ping(&self) -> Result<(), AppError> {
        self.query(crate::sqlgen::dialect::snowflake::PING, &[], Some(1))
            .await
            .map(|_| ())
    }

    /// Signs in again (an expired session); the session's state is the caller's to re-apply.
    pub async fn relogin(&mut self) -> Result<(), AppError> {
        self.session = self.client.create_session().await.map_err(connect_error)?;
        Ok(())
    }
}

/// Reads a result partition by partition into `on_rows` until it breaks; whether rows were
/// left unread.
pub async fn stream(
    cursor: &mut ResultCursor,
    sql: &str,
    mut on_rows: impl FnMut(Vec<Vec<Value>>) -> ControlFlow<()>,
) -> Result<bool, AppError> {
    while let Some(table) = cursor
        .next_table()
        .await
        .map_err(|e| map_error(e, Some(sql)))?
    {
        let rows = table
            .rows::<ValueRow>()
            .map_err(|e| map_error(e, Some(sql)))?
            .map(|r| r.map(|row| row.0))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| map_error(e, Some(sql)))?;
        if on_rows(rows).is_break() {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::snowflake::auth::{AuthInputs, KeyFormat, PrivateKey, auth_config};
    use crate::drivers::snowflake::params::SnowflakeAuth;
    use crate::drivers::snowflake::testing::{
        Answer, ENCRYPTED_KEY, FakeColumn, FakeSnowflake, KEY, KEY_PASSPHRASE, Login,
    };
    use crate::types::schema::ValueKind;

    const ACCOUNT: &str = "myorg-acct.us-east-1";
    const USER: &str = "tabler_user";

    fn spec(fake: &FakeSnowflake, auth: AuthConfig) -> ConnectSpec {
        ConnectSpec {
            account: ACCOUNT.into(),
            user: USER.into(),
            auth,
            warehouse: Some("COMPUTE_WH".into()),
            role: Some("ANALYST".into()),
            database: Some("DB".into()),
            schema: Some("PUBLIC".into()),
            base_url: Some(reqwest::Url::parse(&fake.url()).unwrap()),
            session_parameters: BTreeMap::from([("QUERY_TAG".to_string(), "tabler".into())]),
            response_timeout: Duration::from_secs(30),
        }
    }

    fn key_auth(pem: &str, encrypted: bool) -> AuthConfig {
        let inputs = AuthInputs {
            key: Some(PrivateKey {
                pem: pem.into(),
                format: if encrypted {
                    KeyFormat::EncryptedPkcs8
                } else {
                    KeyFormat::Pkcs8
                },
            }),
            passphrase: encrypted.then(|| KEY_PASSPHRASE.to_string()),
            ..AuthInputs::default()
        };
        auth_config(SnowflakeAuth::KeyPair, &inputs).unwrap()
    }

    async fn key_pair_session() -> (FakeSnowflake, SnowflakeConn) {
        crate::testkit::install_crypto();
        let fake = FakeSnowflake::start(ACCOUNT, USER, Login::key_pair()).await;
        let conn = connect(&spec(&fake, key_auth(KEY, false))).await.unwrap();
        (fake, conn)
    }

    #[tokio::test]
    async fn plain_http_reaches_only_this_machine() {
        crate::testkit::install_crypto();
        let fake = FakeSnowflake::start(ACCOUNT, USER, Login::key_pair()).await;
        let mut s = spec(&fake, key_auth(KEY, false));
        assert!(client(&s).is_ok());
        // A spec built without `params::base_url` still never signs in over clear text.
        s.base_url = Some(reqwest::Url::parse("http://example.com/").unwrap());
        let err = connect(&s).await.unwrap_err();
        assert!(matches!(err, AppError::InvalidInput(_)), "{err:?}");
        assert!(fake.logins().is_empty());
    }

    #[tokio::test]
    async fn key_pair_sign_in_sends_the_claims_snowflake_checks() {
        crate::testkit::install_crypto();
        for (pem, encrypted) in [(KEY, false), (ENCRYPTED_KEY, true)] {
            let fake = FakeSnowflake::start(ACCOUNT, USER, Login::key_pair()).await;
            connect(&spec(&fake, key_auth(pem, encrypted)))
                .await
                .unwrap();
            let logins = fake.logins();
            assert_eq!(logins.len(), 1);
            let login = &logins[0];
            // The fake verified the RS256 signature with the public key; the claims are these.
            let claims = &login.claims;
            let fp = crate::drivers::snowflake::testing::KEY_FINGERPRINT;
            assert_eq!(claims["iss"], format!("MYORG-ACCT.TABLER_USER.{fp}"));
            assert_eq!(claims["sub"], "MYORG-ACCT.TABLER_USER");
            let iat = claims["iat"].as_i64().unwrap();
            let exp = claims["exp"].as_i64().unwrap();
            assert!(exp > iat && exp - iat <= 3600, "{claims}");
            assert!((iat - chrono::Utc::now().timestamp()).abs() < 300);
            assert_eq!(login.body["data"]["ACCOUNT_NAME"], ACCOUNT);
            assert_eq!(login.body["data"]["LOGIN_NAME"], USER);
            assert_eq!(login.body["data"]["AUTHENTICATOR"], "SNOWFLAKE_JWT");
            assert_eq!(
                login.body["data"]["SESSION_PARAMETERS"]["QUERY_TAG"],
                "tabler"
            );
            for part in [
                "warehouse=COMPUTE_WH",
                "roleName=ANALYST",
                "databaseName=DB",
                "schemaName=PUBLIC",
            ] {
                assert!(login.query.contains(part), "{}", login.query);
            }
        }
    }

    #[tokio::test]
    async fn a_key_snowflake_does_not_know_is_an_auth_error() {
        crate::testkit::install_crypto();
        let other = Login::KeyPair {
            fingerprint: "SHA256:AAAA".into(),
            public_key: crate::drivers::snowflake::testing::public_key_der(),
        };
        let fake = FakeSnowflake::start(ACCOUNT, USER, other).await;
        let err = connect(&spec(&fake, key_auth(KEY, false)))
            .await
            .unwrap_err();
        assert!(
            matches!(&err, AppError::Auth(m) if m.contains("JWT")),
            "{err:?}"
        );
        // A wrong passphrase never reaches the server.
        let wrong = AuthInputs {
            key: Some(PrivateKey {
                pem: ENCRYPTED_KEY.into(),
                format: KeyFormat::EncryptedPkcs8,
            }),
            passphrase: Some("nope".into()),
            ..AuthInputs::default()
        };
        let auth = auth_config(SnowflakeAuth::KeyPair, &wrong).unwrap();
        let fake = FakeSnowflake::start(ACCOUNT, USER, Login::key_pair()).await;
        let err = connect(&spec(&fake, auth)).await.unwrap_err();
        assert!(matches!(err, AppError::Auth(_)), "{err:?}");
        assert!(fake.logins().is_empty());
    }

    #[tokio::test]
    async fn tokens_passwords_and_passcodes() {
        crate::testkit::install_crypto();
        let fake = FakeSnowflake::start(ACCOUNT, USER, Login::OAuth("oauth-token".into())).await;
        let ok = AuthInputs {
            secret: Some("oauth-token".into()),
            ..AuthInputs::default()
        };
        connect(&spec(
            &fake,
            auth_config(SnowflakeAuth::Oauth, &ok).unwrap(),
        ))
        .await
        .unwrap();
        assert_eq!(fake.logins()[0].body["data"]["AUTHENTICATOR"], "OAUTH");

        // A PAT goes where the password goes.
        let fake = FakeSnowflake::start(
            ACCOUNT,
            USER,
            Login::Password {
                password: "pat-123".into(),
                passcode: None,
            },
        )
        .await;
        let pat = AuthInputs {
            secret: Some("pat-123".into()),
            ..AuthInputs::default()
        };
        connect(&spec(&fake, auth_config(SnowflakeAuth::Pat, &pat).unwrap()))
            .await
            .unwrap();
        let wrong = AuthInputs {
            secret: Some("pat-999".into()),
            ..AuthInputs::default()
        };
        let err = connect(&spec(
            &fake,
            auth_config(SnowflakeAuth::Pat, &wrong).unwrap(),
        ))
        .await
        .unwrap_err();
        assert!(matches!(err, AppError::Auth(_)), "{err:?}");

        let fake = FakeSnowflake::start(
            ACCOUNT,
            USER,
            Login::Password {
                password: "pw".into(),
                passcode: Some("123456".into()),
            },
        )
        .await;
        let mfa = AuthInputs {
            secret: Some("pw".into()),
            passcode: Some("123456".into()),
            mfa: true,
            ..AuthInputs::default()
        };
        connect(&spec(
            &fake,
            auth_config(SnowflakeAuth::Password, &mfa).unwrap(),
        ))
        .await
        .unwrap();
        let body = &fake.logins()[0].body;
        assert_eq!(body["data"]["PASSCODE"], "123456");
        assert_eq!(body["data"]["EXT_AUTHN_DUO_METHOD"], "passcode");
    }

    fn every_type() -> Vec<FakeColumn> {
        vec![
            FakeColumn::new("N", "fixed").precision(38, 0),
            FakeColumn::new("D", "fixed").precision(10, 2),
            FakeColumn::new("F", "real"),
            FakeColumn::new("S", "text").length(16_777_216),
            FakeColumn::new("B", "binary").length(8),
            FakeColumn::new("OK", "boolean"),
            FakeColumn::new("DAY", "date"),
            FakeColumn::new("T", "time").scale(3),
            FakeColumn::new("NTZ", "timestamp_ntz").scale(9),
            FakeColumn::new("LTZ", "timestamp_ltz").scale(3),
            FakeColumn::new("TZ", "timestamp_tz").scale(3),
            FakeColumn::new("V", "variant"),
            FakeColumn::new("O", "object"),
            FakeColumn::new("A", "array"),
            FakeColumn::new("G", "geography"),
            FakeColumn::new("VEC", "vector"),
        ]
    }

    #[tokio::test]
    async fn every_type_decodes_across_partitions() {
        let (fake, conn) = key_pair_session().await;
        let row = |n: &str| -> Vec<Option<String>> {
            [
                Some(n),
                Some("-12.50"),
                Some("1.5"),
                Some("日本語"),
                Some("00FF"),
                Some("1"),
                Some("19844"),
                Some("49500.123"),
                Some("1714571100.123456789"),
                Some("1714571100.000"),
                Some("1714571100.000 1860"),
                Some("{\n  \"a\": 1\n}"),
                Some("{}"),
                Some("[1,2]"),
                Some("{\"type\":\"Point\",\"coordinates\":[1,2]}"),
                Some("[1.5,2.5]"),
            ]
            .into_iter()
            .map(|c| c.map(str::to_string))
            .collect()
        };
        let nulls = vec![None; 16];
        fake.answer(
            "SELECT * FROM all_types",
            Answer::rows(every_type(), vec![row("9007199254740993"), nulls.clone()])
                .chunk(vec![row("1"), row("2")])
                .chunk(vec![row("3")]),
        );
        let out = conn
            .query("SELECT * FROM all_types", &[], None)
            .await
            .unwrap();
        assert_eq!(out.rows.len(), 5);
        assert!(!out.truncated);
        let kinds: Vec<ValueKind> = out.columns.iter().map(|c| c.kind).collect();
        assert_eq!(kinds[0], ValueKind::BigInt);
        assert_eq!(out.columns[0].data_type, "NUMBER(38,0)");
        assert_eq!(out.columns[10].data_type, "TIMESTAMP_TZ(3)");
        let r = &out.rows[0];
        assert_eq!(r[0], Value::BigInt("9007199254740993".into()));
        assert_eq!(r[1], Value::Decimal("-12.50".into()));
        assert_eq!(r[2], Value::Float(1.5));
        assert_eq!(r[3], Value::Text("日本語".into()));
        assert_eq!(r[4], Value::from_bytes(&[0, 255]));
        assert_eq!(r[5], Value::Bool(true));
        assert_eq!(r[6], Value::Date("2024-05-01".into()));
        assert_eq!(r[7], Value::Time("13:45:00.123".into()));
        assert_eq!(
            r[8],
            Value::Timestamp("2024-05-01 13:45:00.123456789".into())
        );
        assert_eq!(
            r[9],
            Value::TimestampTz("2024-05-01 13:45:00.000 +00:00".into())
        );
        assert_eq!(
            r[10],
            Value::TimestampTz("2024-05-01 20:45:00.000 +07:00".into())
        );
        assert_eq!(r[11], Value::Json("{\n  \"a\": 1\n}".into()));
        assert_eq!(
            r[14],
            Value::other("geography", "{\"type\":\"Point\",\"coordinates\":[1,2]}")
        );
        assert_eq!(
            r[15],
            Value::Array(vec![Value::Float(1.5), Value::Float(2.5)])
        );
        assert!(out.rows[1].iter().all(Value::is_null));
        assert_eq!(out.rows[2][0], Value::Int(1));
        assert_eq!(out.rows[4][0], Value::Int(3));

        // The cap stops reading further partitions.
        let capped = conn
            .query("SELECT * FROM all_types", &[], Some(3))
            .await
            .unwrap();
        assert_eq!(capped.rows.len(), 3);
        assert!(capped.truncated);
    }

    #[tokio::test]
    async fn binds_dml_counts_and_errors() {
        let (fake, conn) = key_pair_session().await;
        fake.answer(
            "UPDATE t SET a = ? WHERE id = ?",
            Answer::rows(
                vec![
                    FakeColumn::new("number of rows updated", "fixed").precision(38, 0),
                    FakeColumn::new("number of multi-joined rows updated", "fixed")
                        .precision(38, 0),
                ],
                vec![vec![Some("2".into()), Some("0".into())]],
            ),
        );
        let out = conn
            .query(
                "UPDATE t SET a = ? WHERE id = ?",
                &[Value::Text("x".into()), Value::Int(7)],
                None,
            )
            .await
            .unwrap();
        assert_eq!(out.affected(), Some(2));
        let sent = fake.queries();
        let bindings = &sent.last().unwrap().body["bindings"];
        assert_eq!(bindings["1"]["type"], "TEXT");
        assert_eq!(bindings["1"]["value"], "x");
        assert_eq!(bindings["2"]["type"], "FIXED");
        assert_eq!(bindings["2"]["value"], "7");
        // The sessions's token goes with every statement.
        assert_eq!(
            sent.last().unwrap().header("authorization"),
            Some(format!(
                "Snowflake Token=\"{}\"",
                crate::drivers::snowflake::testing::SESSION_TOKEN
            ))
            .as_deref()
        );

        fake.answer(
            "CREATE TABLE t (a INT)",
            Answer::rows(
                vec![FakeColumn::new("status", "text")],
                vec![vec![Some("Table T successfully created.".into())]],
            ),
        );
        let ddl = conn
            .query("CREATE TABLE t (a INT)", &[], None)
            .await
            .unwrap();
        assert_eq!(ddl.affected(), None);

        fake.answer(
            "SELECT 1\nFORM t",
            Answer::error(
                "001003",
                "SQL compilation error:\nsyntax error line 2 at position 0 unexpected 'FORM'.",
            ),
        );
        let err = conn.query("SELECT 1\nFORM t", &[], None).await.unwrap_err();
        match err {
            AppError::Db {
                vendor_code,
                position,
                message,
                ..
            } => {
                assert_eq!(vendor_code, Some(1003));
                assert!(message.contains("unexpected 'FORM'"));
                let p = position.unwrap();
                assert_eq!((p.line, p.column), (2, 1));
            }
            other => panic!("{other:?}"),
        }
        // A statement the fake does not know is an error too, not a hang.
        assert!(conn.query("SELECT nothing", &[], None).await.is_err());
        assert!(
            bind(&Value::Bytes(crate::types::value::BytesValue {
                base64: "AA==".into(),
                len: 100_000,
                truncated: true,
            }))
            .is_err()
        );
        assert!(bind(&Value::BigInt("12a".into())).is_err());
        assert!(bind(&Value::Default).is_err());
    }

    #[tokio::test]
    async fn cancel_stops_the_statement_on_the_server() {
        let (fake, conn) = key_pair_session().await;
        fake.answer("SELECT SYSTEM$WAIT(60)", Answer::Hang);
        let (handle, cancel) = conn.prepare("SELECT SYSTEM$WAIT(60)", &[]).unwrap();
        let run = tokio::spawn(SnowflakeConn::fetch(handle, "SELECT SYSTEM$WAIT(60)", None));
        // Wait until the server has the statement, then cancel through the CancelAction.
        fake.wait_for_queries(1).await;
        let action: std::sync::Arc<dyn CancelAction> = std::sync::Arc::new(cancel);
        action.fire().await.unwrap();
        let err = run.await.unwrap().unwrap_err();
        assert!(matches!(err, AppError::Cancelled), "{err:?}");
        // The abort names this statement's request (the fake's hung statement answers only its
        // own abort).
        let hung = fake
            .queries()
            .iter()
            .find(|q| q.body["sqlText"] == "SELECT SYSTEM$WAIT(60)")
            .and_then(|q| q.param("requestId"))
            .unwrap();
        assert!(!hung.is_empty());
        assert_eq!(fake.aborts(), vec![hung]);
    }

    #[tokio::test]
    async fn slow_answers_poll_and_time_out() {
        crate::testkit::install_crypto();
        let fake = FakeSnowflake::start(ACCOUNT, USER, Login::key_pair()).await;
        let mut s = spec(&fake, key_auth(KEY, false));
        s.response_timeout = Duration::from_millis(500);
        let conn = connect(&s).await.unwrap();
        // 333334 first, then the result at its URL.
        fake.answer(
            "SELECT 42",
            Answer::Async(Box::new(Answer::rows(
                vec![FakeColumn::new("42", "fixed").precision(2, 0)],
                vec![vec![Some("42".into())]],
            ))),
        );
        let out = conn.query("SELECT 42", &[], None).await.unwrap();
        assert_eq!(out.rows, vec![vec![Value::Int(42)]]);
        fake.answer(
            "SELECT slow",
            Answer::Slow(Duration::from_secs(5), Box::new(Answer::error("1", "late"))),
        );
        let err = conn.query("SELECT slow", &[], None).await.unwrap_err();
        assert!(matches!(err, AppError::Timeout), "{err:?}");
    }

    #[tokio::test]
    async fn an_expired_session_signs_in_again() {
        let (fake, mut conn) = key_pair_session().await;
        fake.answer(
            "SELECT 1",
            Answer::rows(
                vec![FakeColumn::new("1", "fixed").precision(1, 0)],
                vec![vec![Some("1".into())]],
            ),
        );
        conn.ping().await.unwrap();
        fake.expire_session();
        let err = conn.ping().await.unwrap_err();
        assert!(matches!(err, AppError::NotConnected), "{err:?}");
        conn.relogin().await.unwrap();
        conn.ping().await.unwrap();
        assert_eq!(fake.logins().len(), 2);
    }
}
