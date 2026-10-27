//! Server administration against real servers: databases, schemas, the process list, killing a
//! session and user management, through `tabler_lib::admin`. Gated on `TABLER_TEST_PG_URL` /
//! `TABLER_TEST_MYSQL_URL` (each test prints a skip notice and passes when unset). Every object
//! is named `admin_<random>` and dropped at the end.

use std::sync::Arc;
use std::time::Duration;

use tabler_lib::admin;
use tabler_lib::drivers::{ConnectTarget, DbSession, connect};
use tabler_lib::types::admin::{
    AdminDdl, KillScope, ObjectPrivileges, ResourceLimits, RoleAttributes, UserChange, UserRef,
    UserState,
};
use tabler_lib::types::profile::{ConnectionProfile, Driver, ProfileOptions, SslMode};

/// `scheme://user[:pass]@host[:port]/db` → profile + password.
fn profile_from_url(url: &str, driver: Driver) -> (ConnectionProfile, Option<String>) {
    let rest = url.split_once("://").map(|(_, r)| r).expect("a URL");
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
        name: "admin-test".into(),
        color: None,
        group: None,
        driver,
        host: Some(host),
        port,
        user,
        database: db.filter(|d| !d.is_empty()),
        ssl: if driver == Driver::Mysql {
            SslMode::Disable
        } else {
            SslMode::Prefer
        },
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

async fn open_profile(profile: &ConnectionProfile, password: Option<&str>) -> Arc<dyn DbSession> {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();
    connect(ConnectTarget {
        profile,
        password,
        host_override: None,
    })
    .await
    .expect("connect")
}

async fn open(var: &str, driver: Driver) -> Option<(Arc<dyn DbSession>, ConnectionProfile)> {
    let Ok(url) = std::env::var(var) else {
        println!("skipped: {var} unset");
        return None;
    };
    let (profile, password) = profile_from_url(&url, driver);
    Some((open_profile(&profile, password.as_deref()).await, profile))
}

fn suffix() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..8].to_string()
}

fn privs(object: &str, list: &[&str]) -> ObjectPrivileges {
    ObjectPrivileges {
        object: object.into(),
        privileges: list.iter().map(|s| s.to_string()).collect(),
    }
}

async fn database_names(session: &dyn DbSession) -> Vec<String> {
    session
        .list_databases()
        .await
        .unwrap()
        .into_iter()
        .map(|d| d.name)
        .collect()
}

async fn ok(session: &dyn DbSession, op: AdminDdl) {
    let r = admin::apply_ddl(session, &op).await.expect("valid op");
    assert!(r.error.is_none(), "{op:?}: {:?}", r.error);
}

/// Runs a sleep on a pool connection, finds it in the process list by its marker and kills it.
async fn kill_a_sleeper(
    session: Arc<dyn DbSession>,
    sleep_sql: String,
    marker: String,
    fails: bool,
) {
    let sleeper = {
        let session = session.clone();
        tokio::spawn(async move { session.execute(&sleep_sql, &[]).await })
    };
    let mut target = None;
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let rows = admin::list_processes(session.as_ref()).await.unwrap();
        assert!(
            rows.iter().any(|r| r.current && r.own),
            "the listing session"
        );
        target = rows
            .iter()
            .find(|r| r.query.as_deref().is_some_and(|q| q.contains(&marker)) && !r.current)
            .map(|r| r.id);
        if target.is_some() {
            break;
        }
    }
    let id = target.expect("the sleeping session is listed");
    let out = admin::kill(session.as_ref(), &[id], KillScope::Query)
        .await
        .unwrap();
    assert!(out[0].ok, "{out:?}");
    let result = tokio::time::timeout(Duration::from_secs(10), sleeper)
        .await
        .expect("the sleep stops")
        .unwrap();
    // PG reports the cancel as an error; MySQL's SLEEP() just returns 1 early.
    assert_eq!(result.is_err(), fails, "{result:?}");
    // An unknown process.
    let out = admin::kill(session.as_ref(), &[2_000_000_000], KillScope::Query)
        .await
        .unwrap();
    assert!(!out[0].ok && out[0].message.is_some(), "{out:?}");
}

#[tokio::test]
async fn postgres_admin() {
    let Some((session, _)) = open("TABLER_TEST_PG_URL", Driver::Postgres).await else {
        return;
    };
    let s = session.as_ref();
    let id = suffix();
    let db = format!("admin_db_{id}");
    let renamed = format!("admin_db_{id}_r");
    let latin = format!("admin_latin_{id}");
    let schema = format!("admin_s_{id}");
    let role = format!("admin_role_{id}");
    let role2 = format!("{role}_2");

    // --- databases
    let options = admin::database_options(s).await.unwrap();
    assert!(options.encodings.iter().any(|e| e.name == "UTF8"));
    assert!(options.templates.iter().any(|t| t == "template0"));
    assert!(options.default_encoding.is_some());
    assert!(options.collations.iter().any(|c| c.name == "C"));
    ok(
        s,
        AdminDdl::CreateDatabase {
            name: db.clone(),
            encoding: options.default_encoding.clone(),
            collation: None,
            template: None,
        },
    )
    .await;
    ok(
        s,
        AdminDdl::CreateDatabase {
            name: latin.clone(),
            encoding: Some("LATIN1".into()),
            collation: Some("C".into()),
            template: Some("template0".into()),
        },
    )
    .await;
    ok(
        s,
        AdminDdl::RenameDatabase {
            name: db.clone(),
            new_name: renamed.clone(),
        },
    )
    .await;
    let names = database_names(s).await;
    assert!(names.contains(&renamed) && names.contains(&latin) && !names.contains(&db));
    // A failure comes back in the result.
    let dup = admin::apply_ddl(
        s,
        &AdminDdl::CreateDatabase {
            name: renamed.clone(),
            encoding: None,
            collation: None,
            template: None,
        },
    )
    .await
    .unwrap();
    assert!(dup.error.unwrap().message.contains("already exists"));
    ok(s, AdminDdl::DropDatabase { name: renamed }).await;
    ok(s, AdminDdl::DropDatabase { name: latin }).await;

    // --- schemas
    ok(
        s,
        AdminDdl::CreateSchema {
            name: schema.clone(),
            owner: None,
        },
    )
    .await;
    s.execute(&format!("CREATE TABLE \"{schema}\".t (id int)"), &[])
        .await
        .unwrap();

    // --- process list + kill
    let marker = format!("admin_sleep_{id}");
    kill_a_sleeper(
        session.clone(),
        format!("SELECT pg_sleep(30) /* {marker} */"),
        marker,
        true,
    )
    .await;

    // --- roles
    let current_db = s.server_info().await.unwrap().current_database.unwrap();
    let mut state = UserState {
        name: role.clone(),
        attributes: Some(RoleAttributes {
            login: true,
            connection_limit: 4,
            valid_until: Some("2099-01-01 00:00:00+00".into()),
            ..RoleAttributes::default()
        }),
        database_privileges: vec![privs(&current_db, &["CONNECT", "TEMPORARY"])],
        schema_privileges: vec![privs(&schema, &["USAGE"])],
        table_privileges: vec![privs(&schema, &["SELECT", "INSERT"])],
        ..UserState::default()
    };
    let create = UserChange {
        original: None,
        edited: Some(state.clone()),
        password: Some(r"it's a \ secret".into()),
    };
    let preview = admin::preview_user(s.dialect().core().unwrap(), &create).unwrap();
    assert!(preview.iter().all(|p| !p.contains("secret")), "{preview:?}");
    let r = admin::apply_user(s, &create).await.unwrap();
    assert!(r.error.is_none(), "{:?}", r.error);
    assert!(r.executed.iter().all(|p| !p.contains("secret")));

    let listed = admin::list_users(s, false).await.unwrap();
    assert!(listed.iter().any(|u| u.name == role && u.can_login));
    let loaded = admin::load_user(
        s,
        &UserRef {
            name: role.clone(),
            host: None,
        },
    )
    .await
    .unwrap();
    assert!(loaded.schemas.contains(&schema));
    let got = loaded.state;
    let attrs = got.attributes.clone().unwrap();
    assert!(attrs.login && !attrs.superuser && attrs.inherit);
    assert_eq!(attrs.connection_limit, 4);
    assert!(
        attrs
            .valid_until
            .as_deref()
            .unwrap()
            .starts_with("2099-01-01")
    );
    assert_eq!(
        got.database_privileges
            .iter()
            .find(|p| p.object == current_db)
            .unwrap()
            .privileges,
        ["CONNECT", "TEMPORARY"]
    );
    assert_eq!(got.schema_privileges, [privs(&schema, &["USAGE"])]);
    assert_eq!(
        got.table_privileges,
        [privs(&schema, &["SELECT", "INSERT"])]
    );

    // Rename, change attributes, revoke INSERT, add CREATE on the schema.
    state = got.clone();
    state.name = role2.clone();
    state.attributes = Some(RoleAttributes {
        login: false,
        create_db: true,
        valid_until: None,
        ..attrs.clone()
    });
    state.table_privileges = vec![privs(&schema, &["SELECT"])];
    state.schema_privileges = vec![privs(&schema, &["USAGE", "CREATE"])];
    let edit = UserChange {
        original: Some(got),
        edited: Some(state),
        password: None,
    };
    let r = admin::apply_user(s, &edit).await.unwrap();
    assert!(r.error.is_none(), "{:?}", r.error);
    assert!(
        admin::list_users(s, false)
            .await
            .unwrap()
            .iter()
            .all(|u| u.name != role2),
        "NOLOGIN roles are left out unless asked for"
    );
    let again = admin::load_user(
        s,
        &UserRef {
            name: role2.clone(),
            host: None,
        },
    )
    .await
    .unwrap()
    .state;
    let a = again.attributes.clone().unwrap();
    assert!(!a.login && a.create_db && a.valid_until.is_none());
    assert_eq!(again.table_privileges, [privs(&schema, &["SELECT"])]);
    // In the canonical order (USAGE, CREATE).
    assert_eq!(
        again.schema_privileges,
        [privs(&schema, &["USAGE", "CREATE"])]
    );

    // A failing plan rolls back as a whole.
    let mut bad = again.clone();
    bad.database_privileges
        .push(privs("admin_no_such_db", &["CONNECT"]));
    let r = admin::apply_user(
        s,
        &UserChange {
            original: Some(again.clone()),
            edited: Some(bad),
            password: None,
        },
    )
    .await
    .unwrap();
    assert!(r.error.is_some() && r.executed.is_empty());

    // Dropping revokes what it holds first.
    let r = admin::apply_user(
        s,
        &UserChange {
            original: Some(again),
            edited: None,
            password: None,
        },
    )
    .await
    .unwrap();
    assert!(r.error.is_none(), "{:?}", r.error);
    ok(
        s,
        AdminDdl::DropSchema {
            name: schema,
            cascade: true,
        },
    )
    .await;
    session.close().await;
}

#[tokio::test]
async fn mysql_admin() {
    let Some((session, profile)) = open("TABLER_TEST_MYSQL_URL", Driver::Mysql).await else {
        return;
    };
    let s = session.as_ref();
    let id = suffix();
    let db = format!("admin_db_{id}");
    let user = format!("admin_u_{id}");
    let user2 = format!("{user}_2");

    // --- databases
    let options = admin::database_options(s).await.unwrap();
    assert_eq!(options.default_encoding.as_deref(), Some("utf8mb4"));
    let utf8 = options
        .encodings
        .iter()
        .find(|e| e.name == "utf8mb4")
        .unwrap();
    assert!(utf8.default_collation.is_some());
    assert!(
        options
            .collations
            .iter()
            .any(|c| c.name == "utf8mb4_bin" && c.encoding.as_deref() == Some("utf8mb4"))
    );
    ok(
        s,
        AdminDdl::CreateDatabase {
            name: db.clone(),
            encoding: Some("utf8mb4".into()),
            collation: Some("utf8mb4_bin".into()),
            template: None,
        },
    )
    .await;
    assert!(database_names(s).await.contains(&db));
    let rename = admin::apply_ddl(
        s,
        &AdminDdl::RenameDatabase {
            name: db.clone(),
            new_name: "x".into(),
        },
    )
    .await;
    assert!(rename.is_err(), "MySQL cannot rename a database");

    // --- process list + kill
    let marker = format!("admin_sleep_{id}");
    kill_a_sleeper(
        session.clone(),
        format!("SELECT SLEEP(30) /* {marker} */"),
        marker,
        false,
    )
    .await;

    // --- users
    let password = r"p'w\x";
    let state = UserState {
        name: user.clone(),
        host: Some("%".into()),
        global_privileges: vec!["PROCESS".into()],
        limits: Some(ResourceLimits {
            max_queries_per_hour: 1000,
            max_user_connections: 3,
            ..ResourceLimits::default()
        }),
        database_privileges: vec![privs(&db, &["SELECT", "INSERT"])],
        ..UserState::default()
    };
    let r = admin::apply_user(
        s,
        &UserChange {
            original: None,
            edited: Some(state.clone()),
            password: Some(password.into()),
        },
    )
    .await
    .unwrap();
    assert!(r.error.is_none(), "{:?}", r.error);
    assert!(r.executed.iter().all(|e| !e.contains("p\\'w")));

    // The password literal arrived intact: log in with it.
    let mut as_user = profile.clone();
    as_user.user = Some(user.clone());
    as_user.database = Some(db.clone());
    let theirs = open_profile(&as_user, Some(password)).await;
    theirs.ping().await.unwrap();
    theirs.close().await;

    let reference = UserRef {
        name: user.clone(),
        host: Some("%".into()),
    };
    assert!(
        admin::list_users(s, false)
            .await
            .unwrap()
            .iter()
            .any(|u| u.name == user && u.host.as_deref() == Some("%"))
    );
    let got = admin::load_user(s, &reference).await.unwrap().state;
    assert_eq!(got.global_privileges, ["PROCESS"]);
    assert_eq!(got.database_privileges, [privs(&db, &["SELECT", "INSERT"])]);
    assert_eq!(got.limits.unwrap().max_queries_per_hour, 1000);
    assert_eq!(got.limits.unwrap().max_user_connections, 3);

    let mut edited = got.clone();
    edited.name = user2.clone();
    edited.global_privileges = vec!["SHOW DATABASES".into()];
    edited.database_privileges = vec![privs(&db, &["SELECT", "UPDATE"])];
    edited.limits = Some(ResourceLimits::default());
    let r = admin::apply_user(
        s,
        &UserChange {
            original: Some(got),
            edited: Some(edited.clone()),
            password: None,
        },
    )
    .await
    .unwrap();
    assert!(r.error.is_none(), "{:?}", r.error);
    let again = admin::load_user(
        s,
        &UserRef {
            name: user2.clone(),
            host: Some("%".into()),
        },
    )
    .await
    .unwrap()
    .state;
    assert_eq!(again.global_privileges, ["SHOW DATABASES"]);
    assert_eq!(
        again.database_privileges,
        [privs(&db, &["SELECT", "UPDATE"])]
    );
    assert_eq!(again.limits, Some(ResourceLimits::default()));

    let r = admin::apply_user(
        s,
        &UserChange {
            original: Some(again),
            edited: None,
            password: None,
        },
    )
    .await
    .unwrap();
    assert!(r.error.is_none(), "{:?}", r.error);
    ok(s, AdminDdl::DropDatabase { name: db }).await;
    session.close().await;
}
