//! PostgreSQL driver integration test. Runs only when `TABLER_TEST_PG_URL` is set, e.g.
//! `TABLER_TEST_PG_URL=postgres://apriansyahrs@127.0.0.1:5432/tabler_dev cargo test --test postgres`.

use std::sync::Arc;

use tabler_lib::drivers::{ConnectTarget, DbSession, TxStatement, connect};
use tabler_lib::types::profile::{ConnectionProfile, Driver, ProfileOptions, SslMode};
use tabler_lib::types::schema::{TableKind, TableRef, ValueKind};
use tabler_lib::types::value::Value;

/// Parses `postgres://user[:pass]@host[:port]/db` into a profile plus the password.
fn profile_from_url(url: &str) -> (ConnectionProfile, Option<String>) {
    let rest = url
        .strip_prefix("postgres://")
        .or_else(|| url.strip_prefix("postgresql://"))
        .expect("TABLER_TEST_PG_URL must start with postgres://");
    let (auth, hostdb) = match rest.rsplit_once('@') {
        Some((a, h)) => (Some(a), h),
        None => (None, rest),
    };
    let (user, password) = match auth {
        Some(a) => match a.split_once(':') {
            Some((u, p)) => (Some(u.to_string()), Some(p.to_string())),
            None => (Some(a.to_string()), None),
        },
        None => (None, None),
    };
    let (hostport, db) = match hostdb.split_once('/') {
        Some((h, d)) => (h, Some(d.split('?').next().unwrap_or("").to_string())),
        None => (hostdb, None),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse::<u16>().ok()),
        None => (hostport.to_string(), None),
    };
    let profile = ConnectionProfile {
        id: uuid::Uuid::new_v4(),
        name: "pg-test".into(),
        color: None,
        group: None,
        driver: Driver::Postgres,
        host: Some(host),
        port,
        user,
        database: db.filter(|d| !d.is_empty()),
        ssl: SslMode::Prefer,
        ssl_ca_path: None,
        sqlite_path: None,
        ssh: None,
        options: ProfileOptions::default(),
        sort_order: 0,
        created_at: String::new(),
        updated_at: String::new(),
        last_connected_at: None,
    };
    (profile, password)
}

async fn open() -> Option<Arc<dyn DbSession>> {
    let Ok(url) = std::env::var("TABLER_TEST_PG_URL") else {
        println!("skipped: TABLER_TEST_PG_URL unset");
        return None;
    };
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();
    let (profile, password) = profile_from_url(&url);
    let session = connect(ConnectTarget {
        profile: &profile,
        password: password.as_deref(),
        host_override: None,
    })
    .await
    .expect("connect");
    Some(session)
}

async fn table_exists(session: &Arc<dyn DbSession>, name: &str) -> bool {
    session
        .list_tables("public")
        .await
        .expect("list_tables")
        .iter()
        .any(|t| t.name == name)
}

#[tokio::test]
async fn connects_and_introspects() {
    let Some(session) = open().await else {
        return;
    };

    let info = session.server_info().await.expect("server_info");
    assert!(
        info.version.starts_with("PostgreSQL"),
        "version: {}",
        info.version
    );
    let current_db = info.current_database.clone().expect("current_database");
    assert!(info.current_user.is_some());
    session.ping().await.expect("ping");

    let dbs = session.list_databases().await.expect("list_databases");
    let current = dbs
        .iter()
        .find(|d| d.name == current_db)
        .expect("current database listed");
    assert!(current.is_current);
    assert!(current.encoding.is_some());
    assert_eq!(dbs.iter().filter(|d| d.is_current).count(), 1);

    let schemas = session.list_schemas().await.expect("list_schemas");
    assert!(schemas.iter().any(|s| s.name == "public"));
    assert!(!schemas.iter().any(|s| s.name == "pg_catalog"));

    let tables = session.list_tables("public").await.expect("list_tables");
    for t in &tables {
        assert_eq!(t.schema.as_deref(), Some("public"));
    }

    let meta = session
        .completion_metadata()
        .await
        .expect("completion_metadata");
    assert!(meta.schemas.contains_key("public"));
    assert!(meta.default_schema.is_some());

    session.close().await;
    // After close the editor is gone and the pool refuses new work.
    assert!(session.ping().await.is_err());
}

#[tokio::test]
async fn queries_and_binds() {
    let Some(session) = open().await else {
        return;
    };

    let rs = session
        .query(
            "SELECT 1::text AS a, 'x'::text AS b, NULL::text AS c, 2 AS i, true AS t",
            &[],
        )
        .await
        .expect("query");
    assert_eq!(
        rs.columns
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["a", "b", "c", "i", "t"]
    );
    assert_eq!(rs.columns[0].kind, ValueKind::Text);
    assert_eq!(rs.columns[3].data_type, "int4");
    assert_eq!(rs.columns[3].kind, ValueKind::Int);
    assert_eq!(rs.columns[4].kind, ValueKind::Bool);
    assert_eq!(
        rs.rows,
        vec![vec![
            Value::Text("1".into()),
            Value::Text("x".into()),
            Value::Null,
            Value::Int(2),
            Value::Bool(true),
        ]]
    );
    assert!(!rs.truncated);

    let rs = session
        .query("SELECT ($1::int + 1)::text", &[Value::Int(41)])
        .await
        .expect("parameterised query");
    assert_eq!(rs.rows, vec![vec![Value::Text("42".into())]]);

    // Session init applied on pooled clients: ISO DateStyle and extra_float_digits.
    let rs = session
        .query(
            "SELECT current_setting('DateStyle'), current_setting('extra_float_digits'), \
                    current_setting('application_name')",
            &[],
        )
        .await
        .expect("settings");
    assert_eq!(
        rs.rows[0],
        vec![
            Value::Text("ISO, YMD".into()),
            Value::Text("3".into()),
            Value::Text("tabler".into()),
        ]
    );

    // Binary decoding of the natively supported set; unsupported types are flagged, NULLs stay NULL.
    let rs = session
        .query(
            "SELECT 9007199254740993::int8, 1.5::float8, '\\x00ff'::bytea, \
                    '123e4567-e89b-12d3-a456-426614174000'::uuid, '{\"a\":1}'::jsonb, \
                    DATE '2024-02-29', TIMESTAMP '2024-02-29 12:34:56.5', 1.5::numeric, NULL::numeric, \
                    ARRAY['a','b']::text[]",
            &[],
        )
        .await
        .expect("binary query");
    let row = &rs.rows[0];
    assert_eq!(row[0], Value::BigInt("9007199254740993".into()));
    assert_eq!(row[1], Value::Float(1.5));
    match &row[2] {
        Value::Bytes(b) => assert_eq!(b.len, 2),
        other => panic!("bytea decoded as {other:?}"),
    }
    assert_eq!(
        row[3],
        Value::Uuid("123e4567-e89b-12d3-a456-426614174000".into())
    );
    assert_eq!(row[4], Value::Json("{\"a\": 1}".into()));
    assert_eq!(row[5], Value::Date("2024-02-29".into()));
    assert_eq!(row[6], Value::Timestamp("2024-02-29 12:34:56.5".into()));
    assert!(matches!(&row[7], Value::Other(o) if o.ty == "numeric"));
    assert_eq!(row[8], Value::Null);
    assert_eq!(rs.columns[9].kind, ValueKind::Array);
    assert!(matches!(&row[9], Value::Other(o) if o.ty == "_text"));

    // Errors carry the SQLSTATE and position.
    let err = session
        .query("SELECT * FROM no_such_table_xyz", &[])
        .await
        .expect_err("missing table");
    match err {
        tabler_lib::error::AppError::Db {
            sqlstate, position, ..
        } => {
            assert_eq!(sqlstate.as_deref(), Some("42P01"));
            assert_eq!(position.map(|p| p.offset), Some(14));
        }
        other => panic!("unexpected error {other:?}"),
    }

    session.close().await;
}

#[tokio::test]
async fn executes_and_rolls_back_transactions() {
    let Some(session) = open().await else {
        return;
    };
    let schema = format!("tabler_test_{}", uuid::Uuid::new_v4().simple());
    let table = format!("\"{schema}\".items");

    session
        .execute(&format!("CREATE SCHEMA \"{schema}\""), &[])
        .await
        .expect("create schema");
    let outcome = session
        .execute(
            &format!("CREATE TABLE {table} (id serial PRIMARY KEY, name text NOT NULL, qty int)"),
            &[],
        )
        .await
        .expect("create table");
    assert_eq!(outcome.affected, 0);
    assert!(outcome.rows.is_none());

    // execute with params + RETURNING.
    let outcome = session
        .execute(
            &format!(
                "INSERT INTO {table} (name, qty) VALUES ($1::text, $2::int) RETURNING id, name"
            ),
            &[Value::Text("one".into()), Value::Int(5)],
        )
        .await
        .expect("insert");
    assert_eq!(outcome.affected, 1);
    let rows = outcome.rows.expect("RETURNING rows");
    assert_eq!(
        rows.rows,
        vec![vec![Value::Int(1), Value::Text("one".into())]]
    );

    // transaction: commit path.
    let outcomes = session
        .transaction(vec![
            TxStatement::new(
                format!("INSERT INTO {table} (name, qty) VALUES ($1::text, $2::int)"),
                vec![Value::Text("two".into()), Value::Null],
            ),
            TxStatement::new(
                format!("UPDATE {table} SET qty = $1::int WHERE name = $2::text"),
                vec![Value::Int(7), Value::Text("one".into())],
            ),
        ])
        .await
        .expect("transaction");
    assert_eq!(
        outcomes.iter().map(|o| o.affected).collect::<Vec<_>>(),
        [1, 1]
    );

    // transaction: rollback on error leaves no trace.
    let err = session
        .transaction(vec![
            TxStatement::new(
                format!("INSERT INTO {table} (name) VALUES ($1::text)"),
                vec![Value::Text("ghost".into())],
            ),
            TxStatement::new(format!("INSERT INTO {table} (name) VALUES (NULL)"), vec![]),
        ])
        .await
        .expect_err("not-null violation");
    match err {
        tabler_lib::error::AppError::Db {
            sqlstate,
            statement_index,
            ..
        } => {
            assert_eq!(sqlstate.as_deref(), Some("23502"));
            assert_eq!(statement_index, Some(1));
        }
        other => panic!("unexpected error {other:?}"),
    }
    let rs = session
        .query(
            &format!("SELECT name, qty::text FROM {table} ORDER BY id"),
            &[],
        )
        .await
        .expect("select");
    assert_eq!(
        rs.rows,
        vec![
            vec![Value::Text("one".into()), Value::Text("7".into())],
            vec![Value::Text("two".into()), Value::Null],
        ]
    );

    // Temp tables are per connection, so create+use must share one transaction.
    let outcomes = session
        .transaction(vec![
            TxStatement::new("CREATE TEMP TABLE tmp_t (v int)", vec![]),
            TxStatement::new("INSERT INTO tmp_t VALUES (1), (2)", vec![]),
            TxStatement::new("SELECT sum(v)::text FROM tmp_t", vec![]),
        ])
        .await
        .expect("temp table transaction");
    assert_eq!(outcomes[1].affected, 2);
    assert_eq!(
        outcomes[2].rows.as_ref().unwrap().rows,
        vec![vec![Value::Text("3".into())]]
    );

    // Introspect what we created.
    let meta = session
        .table_meta(&TableRef::new(Some(&schema), "items"))
        .await
        .expect("table_meta");
    assert_eq!(meta.kind, TableKind::Table);
    assert_eq!(meta.primary_key, ["id"]);
    assert!(meta.columns[0].is_pk && meta.columns[0].is_auto_increment);
    assert_eq!(meta.columns[0].kind, ValueKind::Int);
    assert!(!meta.columns[1].nullable);
    assert!(meta.columns[2].nullable);
    assert!(
        meta.indexes
            .iter()
            .any(|i| i.primary && i.columns == ["id"])
    );

    session
        .execute(&format!("DROP SCHEMA \"{schema}\" CASCADE"), &[])
        .await
        .expect("drop schema");
    session.close().await;
}

#[tokio::test]
async fn introspects_seeded_tables() {
    let Some(session) = open().await else {
        return;
    };
    if !table_exists(&session, "users").await || !table_exists(&session, "posts").await {
        println!("skipped seeded-table assertions: users/posts not present");
        session.close().await;
        return;
    }

    let users = session
        .table_meta(&TableRef::new(Some("public"), "users"))
        .await
        .expect("users meta");
    assert_eq!(users.kind, TableKind::Table);
    assert_eq!(users.primary_key, ["id"]);
    let col = |name: &str| {
        users
            .columns
            .iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| panic!("column {name}"))
    };
    let id = col("id");
    assert!(id.is_pk && id.is_auto_increment);
    assert_eq!(id.pk_ordinal, Some(1));
    assert_eq!(id.kind, ValueKind::BigInt);
    let role = col("role");
    assert_eq!(role.kind, ValueKind::Enum);
    assert_eq!(
        role.enum_values.as_ref().map(|v| v.len()),
        Some(3),
        "{:?}",
        role.enum_values
    );
    assert_eq!(col("settings").kind, ValueKind::Json);
    let tags = col("tags");
    assert_eq!(tags.kind, ValueKind::Array);
    assert_eq!(tags.array_elem_type.as_deref(), Some("text"));
    assert_eq!(col("avatar").kind, ValueKind::Bytes);
    assert_eq!(col("created_at").kind, ValueKind::TimestampTz);
    let balance = col("balance");
    assert_eq!(balance.kind, ValueKind::Decimal);
    assert_eq!(balance.numeric_precision, Some(20));
    assert_eq!(balance.numeric_scale, Some(6));
    assert_eq!(col("uid").kind, ValueKind::Uuid);
    assert_eq!(col("is_active").kind, ValueKind::Bool);
    assert_eq!(col("score").kind, ValueKind::Float);

    // Browsing path: cast to text and decode with the column kinds.
    let rs = session
        .query(
            "SELECT role::text, tags::text, settings::text, created_at::text FROM public.users ORDER BY id LIMIT 1",
            &[],
        )
        .await
        .expect("browse users");
    if let Some(row) = rs.rows.first() {
        use tabler_lib::drivers::postgres::decode::text_to_value;
        if let Value::Text(t) = &row[0] {
            assert!(matches!(
                text_to_value(ValueKind::Enum, "user_role", t),
                Value::Enum(_)
            ));
        }
        if let Value::Text(t) = &row[1] {
            assert!(matches!(
                text_to_value(ValueKind::Array, "_text", t),
                Value::Array(_)
            ));
        }
        if let Value::Text(t) = &row[3] {
            assert!(matches!(
                text_to_value(ValueKind::TimestampTz, "timestamptz", t),
                Value::TimestampTz(_)
            ));
        }
    }

    let posts = session
        .table_meta(&TableRef::new(Some("public"), "posts"))
        .await
        .expect("posts meta");
    let fks: Vec<_> = posts
        .foreign_keys
        .iter()
        .filter(|fk| fk.ref_table.name == "users")
        .collect();
    assert_eq!(fks.len(), 1, "{:?}", posts.foreign_keys);
    assert_eq!(fks[0].on_delete, "CASCADE");
    assert_eq!(fks[0].columns, ["user_id"]);
    assert_eq!(fks[0].ref_columns, ["id"]);
    assert_eq!(fks[0].ref_table.schema.as_deref(), Some("public"));
    assert!(
        fks[0]
            .definition
            .as_deref()
            .unwrap_or("")
            .starts_with("FOREIGN KEY")
    );
    assert!(!posts.triggers.is_empty(), "posts should have a trigger");
    assert!(posts.triggers[0].enabled);
    assert!(posts.triggers[0].timing.is_some());
    assert!(!posts.triggers[0].events.is_empty());
    let post_id = posts.columns.iter().find(|c| c.name == "id").unwrap();
    assert!(post_id.is_auto_increment);
    assert_eq!(post_id.extra.as_deref(), Some("ALWAYS"));

    // Views (if any) have a definition; tables do not.
    assert_eq!(
        session
            .view_definition(&TableRef::new(Some("public"), "users"))
            .await
            .expect("view_definition"),
        None
    );
    if let Some(view) = session
        .list_tables("public")
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.kind == TableKind::View)
    {
        let def = session
            .view_definition(&TableRef::new(Some("public"), &view.name))
            .await
            .expect("view def");
        assert!(def.unwrap_or_default().to_uppercase().contains("SELECT"));
    }

    assert!(
        session
            .table_meta(&TableRef::new(Some("public"), "no_such_table_xyz"))
            .await
            .is_err()
    );

    session.close().await;
}
