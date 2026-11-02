//! The server's connections as the process list (`CLIENT LIST`) and killing them
//! (`CLIENT KILL ID`). Redis runs one command at a time and cannot stop one but a script, so
//! Kill Query and Kill Connection both end the connection.

use crate::error::AppError;
use crate::sqlgen::native::redis as cmd;
use crate::types::admin::{KillOutcome, ProcessRow};
use crate::types::browse::TraceCategory;

use super::session::{RedisSession, read_only_error};
use super::values::{as_int, as_text};

/// One `CLIENT LIST` line (`id=7 addr=… name=… age=3 idle=0 flags=N db=0 … cmd=get user=default
/// lib-name=…`) as a process row; `None` without an id. Names are `key=value` words; a value
/// holds no space, except `name`, which cannot (the server refuses spaces in it).
pub fn parse_line(line: &str, own: &[i64], current: Option<i64>) -> Option<ProcessRow> {
    let field = |name: &str| {
        line.split_ascii_whitespace()
            .find_map(|w| w.strip_prefix(name)?.strip_prefix('='))
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    let id: i64 = field("id")?.parse().ok()?;
    let secs = |name: &str| field(name)?.parse::<i64>().ok();
    let application = field("name").or_else(|| field("lib-name"));
    Some(ProcessRow {
        id: id.to_string(),
        user: field("user"),
        database: field("db").map(|d| format!("db{d}")),
        application,
        client: field("addr"),
        state: field("flags"),
        command: field("cmd"),
        started_at: None,
        // Time since the connection's last command.
        duration_ms: secs("idle").map(|s| s.saturating_mul(1000)),
        wait_event: None,
        query: field("cmd"),
        current: current == Some(id),
        own: own.contains(&id),
    })
}

pub async fn list(s: &RedisSession) -> Result<Vec<ProcessRow>, AppError> {
    let reply = s
        .read_one(s.default_db, TraceCategory::Structure, &cmd::client_list())
        .await?;
    let text = as_text(&reply).unwrap_or_default();
    let own = s.own_ids();
    let current = s.reader(s.default_db).await.ok().and_then(|l| l.id);
    Ok(text
        .lines()
        .filter_map(|line| parse_line(line, &own, current))
        .collect())
}

pub async fn kill(s: &RedisSession, ids: &[String]) -> Vec<KillOutcome> {
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let outcome = match (s.read_only, id.trim().parse::<i64>()) {
            (true, _) => Err(read_only_error().to_string()),
            (false, Err(_)) => Err(format!("invalid client id {id}")),
            (false, Ok(n)) => {
                match s
                    .read_one(s.default_db, TraceCategory::Data, &cmd::client_kill(n))
                    .await
                {
                    Ok(r) if as_int(&r) == Some(1) => Ok(()),
                    Ok(_) => Err("no such client (it may have disconnected)".to_string()),
                    Err(e) => Err(e.to_string()),
                }
            }
        };
        out.push(KillOutcome {
            id: id.clone(),
            ok: outcome.is_ok(),
            message: outcome.err(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_list_lines() {
        let line = "id=7 addr=10.0.0.5:52555 laddr=10.0.0.1:6379 fd=8 name= age=30 idle=2 \
                    flags=N db=3 sub=0 psub=0 multi=-1 cmd=client|list user=app \
                    lib-name=tabler lib-ver=0.1.0";
        let row = parse_line(line, &[7], Some(9)).unwrap();
        assert_eq!(row.id, "7");
        assert_eq!(row.user.as_deref(), Some("app"));
        assert_eq!(row.database.as_deref(), Some("db3"));
        assert_eq!(row.application.as_deref(), Some("tabler"));
        assert_eq!(row.client.as_deref(), Some("10.0.0.5:52555"));
        assert_eq!(row.command.as_deref(), Some("client|list"));
        assert_eq!(row.duration_ms, Some(2000));
        assert!(row.own && !row.current);
        let named = parse_line("id=9 addr=x name=worker db=0 idle=0", &[], Some(9)).unwrap();
        assert_eq!(named.application.as_deref(), Some("worker"));
        assert!(named.current && !named.own);
        // `laddr=` is not `addr=`; a line without an id is skipped.
        assert_eq!(parse_line("laddr=1 id=2", &[], None).unwrap().client, None);
        assert!(parse_line("addr=x db=0", &[], None).is_none());
        assert!(parse_line("id=abc", &[], None).is_none());
    }
}
