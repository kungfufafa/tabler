//! Backup / restore through the real binaries found on this Mac (pg_dump + psql / pg_restore,
//! mysqldump + mysql) against the servers in `.env`. Gated on `TABLER_TEST_PG_URL` /
//! `TABLER_TEST_MYSQL_URL` (and on the binaries being installed): each test prints a skip notice
//! and passes otherwise. The dump of `tabler_dev`'s schema is restored into a scratch database
//! named `backup_it_<random>`, dropped again (also when an assertion fails).

#![allow(clippy::result_large_err)]

use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use futures::FutureExt as _;
use tabler_lib::backup::args::{self, ConnParams, Endpoint, ToolInfo};
use tabler_lib::backup::{detect, discover, run};
use tabler_lib::types::backup::{BackupEvent, DumpTool, RestoreFileKind, VersionFlavor};
use tabler_lib::types::profile::{Driver, SslMode};
use tokio_util::sync::CancellationToken;

struct Url {
    user: Option<String>,
    password: Option<String>,
    host: String,
    port: u16,
    database: String,
}

fn parse_url(url: &str, default_port: u16) -> Url {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let (auth, rest) = rest.rsplit_once('@').unwrap_or(("", rest));
    let (user, password) = match auth.split_once(':') {
        Some((u, p)) => (Some(u.to_string()), Some(p.to_string())),
        None if !auth.is_empty() => (Some(auth.to_string()), None),
        None => (None, None),
    };
    let (hostport, database) = rest.split_once('/').unwrap_or((rest, ""));
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().unwrap_or(default_port)),
        None => (hostport.to_string(), default_port),
    };
    Url {
        user,
        password,
        host,
        port,
        database: database.split('?').next().unwrap_or("").to_string(),
    }
}

fn conn(driver: Driver, url: &Url, database: &str) -> ConnParams {
    ConnParams {
        driver,
        endpoint: Endpoint {
            host: url.host.clone(),
            port: url.port,
            via_tunnel: false,
        },
        tls_host: Some(url.host.clone()),
        user: url.user.clone(),
        password: url.password.clone(),
        database: database.to_string(),
        ssl: SslMode::Disable,
        ssl_ca: None,
        connect_timeout_secs: 10,
        system_ca: None,
    }
}

async fn tool(driver: Driver, tool: DumpTool) -> Option<ToolInfo> {
    let path = std::env::var_os("PATH");
    let found = discover::discover(Path::new("/"), path.as_deref(), driver).await;
    // The newest binary of the tool (the servers here are current); MySQL's own client for MySQL.
    let b = found.into_iter().find(|b| {
        b.tool == tool
            && (driver != Driver::Mysql
                || b.version
                    .as_ref()
                    .is_some_and(|v| v.flavor == VersionFlavor::Mysql))
    })?;
    Some(ToolInfo {
        path: PathBuf::from(&b.path),
        tool,
        version: b.version,
    })
}

fn collector() -> (run::Emit, Arc<Mutex<Vec<BackupEvent>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    (Arc::new(move |e| sink.lock().unwrap().push(e)), events)
}

fn scratch_name() -> String {
    format!(
        "backup_it_{}",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    )
}

/// Runs a one-off statement with the restore client (psql -c / mysql -e).
fn admin(client: &ToolInfo, driver: Driver, url: &Url, sql: &str) {
    let mut cmd = std::process::Command::new(&client.path);
    match driver {
        Driver::Postgres => {
            cmd.arg(format!("--host={}", url.host))
                .arg(format!("--port={}", url.port))
                .arg("--no-password")
                .arg("--no-psqlrc")
                .arg(format!("--dbname={}", url.database))
                .arg("-c")
                .arg(sql);
            if let Some(u) = &url.user {
                cmd.arg(format!("--username={u}"));
            }
            if let Some(p) = &url.password {
                cmd.env("PGPASSWORD", p);
            }
        }
        _ => {
            cmd.arg(format!("--host={}", url.host))
                .arg(format!("--port={}", url.port))
                .arg("--protocol=TCP")
                .arg("-e")
                .arg(sql);
            if let Some(u) = &url.user {
                cmd.arg(format!("--user={u}"));
            }
            if let Some(p) = &url.password {
                cmd.env("MYSQL_PWD", p);
            }
        }
    }
    let out = cmd.output().expect("run the admin client");
    assert!(
        out.status.success(),
        "{sql}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn postgres_dump_and_restore_round_trip() {
    let Ok(raw) = std::env::var("TABLER_TEST_PG_URL") else {
        eprintln!("skipped: TABLER_TEST_PG_URL is not set");
        return;
    };
    let url = parse_url(&raw, 5432);
    let (Some(dump), Some(psql), Some(restore)) = (
        tool(Driver::Postgres, DumpTool::PgDump).await,
        tool(Driver::Postgres, DumpTool::Psql).await,
        tool(Driver::Postgres, DumpTool::PgRestore).await,
    ) else {
        eprintln!("skipped: pg_dump / psql / pg_restore not installed");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let scratch = scratch_name();
    admin(
        &psql,
        Driver::Postgres,
        &url,
        &format!("CREATE DATABASE {scratch}"),
    );
    let result = AssertUnwindSafe(async {
        // Plain SQL, gzipped, restored with psql.
        let inv = args::build_backup(
            &dump,
            &conn(Driver::Postgres, &url, &url.database),
            &["--schema-only".into(), "--no-owner".into()],
            true,
        )
        .unwrap();
        let target = dir.path().join("schema.sql.gz");
        let (emit, events) = collector();
        let out = run::run(inv, Some(target.clone()), emit, CancellationToken::new())
            .await
            .expect("pg_dump");
        assert!(out.bytes > 0);
        assert!(matches!(
            events.lock().unwrap().first(),
            Some(BackupEvent::Started { .. })
        ));
        let file = detect::inspect_file(&target).unwrap();
        assert_eq!((file.kind, file.gzip), (RestoreFileKind::Sql, true));
        let inv = args::build_restore(
            &psql,
            &conn(Driver::Postgres, &url, &scratch),
            &["-v ON_ERROR_STOP=1".into()],
            &file,
        )
        .unwrap();
        let (emit, _) = collector();
        let out = run::run(inv, None, emit, CancellationToken::new())
            .await
            .expect("psql restore");
        assert_eq!(out.bytes, file.size);

        // Custom archive, restored with pg_restore --clean.
        let inv = args::build_backup(
            &dump,
            &conn(Driver::Postgres, &url, &url.database),
            &["--schema-only".into(), "--format=custom".into()],
            false,
        )
        .unwrap();
        let target = dir.path().join("schema.dump");
        let (emit, _) = collector();
        run::run(inv, Some(target.clone()), emit, CancellationToken::new())
            .await
            .expect("pg_dump -Fc");
        let file = detect::inspect_file(&target).unwrap();
        assert_eq!(file.kind, RestoreFileKind::PgCustom);
        let inv = args::build_restore(
            &restore,
            &conn(Driver::Postgres, &url, &scratch),
            &[
                "--clean".into(),
                "--if-exists".into(),
                "--no-owner".into(),
                "--exit-on-error".into(),
            ],
            &file,
        )
        .unwrap();
        let (emit, _) = collector();
        run::run(inv, None, emit, CancellationToken::new())
            .await
            .expect("pg_restore");

        // A wrong password / unknown database fails with the binary's message.
        let inv = args::build_backup(
            &dump,
            &conn(Driver::Postgres, &url, "backup_it_does_not_exist"),
            &[],
            false,
        )
        .unwrap();
        let (emit, _) = collector();
        let err = run::run(
            inv,
            Some(dir.path().join("missing.sql")),
            emit,
            CancellationToken::new(),
        )
        .await
        .err()
        .expect("unknown database");
        assert!(err.to_string().starts_with("pg_dump failed:"), "{err}");
        assert!(!dir.path().join("missing.sql").exists());
    })
    .catch_unwind()
    .await;
    admin(
        &psql,
        Driver::Postgres,
        &url,
        &format!("DROP DATABASE IF EXISTS {scratch}"),
    );
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_dump_and_restore_round_trip() {
    let Ok(raw) = std::env::var("TABLER_TEST_MYSQL_URL") else {
        eprintln!("skipped: TABLER_TEST_MYSQL_URL is not set");
        return;
    };
    let url = parse_url(&raw, 3306);
    let (Some(dump), Some(client)) = (
        tool(Driver::Mysql, DumpTool::Mysqldump).await,
        tool(Driver::Mysql, DumpTool::Mysql).await,
    ) else {
        eprintln!("skipped: mysqldump / mysql not installed");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let scratch = scratch_name();
    admin(
        &client,
        Driver::Mysql,
        &url,
        &format!("CREATE DATABASE {scratch}"),
    );
    let result = AssertUnwindSafe(async {
        let inv = args::build_backup(
            &dump,
            &conn(Driver::Mysql, &url, &url.database),
            &[
                "--no-data".into(),
                "--skip-lock-tables".into(),
                // A GTID-enabled server refuses its own GTID_PURGED back.
                "--set-gtid-purged=OFF".into(),
            ],
            true,
        )
        .unwrap();
        let target = dir.path().join("schema.sql.gz");
        let (emit, _) = collector();
        let out = run::run(inv, Some(target.clone()), emit, CancellationToken::new())
            .await
            .expect("mysqldump");
        assert!(out.bytes > 0);
        let file = detect::inspect_file(&target).unwrap();
        assert_eq!((file.kind, file.gzip), (RestoreFileKind::Sql, true));
        let inv =
            args::build_restore(&client, &conn(Driver::Mysql, &url, &scratch), &[], &file).unwrap();
        let (emit, _) = collector();
        run::run(inv, None, emit, CancellationToken::new())
            .await
            .expect("mysql restore");
    })
    .catch_unwind()
    .await;
    admin(
        &client,
        Driver::Mysql,
        &url,
        &format!("DROP DATABASE IF EXISTS {scratch}"),
    );
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
