//! MySQL / MariaDB driver integration tests. Gated on `TABLER_TEST_MYSQL_URL`
//! (`mysql://user[:pass]@host:port/db`); every test prints a skip notice and passes when unset.

use std::sync::Arc;

use tabler_lib::drivers::{ConnectTarget, DbSession, TxStatement, connect};
use tabler_lib::types::profile::{ConnectionProfile, Driver, ProfileOptions, SslMode};
use tabler_lib::types::schema::{TableKind, TableRef, ValueKind};
use tabler_lib::types::value::Value;

const ENV: &str = "TABLER_TEST_MYSQL_URL";

fn pct_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Parses `mysql://user[:pass]@host:port/db` into a profile plus the password.
fn profile_from_url(url: &str) -> (ConnectionProfile, Option<String>) {
    let rest = url
        .strip_prefix("mysql://")
        .unwrap_or_else(|| panic!("{ENV} must start with mysql://"));
    let (creds, hostdb) = match rest.rsplit_once('@') {
        Some((c, h)) => (Some(c), h),
        None => (None, rest),
    };
    let (user, pass) = match creds {
        Some(c) => match c.split_once(':') {
            Some((u, p)) => (Some(pct_decode(u)), Some(pct_decode(p))),
            None => (Some(pct_decode(c)), None),
        },
        None => (None, None),
    };
    let (hostport, db) = match hostdb.split_once('/') {
        Some((h, d)) => (h, Some(d.to_string()).filter(|d| !d.is_empty())),
        None => (hostdb, None),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse::<u16>().ok()),
        None => (hostport.to_string(), None),
    };
    let profile = ConnectionProfile {
        id: uuid::Uuid::new_v4(),
        name: "mysql-test".into(),
        color: None,
        group: None,
        driver: Driver::Mysql,
        host: Some(host),
        port,
        user,
        database: db,
        ssl: SslMode::Disable,
        ssl_ca_path: None,
        sqlite_path: None,
        ssh: None,
        options: ProfileOptions {
            connect_timeout_secs: Some(10),
            ..ProfileOptions::default()
        },
        sort_order: 0,
        created_at: String::new(),
        updated_at: String::new(),
        last_connected_at: None,
    };
    (profile, pass)
}

fn install_crypto() {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();
}

/// Connects with the URL from the environment, or returns `None` (after printing why).
async fn session_with(
    customize: impl FnOnce(&mut ConnectionProfile),
) -> Option<(Arc<dyn DbSession>, ConnectionProfile)> {
    let Ok(url) = std::env::var(ENV) else {
        println!("skipped: {ENV} unset");
        return None;
    };
    install_crypto();
    let (mut profile, password) = profile_from_url(&url);
    customize(&mut profile);
    let session = connect(ConnectTarget {
        profile: &profile,
        password: password.as_deref(),
        host_override: None,
    })
    .await
    .unwrap_or_else(|e| panic!("connect failed: {e}"));
    Some((session, profile))
}

async fn session() -> Option<(Arc<dyn DbSession>, ConnectionProfile)> {
    session_with(|_| {}).await
}

fn db_of(profile: &ConnectionProfile) -> String {
    profile
        .database
        .clone()
        .expect("test URL must name a database")
}

fn text(v: &Value) -> String {
    match v {
        Value::Text(s) => s.clone(),
        other => panic!("expected text, got {other:?}"),
    }
}

#[tokio::test]
async fn connects_and_introspects() {
    let Some((s, profile)) = session().await else {
        return;
    };
    let db = db_of(&profile);

    let info = s.server_info().await.unwrap();
    println!(
        "server: {} db={:?} user={:?}",
        info.version, info.current_database, info.current_user
    );
    assert!(
        info.version.starts_with("MySQL ") || info.version.starts_with("MariaDB "),
        "unexpected version string {:?}",
        info.version
    );
    assert_eq!(info.current_database.as_deref(), Some(db.as_str()));
    assert!(info.current_user.is_some());
    s.ping().await.unwrap();

    let dbs = s.list_databases().await.unwrap();
    let mine = dbs
        .iter()
        .find(|d| d.name == db)
        .expect("list_databases contains the test db");
    assert!(mine.is_current);
    assert!(mine.encoding.is_some());
    assert!(dbs.iter().any(|d| d.name == "information_schema"));

    let schemas = s.list_schemas().await.unwrap();
    assert_eq!(schemas.len(), 1);
    assert_eq!(schemas[0].name, db);
    assert!(schemas[0].is_default);

    let tables = s.list_tables(&db).await.unwrap();
    println!("{} tables in {db}", tables.len());
    assert!(
        tables
            .iter()
            .all(|t| t.schema.as_deref() == Some(db.as_str()))
    );

    // Views in information_schema are reported as system views with no size.
    let sys = s.list_tables("information_schema").await.unwrap();
    assert!(sys.iter().any(|t| t.kind == TableKind::SystemView));

    let meta = s.completion_metadata().await.unwrap();
    assert_eq!(meta.default_schema.as_deref(), Some(db.as_str()));
    assert!(!meta.schemas.contains_key("mysql"));

    s.close().await;
}

#[tokio::test]
async fn query_decodes_text_and_binary_protocol() {
    let Some((s, _)) = session().await else {
        return;
    };

    // Text protocol (no params): every cell arrives as bytes and is parsed by column type.
    let rs = s
        .query(
            "SELECT 1 AS i, 'x' AS s, NULL AS n, 1.5 AS f, CAST(9007199254740993 AS SIGNED) AS big",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(
        rs.columns
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["i", "s", "n", "f", "big"]
    );
    assert_eq!(rs.rows.len(), 1);
    let row = &rs.rows[0];
    assert_eq!(row[0], Value::Int(1));
    assert_eq!(row[1], Value::Text("x".into()));
    assert_eq!(row[2], Value::Null);
    // A `1.5` literal is a DECIMAL in MySQL, so it decodes as Decimal (not Float).
    assert_eq!(row[3], Value::Decimal("1.5".into()));
    assert_eq!(rs.columns[3].kind, ValueKind::Decimal);
    assert_eq!(row[4], Value::BigInt("9007199254740993".into()));
    assert_eq!(rs.columns[4].kind, ValueKind::BigInt);
    assert_eq!(rs.columns[2].nullable, Some(true));
    assert_eq!(rs.columns[0].nullable, Some(false));

    // Binary protocol (with params): typed values.
    let rs = s
        .query("SELECT ? + 1 AS v", &[Value::Int(41)])
        .await
        .unwrap();
    assert_eq!(rs.rows[0][0], Value::Int(42));

    let rs = s
        .query(
            "SELECT CAST('2026-01-02' AS DATE) AS d, CAST('2026-01-02 03:04:05' AS DATETIME) AS ts, \
             CAST('-01:02:03' AS TIME) AS t, CAST(1.5 AS DOUBLE) AS f, ? AS p, \
             CAST(? AS UNSIGNED) AS u, X'00FF' AS b",
            &[Value::Text("hi".into()), Value::BigInt("18446744073709551615".into())],
        )
        .await
        .unwrap();
    let row = &rs.rows[0];
    assert_eq!(row[0], Value::Date("2026-01-02".into()));
    assert_eq!(row[1], Value::Timestamp("2026-01-02 03:04:05".into()));
    assert_eq!(row[2], Value::Time("-01:02:03".into()));
    assert_eq!(row[3], Value::Float(1.5));
    assert_eq!(row[4], Value::Text("hi".into()));
    assert_eq!(row[5], Value::BigInt("18446744073709551615".into()));
    match &row[6] {
        Value::Bytes(b) => assert_eq!(b.base64, "AP8="),
        other => panic!("expected bytes, got {other:?}"),
    }
    assert_eq!(rs.columns[6].kind, ValueKind::Bytes);

    // Text protocol temporal values keep the server's textual form.
    let rs = s
        .query(
            "SELECT CAST('2026-01-02 03:04:05.5' AS DATETIME(6)) AS ts, CAST('25:00:00' AS TIME) AS t",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(
        rs.rows[0][0],
        Value::Timestamp("2026-01-02 03:04:05.500000".into())
    );
    assert_eq!(rs.rows[0][1], Value::Time("25:00:00".into()));

    // Several result sets: only the first is returned, the rest is drained.
    let rs = s.query("SELECT 1 AS a; SELECT 2 AS b", &[]).await.unwrap();
    assert_eq!(rs.columns[0].name, "a");
    assert_eq!(rs.rows[0][0], Value::Int(1));
    let rs = s.query("SELECT 3 AS c", &[]).await.unwrap();
    assert_eq!(rs.rows[0][0], Value::Int(3));

    // Errors carry the server code and SQLSTATE.
    let err = s
        .query("SELECT * FROM no_such_table_xyz", &[])
        .await
        .unwrap_err();
    match err {
        tabler_lib::error::AppError::Db {
            vendor_code,
            sqlstate,
            ..
        } => {
            assert_eq!(vendor_code, Some(1146));
            assert_eq!(sqlstate.as_deref(), Some("42S02"));
        }
        other => panic!("expected Db error, got {other:?}"),
    }

    s.close().await;
}

#[tokio::test]
async fn execute_transaction_and_table_meta() {
    let Some((s, profile)) = session().await else {
        return;
    };
    let db = db_of(&profile);
    let is_mysql = s.server_info().await.unwrap().version.starts_with("MySQL");
    let table = format!("tabler_test_{}", uuid::Uuid::new_v4().simple());
    let q = |sql: &str| sql.replace("{t}", &format!("`{table}`"));

    let json_type = if is_mysql { "JSON" } else { "LONGTEXT" };
    s.execute(
        &q(&format!(
            "CREATE TABLE {{t}} (\
               id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT PRIMARY KEY, \
               name VARCHAR(50) NOT NULL DEFAULT 'anon' COMMENT 'display name', \
               role ENUM('admin','editor','viewer') NOT NULL DEFAULT 'viewer', \
               tags SET('a','b','c') NULL, \
               data VARBINARY(8) NULL, \
               flags BIT(3) NOT NULL DEFAULT b'0', \
               settings {json_type} NULL, \
               amount DECIMAL(10,2) NOT NULL DEFAULT 0, \
               created_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6), \
               UNIQUE KEY uq_name (name), \
               KEY idx_role_created (role, created_at)\
             ) COMMENT='tabler integration test'"
        )),
        &[],
    )
    .await
    .unwrap();

    let result =
        async {
            // INSERT through the binary protocol with every parameter kind.
            let out = s
            .execute(
                &q("INSERT INTO {t} (name, role, tags, data, flags, settings, amount, created_at) \
                    VALUES (?, ?, ?, ?, ?, ?, ?, ?)"),
                &[
                    Value::Text("alice".into()),
                    Value::Enum("admin".into()),
                    Value::Text("a,c".into()),
                    Value::from_bytes(&[0, 255, 1]),
                    Value::Int(5),
                    Value::Json("{\"theme\": \"dark\"}".into()),
                    Value::Decimal("12.50".into()),
                    Value::Timestamp("2026-01-02 03:04:05.000120".into()),
                ],
            )
            .await?;
            assert_eq!(out.affected, 1);
            assert_eq!(out.last_insert_id.as_deref(), Some("1"));
            assert!(out.rows.is_none());

            // Identical UPDATE still counts the matched row (client_found_rows).
            let out = s
                .execute(
                    &q("UPDATE {t} SET name = ? WHERE id = ?"),
                    &[Value::Text("alice".into()), Value::Int(1)],
                )
                .await?;
            assert_eq!(out.affected, 1);
            assert_eq!(out.last_insert_id, None);

            // Read back through the text protocol.
            let rs = s.query(&q("SELECT * FROM {t}"), &[]).await?;
            assert_eq!(rs.rows.len(), 1);
            let cols: Vec<_> = rs.columns.iter().map(|c| c.name.as_str()).collect();
            assert_eq!(
                cols,
                [
                    "id",
                    "name",
                    "role",
                    "tags",
                    "data",
                    "flags",
                    "settings",
                    "amount",
                    "created_at"
                ]
            );
            assert!(
                rs.columns
                    .iter()
                    .all(|c| c.table.as_deref() == Some(table.as_str()))
            );
            assert_eq!(rs.columns[0].data_type, "bigint unsigned");
            let row = &rs.rows[0];
            assert_eq!(row[0], Value::Int(1));
            assert_eq!(row[1], Value::Text("alice".into()));
            assert_eq!(row[2], Value::Enum("admin".into()));
            assert_eq!(rs.columns[2].kind, ValueKind::Enum);
            assert_eq!(row[3], Value::Text("a,c".into()));
            assert_eq!(rs.columns[3].kind, ValueKind::Set);
            match &row[4] {
                Value::Bytes(b) => assert_eq!((b.base64.as_str(), b.len), ("AP8B", 3)),
                other => panic!("expected bytes, got {other:?}"),
            }
            assert_eq!(row[5], Value::other("bit", "b'101'"));
            if is_mysql {
                assert_eq!(rs.columns[6].kind, ValueKind::Json);
                assert!(matches!(&row[6], Value::Json(j) if j.contains("\"theme\"")));
            }
            assert_eq!(row[7], Value::Decimal("12.50".into()));
            assert_eq!(
                row[8],
                Value::Timestamp("2026-01-02 03:04:05.000120".into())
            );

            // Same row through the binary protocol.
            let rs = s
                .query(&q("SELECT * FROM {t} WHERE id = ?"), &[Value::Int(1)])
                .await?;
            let row = &rs.rows[0];
            assert_eq!(row[0], Value::Int(1));
            assert_eq!(row[2], Value::Enum("admin".into()));
            assert_eq!(row[5], Value::other("bit", "b'101'"));
            assert_eq!(row[7], Value::Decimal("12.50".into()));
            assert_eq!(
                row[8],
                Value::Timestamp("2026-01-02 03:04:05.000120".into())
            );

            // Transaction: all-or-nothing.
            let outs = s
                .transaction(vec![
                    TxStatement::new(
                        q("INSERT INTO {t} (name) VALUES (?)"),
                        vec![Value::Text("bob".into())],
                    ),
                    TxStatement::new(
                        q("INSERT INTO {t} (name) VALUES (?)"),
                        vec![Value::Text("carol".into())],
                    ),
                ])
                .await?;
            assert_eq!(outs.len(), 2);
            assert_eq!(outs[1].last_insert_id.as_deref(), Some("3"));
            let err = s
                .transaction(vec![
                    TxStatement::new(
                        q("INSERT INTO {t} (name) VALUES (?)"),
                        vec![Value::Text("dave".into())],
                    ),
                    TxStatement::new(
                        q("INSERT INTO {t} (name) VALUES (?)"),
                        vec![Value::Text("alice".into())], // duplicate key
                    ),
                ])
                .await
                .unwrap_err();
            match err {
                tabler_lib::error::AppError::Db {
                    vendor_code,
                    statement_index,
                    ..
                } => {
                    assert_eq!(vendor_code, Some(1062));
                    assert_eq!(statement_index, Some(1));
                }
                other => panic!("expected Db error, got {other:?}"),
            }
            let rs = s.query(&q("SELECT COUNT(*) FROM {t}"), &[]).await?;
            assert_eq!(
                rs.rows[0][0],
                Value::Int(3),
                "failed transaction was rolled back"
            );

            // table_meta for the test table.
            let meta = s.table_meta(&TableRef::new(None, &table)).await?;
            assert_eq!(meta.table.schema.as_deref(), Some(db.as_str()));
            assert_eq!(meta.kind, TableKind::Table);
            assert_eq!(meta.primary_key, ["id"]);
            assert_eq!(meta.comment.as_deref(), Some("tabler integration test"));
            let id = &meta.columns[0];
            assert!(id.is_pk && id.is_auto_increment && id.is_unsigned);
            assert_eq!(id.pk_ordinal, Some(1));
            assert_eq!(id.kind, ValueKind::BigInt);
            assert_eq!(id.data_type, "bigint unsigned");
            assert_eq!(id.extra.as_deref(), Some("auto_increment"));
            let name = &meta.columns[1];
            assert_eq!(name.comment.as_deref(), Some("display name"));
            assert_eq!(name.default_expr.as_deref(), Some("anon"));
            assert_eq!(name.char_max_len, Some(50));
            assert!(!name.nullable && name.collation.is_some());
            let role = &meta.columns[2];
            assert_eq!(role.kind, ValueKind::Enum);
            assert_eq!(
                role.enum_values.as_deref(),
                Some(&["admin".to_string(), "editor".into(), "viewer".into()][..])
            );
            let tags = &meta.columns[3];
            assert_eq!(tags.kind, ValueKind::Set);
            assert_eq!(tags.enum_values.as_ref().map(|v| v.len()), Some(3));
            assert!(tags.nullable);
            assert_eq!(meta.columns[4].kind, ValueKind::Bytes);
            assert_eq!(meta.columns[5].kind, ValueKind::Bit);
            if is_mysql {
                assert_eq!(meta.columns[6].kind, ValueKind::Json);
            }
            let amount = &meta.columns[7];
            assert_eq!(amount.kind, ValueKind::Decimal);
            assert_eq!(
                (amount.numeric_precision, amount.numeric_scale),
                (Some(10), Some(2))
            );
            assert_eq!(meta.columns[8].kind, ValueKind::Timestamp);
            assert_eq!(meta.indexes[0].name, "PRIMARY");
            assert!(meta.indexes[0].primary && meta.indexes[0].unique);
            let uq = meta.indexes.iter().find(|i| i.name == "uq_name").unwrap();
            assert!(uq.unique && uq.columns == ["name"]);
            let idx = meta
                .indexes
                .iter()
                .find(|i| i.name == "idx_role_created")
                .unwrap();
            assert!(!idx.unique);
            assert_eq!(idx.columns, ["role", "created_at"]);
            assert_eq!(idx.method.as_deref(), Some("BTREE"));
            assert!(
                meta.ddl
                    .as_deref()
                    .unwrap_or("")
                    .starts_with("CREATE TABLE")
            );

            // Listing and completion metadata see the new table.
            let tables = s.list_tables(&db).await?;
            let mine = tables.iter().find(|t| t.name == table).unwrap();
            assert_eq!(mine.kind, TableKind::Table);
            assert_eq!(mine.comment.as_deref(), Some("tabler integration test"));
            assert!(mine.size_bytes.unwrap_or(0) > 0);
            let meta = s.completion_metadata().await?;
            assert_eq!(meta.schemas[&db][&table][0], "id");

            // A view over the table.
            let view = format!("{table}_v");
            s.execute(
                &format!("CREATE VIEW `{view}` AS SELECT id, name FROM `{table}`"),
                &[],
            )
            .await?;
            let def = s.view_definition(&TableRef::new(Some(&db), &view)).await?;
            assert!(def.unwrap_or_default().to_lowercase().contains("select"));
            let vmeta = s.table_meta(&TableRef::new(Some(&db), &view)).await?;
            assert_eq!(vmeta.kind, TableKind::View);
            assert_eq!(vmeta.columns.len(), 2);
            assert!(vmeta.ddl.as_deref().unwrap_or("").starts_with("CREATE"));
            s.execute(&format!("DROP VIEW `{view}`"), &[]).await?;
            Ok::<(), tabler_lib::error::AppError>(())
        }
        .await;

    let _ = s.execute(&q("DROP TABLE IF EXISTS {t}"), &[]).await;
    let _ = s
        .execute(&format!("DROP VIEW IF EXISTS `{table}_v`"), &[])
        .await;
    result.unwrap();
    s.close().await;
}

#[tokio::test]
async fn seeded_schema_if_present() {
    let Some((s, profile)) = session().await else {
        return;
    };
    let db = db_of(&profile);
    let tables = s.list_tables(&db).await.unwrap();
    if !tables.iter().any(|t| t.name == "users") {
        println!("skipped: no seeded `users` table in {db}");
        s.close().await;
        return;
    }

    let users = s
        .table_meta(&TableRef::new(Some(&db), "users"))
        .await
        .unwrap();
    assert_eq!(users.primary_key, ["id"]);
    let col = |n: &str| {
        users
            .columns
            .iter()
            .find(|c| c.name == n)
            .unwrap_or_else(|| panic!("users.{n} missing"))
    };
    let id = col("id");
    assert!(id.is_pk && id.is_auto_increment);
    assert_eq!(id.kind, ValueKind::BigInt);
    let role = col("role");
    assert_eq!(role.kind, ValueKind::Enum);
    assert_eq!(role.enum_values.as_ref().map(|v| v.len()), Some(3));
    assert_eq!(col("settings").kind, ValueKind::Json);
    assert_eq!(col("tags").kind, ValueKind::Set);
    assert_eq!(col("avatar").kind, ValueKind::Bytes);
    assert_eq!(col("created_at").kind, ValueKind::Timestamp);
    assert!(
        users
            .ddl
            .as_deref()
            .unwrap_or("")
            .starts_with("CREATE TABLE")
    );

    let posts = s
        .table_meta(&TableRef::new(Some(&db), "posts"))
        .await
        .unwrap();
    let fk = posts
        .foreign_keys
        .iter()
        .find(|f| f.ref_table.name == "users")
        .expect("posts has a FK to users");
    assert_eq!(fk.ref_table.schema.as_deref(), Some(db.as_str()));
    assert_eq!(fk.on_delete, "CASCADE");
    assert!(!fk.columns.is_empty() && fk.columns.len() == fk.ref_columns.len());
    assert!(!posts.triggers.is_empty(), "posts has a trigger");
    let trg = &posts.triggers[0];
    assert!(trg.timing.is_some() && !trg.events.is_empty() && trg.enabled);

    // Seeded rows decode without panicking.
    let rs = s.query("SELECT * FROM `users` LIMIT 5", &[]).await.unwrap();
    assert!(!rs.rows.is_empty());
    let _ = text(
        &rs.rows[0][rs
            .columns
            .iter()
            .position(|c| c.name == "email")
            .unwrap_or(1)],
    );
    s.close().await;
}

#[tokio::test]
async fn tls_require_encrypts_when_server_supports_it() {
    let Some((plain, _)) = session().await else {
        return;
    };
    let rs = plain
        .query("SHOW STATUS LIKE 'Ssl_cipher'", &[])
        .await
        .unwrap();
    assert_eq!(
        text(&rs.rows[0][1]),
        "",
        "ssl=disable must not negotiate TLS"
    );
    let rs = plain
        .query("SHOW VARIABLES LIKE 'tls_version'", &[])
        .await
        .unwrap();
    let tls_version = rs.rows.first().map(|r| text(&r[1])).unwrap_or_default();
    plain.close().await;
    if tls_version.is_empty() {
        println!("skipped: server has no TLS configured");
        return;
    }

    for mode in [SslMode::Require, SslMode::Prefer] {
        let Some((s, _)) = session_with(|p| p.ssl = mode).await else {
            return;
        };
        let rs = s.query("SHOW STATUS LIKE 'Ssl_cipher'", &[]).await.unwrap();
        let cipher = text(&rs.rows[0][1]);
        println!("ssl={mode:?}: cipher={cipher}");
        assert!(!cipher.is_empty(), "ssl={mode:?} should negotiate TLS");
        s.ping().await.unwrap();
        s.close().await;
    }
}

#[tokio::test]
async fn caching_sha2_password_over_plain_tcp() {
    let Some((root, profile)) = session().await else {
        return;
    };
    if !root
        .server_info()
        .await
        .unwrap()
        .version
        .starts_with("MySQL")
    {
        println!("skipped: caching_sha2_password is MySQL-only");
        root.close().await;
        return;
    }
    // MySQL user names are capped at 32 characters.
    let user = format!(
        "tabler_t_{}",
        &uuid::Uuid::new_v4().simple().to_string()[..12]
    );
    let password = uuid::Uuid::new_v4().simple().to_string();
    let created = root
        .execute(
            &format!(
                "CREATE USER '{user}'@'%' IDENTIFIED WITH caching_sha2_password BY '{password}'"
            ),
            &[],
        )
        .await;
    if let Err(e) = created {
        println!("skipped: cannot create a test user ({e})");
        root.close().await;
        return;
    }

    let result = async {
        // First login: the server demands full authentication; without TLS the client must
        // fetch the server's RSA public key and send the encrypted password.
        for attempt in 1..=2 {
            let (mut p, _) = profile_from_url(&std::env::var(ENV).unwrap());
            p.user = Some(user.clone());
            p.database = None;
            p.ssl = SslMode::Disable;
            let s = connect(ConnectTarget {
                profile: &p,
                password: Some(&password),
                host_override: None,
            })
            .await
            .map_err(|e| format!("attempt {attempt}: {e}"))?;
            let info = s.server_info().await.map_err(|e| e.to_string())?;
            assert!(
                info.current_user
                    .as_deref()
                    .unwrap_or("")
                    .starts_with(&user)
            );
            s.close().await;
        }
        // Wrong password maps to an auth error.
        let (mut p, _) = profile_from_url(&std::env::var(ENV).unwrap());
        p.user = Some(user.clone());
        p.database = None;
        let err = connect(ConnectTarget {
            profile: &p,
            password: Some("definitely-wrong"),
            host_override: None,
        })
        .await
        .err()
        .ok_or("wrong password accepted")?;
        assert!(
            matches!(err, tabler_lib::error::AppError::Auth(_)),
            "expected Auth error, got {err:?}"
        );
        Ok::<(), String>(())
    }
    .await;
    let _ = profile;
    let _ = root
        .execute(&format!("DROP USER IF EXISTS '{user}'@'%'"), &[])
        .await;
    root.close().await;
    result.unwrap();
}

/// The inspector's "Load full value" on a table without a key: MySQL's all-columns WHERE already
/// ends with `LIMIT 1`, and the blob must come back whole (beyond the 64 KiB grid preview).
#[tokio::test]
async fn fetch_full_cell_by_all_columns_key() {
    use std::collections::BTreeMap;
    use tabler_lib::sqlgen::mutate::TableSpec;
    use tabler_lib::sqlgen::select::build_fetch_cell;
    use tabler_lib::types::browse::RowKey;
    use tabler_lib::types::value::decode_hex;

    let Some((s, profile)) = session().await else {
        return;
    };
    let db = db_of(&profile);
    let table = format!("tabler_test_{}", uuid::Uuid::new_v4().simple());
    let q = |sql: &str| sql.replace("{t}", &format!("`{table}`"));
    let big: Vec<u8> = (0..70_000u32).map(|i| (i % 251) as u8).collect();

    let result = async {
        s.execute(
            &q("CREATE TABLE {t} (a INT NULL, b LONGBLOB NULL, c VARCHAR(20) NULL)"),
            &[],
        )
        .await?;
        s.execute(
            &q("INSERT INTO {t} (a, b, c) VALUES (?, ?, ?), (?, ?, ?)"),
            &[
                Value::Int(1),
                Value::from_bytes_uncapped(&big),
                Value::Text("x".into()),
                Value::Int(2),
                Value::Null,
                Value::Null,
            ],
        )
        .await?;
        let meta = s.table_meta(&TableRef::new(Some(&db), &table)).await?;
        let spec = TableSpec {
            dialect: s.dialect(),
            table: &meta.table,
            columns: &meta.columns,
        };
        let blob = meta.columns.iter().find(|c| c.name == "b").unwrap();
        assert_eq!(blob.kind, ValueKind::Bytes);

        let key = RowKey::AllColumns(BTreeMap::from([
            ("a".to_string(), Value::Int(1)),
            ("c".to_string(), Value::Text("x".into())),
        ]));
        let (sql, params) = build_fetch_cell(&spec, blob, &key)?;
        assert_eq!(sql.matches("LIMIT").count(), 1, "{sql}");
        let rs = s.query(&sql, &params).await?;
        let hex = match rs.rows[0][0].clone() {
            Value::Text(t) => t,
            Value::Bytes(b) => {
                use base64::Engine as _;
                let raw = base64::engine::general_purpose::STANDARD
                    .decode(b.base64)
                    .unwrap();
                String::from_utf8(raw).unwrap()
            }
            other => panic!("expected hex text, got {other:?}"),
        };
        assert_eq!(decode_hex(&hex).unwrap(), big);

        // NULL key values match too (`<=>`), and a NULL blob stays NULL.
        let key = RowKey::AllColumns(BTreeMap::from([
            ("a".to_string(), Value::Int(2)),
            ("c".to_string(), Value::Null),
        ]));
        let (sql, params) = build_fetch_cell(&spec, blob, &key)?;
        let rs = s.query(&sql, &params).await?;
        assert_eq!(rs.rows.len(), 1);
        assert_eq!(rs.rows[0][0], Value::Null);
        Ok::<(), tabler_lib::error::AppError>(())
    }
    .await;

    let _ = s.execute(&q("DROP TABLE IF EXISTS {t}"), &[]).await;
    result.unwrap();
    s.close().await;
}
